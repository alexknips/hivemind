//! Decision context features: condition signals for why a decision was made the way it was.
//!
//! Derives INDEPENDENT VARIABLES for failure-mode attribution — the circumstances under
//! which a decision was captured — purely from the graph. No LLMs. Works on any deployment.
//!
//! Five feature groups:
//!
//! - `authorship`: who produced the decision (human-authored / agent-proposed+human-accepted /
//!   agent-only / unknown) via `PROPOSED_BY` and `ACCEPTED_BY` edges.
//! - `source` / `source_ref`: which system or model captured it (cli/agent/human/slack/…).
//! - `review`: how substantively the decision was reviewed (unreviewed / self_accepted /
//!   peer_reviewed / disputed).
//! - `evidence_count` / `hypothesis_count`: sufficiency of supporting substrate.
//! - `options_count` / `rationale_chars`: context richness proxies.
//!
//! Pair with `DecisionOutcome` from `outcome.rs` for causal attribution: context is the
//! independent variable, outcome is the dependent variable.

use std::time::Instant;

use serde::Serialize;

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::shared::{
    optional_int, optional_string, query_error, query_timer_start, required_string,
    MAX_QUERY_RESULTS,
};
use super::QueryResponse;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// How a decision was authored: who produced it and whether a peer reviewed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorshipShape {
    /// The actor who proposed the decision is a human (`actor_id` starts with `human:`).
    HumanAuthored,
    /// An agent proposed the decision and at least one human actor accepted it.
    AgentProposedHumanAccepted,
    /// An agent proposed the decision; no human actor appears in `ACCEPTED_BY` edges.
    AgentOnly,
    /// No `PROPOSED_BY` edge exists (e.g., decision arrived via a path that omitted provenance).
    Unknown,
}

/// How substantively the decision was reviewed by actors other than the proposer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewShape {
    /// No `ACCEPTED_BY` or `REJECTED_BY` edges — decision was never explicitly reviewed.
    Unreviewed,
    /// The only `ACCEPTED_BY` actor is the proposer themselves (self-acceptance).
    SelfAccepted,
    /// At least one `ACCEPTED_BY` actor is distinct from the proposer.
    PeerReviewed,
    /// Both `ACCEPTED_BY` and `REJECTED_BY` actors exist — actively contested.
    Disputed,
}

/// Derived context record for a single decision: the conditions under which it was made.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionContext {
    pub decision_id: String,
    /// Authorship shape derived from `PROPOSED_BY` and `ACCEPTED_BY` edges.
    pub authorship: AuthorshipShape,
    /// Actor ID of the proposer (`PROPOSED_BY` target), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposer_id: Option<String>,
    /// Source system that captured the decision (cli / agent / human / slack / document / api).
    pub source: String,
    /// Free-text model/session reference from the capturing event (e.g. `claude:opus:session-xyz`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Review depth: was the decision examined by a peer?
    pub review: ReviewShape,
    /// Number of actors who accepted the decision.
    pub accepted_count: i64,
    /// Number of actors who rejected the decision.
    pub rejected_count: i64,
    /// Number of `BASED_ON` evidence edges.
    pub evidence_count: i64,
    /// Number of hypotheses premised on (via options or direct `PREMISED_ON_DIRECT` edges).
    pub hypothesis_count: i64,
    /// Number of options considered (`HAS_OPTION` edges).
    pub options_count: i64,
    /// Character count of the decision rationale (proxy for context richness).
    pub rationale_chars: i64,
}

