//! Situational decision surfacing: "what should I know before I touch this?" answered
//! from the working situation itself (touched paths, a branch name, a cwd) rather than
//! a hand-typed question. See hivemind-tenv.2.
//!
//! Matching is entirely deterministic and portable: an exact `topic_keys` membership
//! check (same primitive `get_relevant_decisions` uses) plus a term-overlap heuristic
//! against evidence content, both read via the already-dispatched `node_rows`/
//! `relation_edges` scans. No SQLite FTS, no `postgres.rs` changes, no LLM (AGENTS.md
//! Principles 1/7) — this is the same tier of determinism as `search.rs`'s rank system.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::ledger::{EventLedger, TenantScopedLedger};
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::decision::{get_decision, DecisionView};
use super::history::{
    get_decisions_changed_since, ChangeBoundary, ChangedSinceRequest, HistoryFilterRequest,
};
use super::outcome::{get_decision_outcome, DecisionOutcome};
use super::shared::{
    node_rows, normalized_limit, optional_int, optional_string, optional_string_list, parse_cursor,
    query_error, relation_edges, MAX_QUERY_RESULTS,
};
use super::terms::{overlapping_terms, path_terms, text_terms};
use super::{QueryContext, QueryResponse};

/// One reason a decision surfaced for the given situation, with enough detail that
/// "why did this show up" is inspectable rather than a black-box score (AGENTS.md §6).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchReason {
    /// Exact membership: the decision's `topic_keys` contains this term verbatim.
    TopicKey { topic: String },
    /// Fuzzy: an evidence item this decision is `BASED_ON` shares these terms with the
    /// situation. Term overlap over free prose, not a structural path reference —
    /// evidence content has no `paths` field to match exactly against.
    EvidenceOverlap {
        evidence_id: String,
        terms: Vec<String>,
    },
}

/// A decision that bears on the current situation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SituationalMatch {
    pub decision: DecisionView,
    pub outcome: DecisionOutcome,
    pub matched_via: Vec<MatchReason>,
    /// Fraction of the situation's terms this decision's topic/evidence covers, 0.0-1.0.
    pub score: f64,
    /// `Some(true/false)` when `--since`/`--since-branch-point` was supplied: whether
    /// this decision changed within that window. `None` when no boundary was requested
    /// — absence of the field is never used to imply "unchanged".
    pub changed_since: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SituationalRequest {
    /// The situation surface: touched file/dir paths, a branch name, or a cwd path.
    /// All entries are tokenized identically via `path_terms`; the caller (CLI) is
    /// responsible for resolving cwd/diff/branch into this list via git — the query
    /// layer itself never shells out (AGENTS.md three-layer separation).
    pub paths: Vec<String>,
    pub since_offset: Option<u64>,
    pub since_timestamp: Option<DateTime<Utc>>,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SituationalResults {
    /// The deduped, normalized terms actually used to match — an agent can see exactly
    /// what was extracted from its situation, not just the raw input paths.
    pub query_terms: Vec<String>,
    /// Resolved `--since`/`--since-branch-point` boundary, when one was requested.
    pub since_boundary: Option<ChangeBoundary>,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub matches: Vec<SituationalMatch>,
}

pub fn get_situational_decisions(
    context: &QueryContext,
    graph: &impl GraphView,
    ledger: &impl EventLedger,
    request: &SituationalRequest,
) -> Result<QueryResponse<SituationalResults>> {
    let started = Instant::now();

    let paths: Vec<&str> = request
        .paths
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .collect();
    if paths.is_empty() {
        return Err(query_error(
            "situational query requires at least one path, branch name, or cwd term",
        )
        .into());
    }

    let query_terms: Vec<String> = paths
        .iter()
        .flat_map(|path| path_terms(path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let limit = normalized_limit(request.limit);
    let offset = parse_cursor(request.cursor.as_deref())?;

    let mut scored = score_candidates(graph, &query_terms)?;
    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(right.event_origin.cmp(&left.event_origin))
            .then(left.decision_id.cmp(&right.decision_id))
    });

    let changed_ids = if request.since_offset.is_some() || request.since_timestamp.is_some() {
        Some(changed_decision_ids(context, ledger, request)?)
    } else {
        None
    };
    let since_boundary = changed_ids.as_ref().map(|(boundary, _)| boundary.clone());
    let changed_decision_ids = changed_ids.map(|(_, ids)| ids);

    let total_matches = scored.len();
    let mut matches = Vec::new();
    for candidate in scored.into_iter().skip(offset).take(limit) {
        let Some(decision) = get_decision(graph, &candidate.decision_id)?.data else {
            continue;
        };
        let Some(outcome) = get_decision_outcome(graph, &candidate.decision_id)?.data else {
            continue;
        };
        let changed_since = changed_decision_ids
            .as_ref()
            .map(|ids| ids.contains(&candidate.decision_id));
        matches.push(SituationalMatch {
            decision,
            outcome,
            matched_via: candidate.reasons,
            score: candidate.score,
            changed_since,
        });
    }

    let next_offset = offset.saturating_add(matches.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: matches.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: SituationalResults {
            query_terms,
            since_boundary,
            limit,
            cursor: request.cursor.clone(),
            next_cursor,
            total_matches,
            matches,
        },
    })
}

