//! Decision outcome derivation: pure graph-read signals for whether a decision held up.
//!
//! No LLMs. Works on any deployment (self-hosted or hosted). Four signals, each with
//! its contributing reasons attached:
//!
//! - `superseded`: a newer decision explicitly superseded this one; includes how
//!   many ledger events elapsed before that happened (`gap_events`).
//! - `stale_premises`: something this decision rests on no longer stands — a hypothesis it
//!   premises on has been refuted (a failed bet included), or a prior decision it follows from
//!   has been superseded or rejected.
//! - `contested`: the decision has both accepting and rejecting actors, unresolved.
//! - `thin_structure`: no options attached and/or nothing declared about what it rests on
//!   (never asked — a premise link, evidence, an assumption or a declared bet all count).
//!
//! `held_up` is true when superseded, stale_premises, and contested are all absent;
//! thin structure alone does not flip `held_up` to false — it is a quality signal,
//! not a soundness signal. Neither does an overdue bet: it is reported as `unchecked`
//! (attention, not staleness).

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::grounding::{
    grounding_state_of, premise_signals, GroundingState, StalePremise, UncheckedBet,
};
use super::shared::{
    optional_int, query_error, query_superseder, query_timer_start, required_string,
    MAX_QUERY_RESULTS,
};
use super::QueryResponse;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One reason why a decision may not have held up, with all context needed to understand it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutcomeReason {
    /// The decision was explicitly superseded by `by_id`.
    /// `gap_events` is the number of ledger events that elapsed between the decision being
    /// proposed and the supersession being recorded (smaller = faster turnaround).
    SupersededBy {
        by_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        gap_events: Option<i64>,
    },
    /// A hypothesis that this decision premises on has been refuted by evidence.
    PremisedOnRefuted { hypothesis_id: String },
    /// A prior decision this decision follows from has been superseded by `by_id`.
    PremiseSuperseded { decision_id: String, by_id: String },
    /// A prior decision this decision follows from has been rejected.
    PremiseRejected { decision_id: String },
    /// The decision is actively contested: at least one actor accepted it and at least one rejected it.
    Contested,
    /// The decision has thin structure — no options listed and/or nothing declared about what
    /// it rests on (`nothing_declared`: never asked, as opposed to a declared bet).
    ThinStructure {
        no_options: bool,
        nothing_declared: bool,
    },
}

/// Derived outcome record for a single decision.
///
/// `reasons` is empty when the decision is clean (no negative signals).
/// `held_up` is false when the decision is superseded, has stale premises, or is contested.
/// Thin structure alone does NOT set `held_up = false`, and neither does an overdue bet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionOutcome {
    pub decision_id: String,
    /// False when superseded, stale (a premise no longer stands), or contested.
    pub held_up: bool,
    pub superseded: bool,
    pub superseded_by: Option<String>,
    /// Ledger-event gap between decision proposal and supersession (proxy for speed).
    pub supersession_gap_events: Option<i64>,
    /// A hypothesis it premises on was refuted, or a prior decision it follows from was
    /// superseded or rejected. The reasons say which.
    pub stale_premises: bool,
    pub refuted_hypothesis_ids: Vec<String>,
    pub contested: bool,
    pub has_options: bool,
    pub has_evidence: bool,
    /// Whether anything was declared about what this decision rests on.
    pub grounding_state: GroundingState,
    /// Bets whose check date has passed with no evidence either way. Attention, not
    /// staleness: `held_up` is unaffected.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unchecked: Vec<UncheckedBet>,
    /// All contributing reasons, each carrying the details needed to understand it.
    pub reasons: Vec<OutcomeReason>,
}

/// Request for the bulk quality-candidates query.
#[derive(Clone, Debug, Default)]
pub struct DecisionQualityCandidatesRequest {
    /// Minimum ledger offset to include (inclusive). Filters by the decision's `event_origin`.
    pub since_event_origin: Option<i64>,
    /// Maximum number of results (capped at `MAX_QUERY_RESULTS`).
    pub limit: usize,
    /// Skip N results (offset-based cursor).
    pub cursor: Option<String>,
    /// When true, only return decisions that have at least one quality signal.
    pub only_with_signals: bool,
}

// ---------------------------------------------------------------------------
// Single-decision outcome
// ---------------------------------------------------------------------------