/// Request for the bulk context-candidates query.
#[derive(Clone, Debug, Default)]
pub struct DecisionContextRequest {
    /// Minimum ledger offset to include (inclusive). Filters by the decision's `event_origin`.
    pub since_event_origin: Option<i64>,
    /// Maximum number of results (capped at `MAX_QUERY_RESULTS`).
    pub limit: usize,
    /// Skip N results (offset-based cursor).
    pub cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Single-decision context
// ---------------------------------------------------------------------------

/// Derive the context record for a single decision, or `None` if the decision does not exist.
pub fn get_decision_context(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionContext>>> {
    let started = query_timer_start();

    let decision_rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) \
         RETURN d.id AS id, d.source AS source, d.source_ref AS source_ref, d.rationale AS rationale \
         LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let data = if let Some(row) = decision_rows.first() {
        let id = required_string(row, "id")?;
        let source = optional_string(row, "source").unwrap_or_else(|| "cli".to_owned());
        let source_ref = optional_string(row, "source_ref");
        let rationale_chars = optional_string(row, "rationale")
            .map(|r| r.len() as i64)
            .unwrap_or(0);
        Some(derive_context(
            graph,
            &id,
            source,
            source_ref,
            rationale_chars,
        )?)
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
// Bulk context-candidates query
// ---------------------------------------------------------------------------

/// Return context records for all decisions (or a filtered subset), paginated.
/// Complements `get_decision_quality_candidates` — context is the independent variable
/// side of the causal pair.
pub fn get_decision_context_candidates(
    graph: &impl GraphView,
    request: &DecisionContextRequest,
) -> Result<QueryResponse<Vec<DecisionContext>>> {
    let started = Instant::now();
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

    let decision_rows = if let Some(since) = request.since_event_origin {
        graph.query(
            "MATCH (d:`Decision`) WHERE d.event_origin >= $since \
             RETURN d.id AS id, d.source AS source, d.source_ref AS source_ref, d.rationale AS rationale \
             ORDER BY d.event_origin, d.id;",
            &GraphParams::from([("since".to_owned(), GraphValue::Int(since))]),
        )?
    } else {
        graph.query(
            "MATCH (d:`Decision`) \
             RETURN d.id AS id, d.source AS source, d.source_ref AS source_ref, d.rationale AS rationale \
             ORDER BY d.event_origin, d.id;",
            &GraphParams::new(),
        )?
    };

    let paged: Vec<_> = decision_rows
        .into_iter()
        .skip(skip)
        .take(limit + 1)
        .collect();
    let truncated = paged.len() > limit;
    let window = paged.get(..paged.len().min(limit)).unwrap_or(&[]);

    let mut contexts = Vec::with_capacity(window.len());
    for row in window {
        let id = required_string(row, "id")?;
        let source = optional_string(row, "source").unwrap_or_else(|| "cli".to_owned());
        let source_ref = optional_string(row, "source_ref");
        let rationale_chars = optional_string(row, "rationale")
            .map(|r| r.len() as i64)
            .unwrap_or(0);
        contexts.push(derive_context(
            graph,
            &id,
            source,
            source_ref,
            rationale_chars,
        )?);
    }

    Ok(QueryResponse {
        result_count: contexts.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: contexts,
    })
}

// ---------------------------------------------------------------------------
// Core derivation logic
// ---------------------------------------------------------------------------

fn derive_context(
    graph: &impl GraphView,
    decision_id: &str,
    source: String,
    source_ref: Option<String>,
    rationale_chars: i64,
) -> Result<DecisionContext> {
    let proposer = query_proposer(graph, decision_id)?;
    let proposer_id = proposer.as_ref().map(|(id, _)| id.clone());
    let proposer_kind = proposer.as_ref().map(|(_, k)| k.as_str()).unwrap_or("");

    let acceptors = query_actor_ids_by_edge(graph, decision_id, "ACCEPTED_BY")?;
    let accepted_count = acceptors.len() as i64;

    let rejected_count = query_actor_edge_count(graph, decision_id, "REJECTED_BY")?;

    let authorship = derive_authorship(proposer_kind, &acceptors);
    let review = derive_review(
        proposer_id.as_deref(),
        &acceptors,
        accepted_count,
        rejected_count,
    );

    let evidence_count = query_edge_count(graph, decision_id, "BASED_ON", "Evidence")?;
    let hypothesis_count = query_hypothesis_count(graph, decision_id)?;
    let options_count = query_edge_count(graph, decision_id, "HAS_OPTION", "Option")?;

    Ok(DecisionContext {
        decision_id: decision_id.to_owned(),
        authorship,
        proposer_id,
        source,
        source_ref,
        review,
        accepted_count,
        rejected_count,
        evidence_count,
        hypothesis_count,
        options_count,
        rationale_chars,
    })
}

fn derive_authorship(proposer_kind: &str, acceptors: &[(String, String)]) -> AuthorshipShape {
    match proposer_kind {
        "human" => AuthorshipShape::HumanAuthored,
        "agent" => {
            let has_human_acceptor = acceptors.iter().any(|(_, k)| k == "human");
            if has_human_acceptor {
                AuthorshipShape::AgentProposedHumanAccepted
            } else {
                AuthorshipShape::AgentOnly
            }
        }
        _ => AuthorshipShape::Unknown,
    }
}

fn derive_review(
    proposer_id: Option<&str>,
    acceptors: &[(String, String)],
    accepted_count: i64,
    rejected_count: i64,
) -> ReviewShape {
    if accepted_count > 0 && rejected_count > 0 {
        return ReviewShape::Disputed;
    }
    if accepted_count == 0 {
        return ReviewShape::Unreviewed;
    }
    // accepted_count > 0, rejected_count == 0
    let only_self = match proposer_id {
        Some(pid) => acceptors.len() == 1 && acceptors.first().is_some_and(|(id, _)| id == pid),
        None => false,
    };
    if only_self {
        ReviewShape::SelfAccepted
    } else {
        ReviewShape::PeerReviewed
    }
}

// ---------------------------------------------------------------------------
// Graph sub-queries
// ---------------------------------------------------------------------------

/// Returns `Some((actor_id, kind))` for the single `PROPOSED_BY` actor, or `None`.
fn query_proposer(graph: &impl GraphView, decision_id: &str) -> Result<Option<(String, String)>> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id})-[:`PROPOSED_BY`]->(a:`Actor`) \
         RETURN a.id AS actor_id, a.kind AS kind \
         LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows.first().map(|row| {
        let actor_id = optional_string(row, "actor_id").unwrap_or_default();
        let kind = optional_string(row, "kind").unwrap_or_default();
        (actor_id, kind)
    }))
}