struct ScoredCandidate {
    decision_id: String,
    event_origin: i64,
    score: f64,
    reasons: Vec<MatchReason>,
}

/// Reasons and matched terms accumulated for one decision, kept together so each
/// decision needs exactly one map entry rather than two parallel ones. Every decision
/// gets an entry up front (see `score_candidates`) so later passes can update in place
/// by reference instead of re-inserting (and re-cloning) its id as a map key.
#[derive(Default)]
struct Accumulated {
    event_origin: i64,
    reasons: Vec<MatchReason>,
    matched_terms: BTreeSet<String>,
}

fn score_candidates(
    graph: &impl GraphView,
    query_terms: &[String],
) -> Result<Vec<ScoredCandidate>> {
    let evidence_rows = node_rows(graph, NodeKind::Evidence)?;

    let mut evidence_to_decisions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (decision_id, evidence_id) in relation_edges(graph, RelationKind::BasedOn)? {
        evidence_to_decisions
            .entry(evidence_id)
            .or_default()
            .push(decision_id);
    }

    // Populated for every decision (not just matches) so the evidence pass below can
    // look candidates up by reference (`get_mut`) rather than by owned key (`entry`).
    let mut accumulated: BTreeMap<String, Accumulated> = BTreeMap::new();
    for (decision_id, row) in node_rows(graph, NodeKind::Decision)? {
        let topic_keys = optional_string_list(&row, "topic_keys");
        let topic_terms: Vec<String> = topic_keys.iter().map(|t| t.to_ascii_lowercase()).collect();
        let mut entry = Accumulated {
            event_origin: optional_int(&row, "event_origin").unwrap_or(0),
            ..Accumulated::default()
        };
        for hit in overlapping_terms(query_terms, &topic_terms) {
            entry
                .reasons
                .push(MatchReason::TopicKey { topic: hit.clone() });
            entry.matched_terms.insert(hit);
        }
        accumulated.insert(decision_id, entry);
    }

    for (evidence_id, row) in &evidence_rows {
        let content = optional_string(row, "content").unwrap_or_default();
        let evidence_terms = text_terms(&content);
        let hits = overlapping_terms(query_terms, &evidence_terms);
        if hits.is_empty() {
            continue;
        }
        let Some(citing_decisions) = evidence_to_decisions.get(evidence_id) else {
            continue;
        };
        for decision_id in citing_decisions {
            let Some(entry) = accumulated.get_mut(decision_id) else {
                continue;
            };
            entry.matched_terms.extend(hits.iter().cloned());
            entry.reasons.push(MatchReason::EvidenceOverlap {
                evidence_id: evidence_id.clone(),
                terms: hits.clone(),
            });
        }
    }

    let mut scored = Vec::with_capacity(accumulated.len());
    for (decision_id, entry) in accumulated {
        if entry.reasons.is_empty() {
            continue;
        }
        let score = if query_terms.is_empty() {
            0.0
        } else {
            entry.matched_terms.len() as f64 / query_terms.len() as f64
        };
        scored.push(ScoredCandidate {
            decision_id,
            event_origin: entry.event_origin,
            score,
            reasons: entry.reasons,
        });
    }

    Ok(scored)
}

/// Resolve `--since`/`--since-branch-point` into the boundary actually applied plus the
/// set of decision ids touched within it. Reuses `get_decisions_changed_since`
/// (`EventLedger`-generic, offset/timestamp-based) rather than a new ranking signal —
/// this is an annotation layered on top of the situational match set, per the parent
/// epic's mayor-approved decision D.
fn changed_decision_ids(
    context: &QueryContext,
    ledger: &impl EventLedger,
    request: &SituationalRequest,
) -> Result<(ChangeBoundary, BTreeSet<String>)> {
    let scoped = TenantScopedLedger::new(ledger, context.tenant_id.clone());
    let response = get_decisions_changed_since(
        &scoped,
        &ChangedSinceRequest {
            since_offset: request.since_offset,
            since_timestamp: request.since_timestamp,
            until_offset: None,
            until_timestamp: None,
            filters: HistoryFilterRequest::default(),
            limit: MAX_QUERY_RESULTS,
            cursor: None,
        },
    )?;
    let mut ids = BTreeSet::new();
    for item in &response.data.items {
        ids.extend(item.decision_ids.iter().cloned());
    }
    Ok((response.data.resolved_since, ids))
}
