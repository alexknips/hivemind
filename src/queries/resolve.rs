//! Resolve-by-description: the shared primitive behind every fluent follow-up verb.
//!
//! An agent that just ran a free-text search knows a decision's *content*, not an opaque id.
//! This module closes that gap deterministically — no LLM, no embeddings, no learned weights
//! (PRINCIPLES.md §1/§7) — by reusing `search.rs`'s existing rank-tier matcher
//! (`collect_resolver_candidates`) and adding `event_origin` (the ledger offset already on every
//! node) as a recency tiebreak. See docs/AGENT_FLUENT_QUERYING.md for the full design.
//!
//! Ambiguity is a value, not an error: `ResolveOutcome::Ambiguous` is the expected outcome when a
//! description matches more than one decision at the same confidence tier, matching this
//! project's stance that `contested` is a status, never a silently-collapsed error (AGENTS.md §6).
//! The gate is deliberately conservative and identical for every caller (read or write verb):
//! resolved only when exactly one candidate occupies the best rank tier.
//!
//! A description that names a word no decision contains ("why did we choose to move the demo
//! cell..." when the record never says "choose") is not a dead end: when no decision matches
//! every term, decisions matching most of them come back as an `Ambiguous` candidate list, each
//! carrying the terms it lacks. Close candidates are never auto-resolved.

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::search::collect_resolver_candidates;
use super::shared::{
    optional_int, optional_string, query_error, query_timer_start, MAX_QUERY_RESULTS,
};
use super::QueryResponse;

/// One candidate returned by the resolver, ordered by descending confidence
/// (ascending `rank`, then descending `event_origin` as the recency tiebreak).
///
/// `Deserialize` backs the CLI-layer `#N` continuation file (a local convenience cache, not
/// decision memory — see the `resolve_fluent_target` helper in `src/cli/run/mod.rs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedCandidate {
    pub decision_id: String,
    pub title: String,
    /// Reused tier from `evaluate_search_match`; 0 = exact id/title match, lower is better.
    pub rank: u8,
    /// Ledger offset at creation — recency tiebreak, never a ranking input on its own.
    pub event_origin: i64,
    pub matched_fields: Vec<String>,
    /// Description terms this decision does not contain. Empty for a full match; non-empty only
    /// for the close candidates offered when no decision matches every term.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_terms: Vec<String>,
}

/// Outcome of a resolve-by-description call: either a confident single match, or a candidate
/// list the caller must disambiguate via `--pick N` / `#N` or the `--id` escape hatch.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ResolveOutcome {
    Resolved { candidate: ResolvedCandidate },
    Ambiguous { candidates: Vec<ResolvedCandidate> },
    NotFound,
}

/// Resolve a free-text description to a decision. `topic_hint` narrows the candidate set the
/// same way `search --topic` / `get_relevant_decisions --topic` already do.
///
/// Resolved iff exactly one candidate occupies the best (lowest) rank tier — no numeric recency
/// margin, identical for read and write callers (mayor decision, hivemind-tenv.1, 2026-09-07).
/// Only decisions matching every term compete for that; close candidates (missing some terms) are
/// offered, as `Ambiguous`, only when there is no full match.
pub fn resolve_decision_by_description(
    graph: &impl GraphView,
    description: &str,
    topic_hint: Option<&str>,
) -> Result<QueryResponse<ResolveOutcome>> {
    let started = query_timer_start();
    let description = description.trim();
    if description.is_empty() {
        return Err(query_error("description must not be empty").into());
    }

    let topic_keys: Vec<String> = topic_hint
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
        .map(|topic| vec![topic.to_owned()])
        .unwrap_or_default();

    let mut rows = collect_resolver_candidates(graph, description, &topic_keys)?;
    let only_close_candidates =
        !rows.is_empty() && rows.iter().all(|row| !row.missing_terms.is_empty());
    if !only_close_candidates {
        rows.retain(|row| row.missing_terms.is_empty());
    }
    rows.sort_by(|left, right| {
        (
            left.missing_terms.len(),
            left.rank,
            Reverse(left.event_origin),
            &left.decision_id,
        )
            .cmp(&(
                right.missing_terms.len(),
                right.rank,
                Reverse(right.event_origin),
                &right.decision_id,
            ))
    });

    let truncated = rows.len() > MAX_QUERY_RESULTS;
    rows.truncate(MAX_QUERY_RESULTS);

    let mut candidates: Vec<ResolvedCandidate> = rows
        .into_iter()
        .map(|row| ResolvedCandidate {
            decision_id: row.decision_id,
            title: row.title,
            rank: row.rank,
            event_origin: row.event_origin,
            matched_fields: row.matched_fields,
            missing_terms: row.missing_terms,
        })
        .collect();

    let result_count = candidates.len();
    let outcome = if only_close_candidates {
        ResolveOutcome::Ambiguous { candidates }
    } else if let Some(best_rank) = candidates.first().map(|candidate| candidate.rank) {
        let best_tier_count = candidates.iter().filter(|c| c.rank == best_rank).count();
        if best_tier_count == 1 {
            ResolveOutcome::Resolved {
                candidate: candidates.remove(0),
            }
        } else {
            ResolveOutcome::Ambiguous { candidates }
        }
    } else {
        ResolveOutcome::NotFound
    };

    Ok(QueryResponse {
        result_count,
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: outcome,
    })
}

/// Look up a decision named by its literal id, as a grounding premise: `Resolved` when the
/// decision exists, `NotFound` when it does not. Same outcome shape as a description lookup so a
/// caller resolving a mixed list of ids and descriptions handles one type; the candidate carries
/// the decision's title so the caller can echo what it linked to.
pub fn resolve_decision_by_id(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<ResolveOutcome>> {
    let started = query_timer_start();
    let decision_id = decision_id.trim();
    if decision_id.is_empty() {
        return Err(query_error("decision id must not be empty").into());
    }

    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, d.title AS title, d.event_origin AS event_origin LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    let data = match rows.first() {
        Some(row) => ResolveOutcome::Resolved {
            candidate: ResolvedCandidate {
                decision_id: decision_id.to_owned(),
                title: optional_string(row, "title").unwrap_or_default(),
                rank: 0,
                event_origin: optional_int(row, "event_origin").unwrap_or(0),
                matched_fields: vec!["id".to_owned()],
                missing_terms: Vec::new(),
            },
        },
        None => ResolveOutcome::NotFound,
    };

    Ok(QueryResponse {
        result_count: usize::from(matches!(data, ResolveOutcome::Resolved { .. })),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

#[cfg(test)]
mod tests;