/// Returns `(actor_id, kind)` pairs for all actors connected via the given relationship label.
fn query_actor_ids_by_edge(
    graph: &impl GraphView,
    decision_id: &str,
    relation: &str,
) -> Result<Vec<(String, String)>> {
    let cypher = format!(
        "MATCH (d:`Decision` {{id: $id}})-[:`{relation}`]->(a:`Actor`) \
         RETURN a.id AS actor_id, a.kind AS kind \
         ORDER BY a.id;"
    );
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let actor_id = optional_string(&row, "actor_id").unwrap_or_default();
        let kind = optional_string(&row, "kind").unwrap_or_default();
        result.push((actor_id, kind));
    }
    Ok(result)
}

/// Returns the count of actors connected via the given relationship label.
fn query_actor_edge_count(
    graph: &impl GraphView,
    decision_id: &str,
    relation: &str,
) -> Result<i64> {
    let cypher = format!(
        "MATCH (d:`Decision` {{id: $id}})-[:`{relation}`]->(:`Actor`) \
         RETURN count(*) AS cnt;"
    );
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows
        .first()
        .and_then(|r| optional_int(r, "cnt"))
        .unwrap_or(0))
}

/// Returns the count of nodes connected via a given relationship label.
fn query_edge_count(
    graph: &impl GraphView,
    decision_id: &str,
    relation: &str,
    target_kind: &str,
) -> Result<i64> {
    let cypher = format!(
        "MATCH (d:`Decision` {{id: $id}})-[:`{relation}`]->(:`{target_kind}`) \
         RETURN count(*) AS cnt;"
    );
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows
        .first()
        .and_then(|r| optional_int(r, "cnt"))
        .unwrap_or(0))
}

/// Returns the total hypothesis count (via options or direct `PREMISED_ON_DIRECT` edges).
fn query_hypothesis_count(graph: &impl GraphView, decision_id: &str) -> Result<i64> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id})-[:`CHOSE`]->(:`Option`)-[:`PREMISED_ON`]->(h:`Hypothesis`) RETURN h.id AS hid \
         UNION \
         MATCH (d:`Decision` {id: $id})-[:`PREMISED_ON_DIRECT`]->(h:`Hypothesis`) RETURN h.id AS hid \
         ORDER BY hid;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows.len() as i64)
}

// ---------------------------------------------------------------------------
// Cursor helpers (public for MCP serialization)
// ---------------------------------------------------------------------------

pub fn context_next_cursor(skip: usize, window_len: usize) -> Option<String> {
    if window_len > 0 {
        Some((skip + window_len).to_string())
    } else {
        None
    }
}