/// Derive the outcome record for a single decision, or `None` if the decision does not exist.
/// Reads the clock once, to say whether a bet's check date has passed.
pub fn get_decision_outcome(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionOutcome>>> {
    get_decision_outcome_at(graph, decision_id, Utc::now())
}

/// `get_decision_outcome` as of `now`.
pub fn get_decision_outcome_at(
    graph: &impl GraphView,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<QueryResponse<Option<DecisionOutcome>>> {
    let started = query_timer_start();

    // Existence check + own event_origin in one query.
    let decision_rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, d.event_origin AS event_origin LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let data = if let Some(row) = decision_rows.first() {
        let id = required_string(row, "id")?;
        let decision_event_origin = optional_int(row, "event_origin");
        Some(derive_outcome(graph, &id, decision_event_origin, now)?)
    } else {
        None
    };

    Ok(QueryResponse {
        result_count: usize::from(data.is_some()),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

// ---------------------------------------------------------------------------
// Bulk quality-candidates query
// ---------------------------------------------------------------------------

/// Return outcome records for all decisions (or a filtered subset), paginated.
/// Intended as the primary data-pull endpoint for external quality scorers.
pub fn get_decision_quality_candidates(
    graph: &impl GraphView,
    request: &DecisionQualityCandidatesRequest,
) -> Result<QueryResponse<Vec<DecisionOutcome>>> {
    let started = Instant::now();
    let now = Utc::now();
    let limit = if request.limit == 0 {
        MAX_QUERY_RESULTS
    } else {
        request.limit.min(MAX_QUERY_RESULTS)
    };
    let skip: usize = match request.cursor.as_deref() {
        None => 0,
        Some(c) => c
            .parse::<usize>()
            .map_err(|e| query_error(format!("cursor must be a non-negative offset: {e}")))?,
    };

    // Fetch all decision IDs + event_origins, optionally filtered by since_event_origin.
    // ORDER BY event_origin ensures a stable ordering for pagination.
    let decision_rows = if let Some(since) = request.since_event_origin {
        graph.query(
            "MATCH (d:`Decision`) WHERE d.event_origin >= $since RETURN d.id AS id, d.event_origin AS event_origin ORDER BY d.event_origin, d.id;",
            &GraphParams::from([("since".to_owned(), GraphValue::Int(since))]),
        )?
    } else {
        graph.query(
            "MATCH (d:`Decision`) RETURN d.id AS id, d.event_origin AS event_origin ORDER BY d.event_origin, d.id;",
            &GraphParams::new(),
        )?
    };

    let total = decision_rows.len();
    let paged: Vec<_> = decision_rows
        .into_iter()
        .skip(skip)
        .take(limit + 1)
        .collect();
    let truncated = paged.len() > limit;
    let window = paged.get(..paged.len().min(limit)).unwrap_or(&[]);

    let mut outcomes = Vec::with_capacity(window.len());
    for row in window {
        let id = required_string(row, "id")?;
        let event_origin = optional_int(row, "event_origin");
        let outcome = derive_outcome(graph, &id, event_origin, now)?;
        if !request.only_with_signals || !outcome.reasons.is_empty() {
            outcomes.push(outcome);
        }
    }

    let _ = total; // available for future use in metadata
    Ok(QueryResponse {
        result_count: outcomes.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: outcomes,
    })
}

// ---------------------------------------------------------------------------
// Core derivation logic
// ---------------------------------------------------------------------------

fn derive_outcome(
    graph: &impl GraphView,
    decision_id: &str,
    decision_event_origin: Option<i64>,
    now: DateTime<Utc>,
) -> Result<DecisionOutcome> {
    let mut reasons = Vec::new();

    // --- Signal 1: superseded ---
    let supersession = query_superseder(graph, decision_id)?;
    let superseded = supersession.is_some();
    let (superseded_by, supersession_gap_events) = if let Some((by_id, edge_origin)) = supersession
    {
        let gap = decision_event_origin
            .zip(edge_origin)
            .map(|(d, s)| s - d)
            .filter(|&g| g >= 0); // guard against clock/origin anomalies
        reasons.push(OutcomeReason::SupersededBy {
            by_id: by_id.clone(),
            gap_events: gap,
        });
        (Some(by_id), gap)
    } else {
        (None, None)
    };

    // --- Signal 2: stale premises (a refuted hypothesis, or a prior decision that no longer
    // stands). A failed bet is a refuted hypothesis, so it lands here too. ---
    let raw_premise_ids = query_refuted_premises(graph, decision_id)?;
    let mut stale_premises = !raw_premise_ids.is_empty();
    for hyp_id in raw_premise_ids {
        reasons.push(OutcomeReason::PremisedOnRefuted {
            hypothesis_id: hyp_id,
        });
    }
    let signals = premise_signals(graph, decision_id, now)?;
    stale_premises |= !signals.stale.is_empty();
    for stale in signals.stale {
        reasons.push(match stale {
            StalePremise::Superseded { decision_id, by_id } => {
                OutcomeReason::PremiseSuperseded { decision_id, by_id }
            }
            StalePremise::Rejected { decision_id } => {
                OutcomeReason::PremiseRejected { decision_id }
            }
        });
    }
    // Reconstruct for the struct field — extracted from reasons after the loop to avoid
    // per-element clone inside the for body.
    let refuted_hypothesis_ids: Vec<String> = reasons
        .iter()
        .filter_map(|r| match r {
            OutcomeReason::PremisedOnRefuted { hypothesis_id } => Some(hypothesis_id.clone()),
            _ => None,
        })
        .collect();

    // --- Signal 3: contested ---
    let contested = query_contested(graph, decision_id)?;
    if contested {
        reasons.push(OutcomeReason::Contested);
    }

    // --- Signal 4: thin structure ---
    // "Nothing declared" is only true when no premise link, evidence, assumption or bet exists:
    // a declared bet is a positive answer to "what does this rest on?", not thin.
    let has_options = query_has_options(graph, decision_id)?;
    let has_evidence = query_has_evidence(graph, decision_id)?;
    let grounding_state = grounding_state_of(
        signals.premise_count,
        usize::from(has_evidence),
        signals.hypothesis_kinds.iter().copied(),
    );
    let nothing_declared = grounding_state == GroundingState::NothingDeclared;
    if !has_options || nothing_declared {
        reasons.push(OutcomeReason::ThinStructure {
            no_options: !has_options,
            nothing_declared,
        });
    }

    let held_up = !superseded && !stale_premises && !contested;

    Ok(DecisionOutcome {
        decision_id: decision_id.to_owned(),
        held_up,
        superseded,
        superseded_by,
        supersession_gap_events,
        stale_premises,
        refuted_hypothesis_ids,
        contested,
        has_options,
        has_evidence,
        grounding_state,
        unchecked: signals.unchecked,
        reasons,
    })
}

// ---------------------------------------------------------------------------
// Graph sub-queries
// ---------------------------------------------------------------------------

/// Returns the IDs of all hypotheses that this decision premises on that have been refuted.
fn query_refuted_premises(graph: &impl GraphView, decision_id: &str) -> Result<Vec<String>> {
    // Two paths: Decision -CHOSE-> Option -PREMISED_ON-> Hypothesis <-REFUTES- Evidence
    //            Decision -PREMISED_ON_DIRECT-> Hypothesis <-REFUTES- Evidence
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id})-[:`CHOSE`]->(o:`Option`)-[:`PREMISED_ON`]->(h:`Hypothesis`)<-[:`REFUTES`]-(:`Evidence`) \
         RETURN h.id AS hypothesis_id \
         UNION \
         MATCH (d:`Decision` {id: $id})-[:`PREMISED_ON_DIRECT`]->(h:`Hypothesis`)<-[:`REFUTES`]-(:`Evidence`) \
         RETURN h.id AS hypothesis_id \
         ORDER BY hypothesis_id;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(required_string(&row, "hypothesis_id")?);
    }
    Ok(ids)
}

/// True when the decision has both accepting and rejecting actors simultaneously.
fn query_contested(graph: &impl GraphView, decision_id: &str) -> Result<bool> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) \
         RETURN \
           COUNT { MATCH (d)-[:`ACCEPTED_BY`]->() } AS accepted_count, \
           COUNT { MATCH (d)-[:`REJECTED_BY`]->() } AS rejected_count;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    if let Some(row) = rows.first() {
        let accepted = optional_int(row, "accepted_count").unwrap_or(0);
        let rejected = optional_int(row, "rejected_count").unwrap_or(0);
        Ok(accepted > 0 && rejected > 0)
    } else {
        Ok(false)
    }
}

/// True when the decision has at least one option attached.
fn query_has_options(graph: &impl GraphView, decision_id: &str) -> Result<bool> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id})-[:`HAS_OPTION`]->(:`Option`) RETURN count(*) AS cnt LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows
        .first()
        .and_then(|r| optional_int(r, "cnt"))
        .unwrap_or(0)
        > 0)
}

/// True when the decision has at least one evidence item attached.
fn query_has_evidence(graph: &impl GraphView, decision_id: &str) -> Result<bool> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id})-[:`BASED_ON`]->(:`Evidence`) RETURN count(*) AS cnt LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows
        .first()
        .and_then(|r| optional_int(r, "cnt"))
        .unwrap_or(0)
        > 0)
}

// ---------------------------------------------------------------------------
// Cursor helpers (public for MCP serialization)
// ---------------------------------------------------------------------------

pub fn outcome_next_cursor(skip: usize, window_len: usize) -> Option<String> {
    if window_len > 0 {
        Some((skip + window_len).to_string())
    } else {
        None
    }
}
