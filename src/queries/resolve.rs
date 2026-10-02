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
//! The gate for a description that matches in full is deliberately conservative and identical for
//! every caller (read or write verb): resolved only when exactly one candidate occupies the best
//! rank tier.
//!
//! A description that names a word no decision contains ("why did we choose to move the demo
//! cell..." when the record never says "choose") is not a dead end: when no decision matches
//! every term, decisions matching most of them come back as an `Ambiguous` candidate list, each
//! carrying the terms it lacks. A verb that writes never auto-resolves a close candidate. A verb
//! that only reads (`why`, `verify`, ...) asks with `resolve_decision_for_reading`: it matches at
//! the bar `recall` uses, and answers with a close candidate that leads alone, saying what the
//! decision lacks, instead of making the asker repeat the question with `--pick`. Which candidate
//! leads is decided by how many terms each lacks and then by how many of the terms it did match
//! its title or topic keys carry (see `closeness`), so a decision about the thing asked about
//! beats one that only mentions the words somewhere in a long rationale.
//!
//! A negation in the description ("don't adopt Kafka", "why didn't we ...", "do not ...") is not a
//! word to find: it never appears in `missing_terms`. It is polarity. Only a decision whose own
//! title is negated answers a negated description; one that matches every other word with the
//! opposite polarity is a close candidate whose reason is `POLARITY_REASON`, and is never resolved
//! to, by a verb that writes or one that reads.

use std::cmp::Reverse;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::same_as::{fold_linked, RecordedCopy};
use super::search::{collect_resolver_candidates, CloseMatch, ResolverCandidateRow};
use super::shared::{
    optional_int, optional_string, query_error, query_timer_start, MAX_QUERY_RESULTS,
};
use super::terms::resolver_terms;
use super::QueryResponse;

/// Fewest description words a close candidate must share before a read verb answers with it
/// instead of listing it. Listing needs only half the words (as `recall` does); answering names
/// the decision as the one asked about, and one shared word out of two is too little for that.
const MIN_WORDS_TO_ANSWER_CLOSE: usize = 2;

/// Why a decision that has every word asked is still only a close candidate: the description is
/// negated and the decision's title is not. Shown beside the candidate wherever `missing_terms`
/// would be, so a reader sees the opposite polarity instead of being told it lacks "not".
pub const POLARITY_REASON: &str = "question is negated; this decision is not";

/// `serde` skip for a flag that is only worth saying when set.
fn is_false(value: &bool) -> bool {
    !*value
}

/// Who asks: a verb that acts on the decision it resolves, or one that only shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asker {
    /// `disagree`, `supersede`, `ground`, ...: a wrong pick writes to the wrong decision.
    Writer,
    /// `why`, `verify`, `chain`, `compact-view`: a wrong pick shows the wrong decision, and says
    /// what it lacks.
    Reader,
}

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
    /// The description is negated and this decision's title is not (`POLARITY_REASON`). Such a
    /// decision is a close candidate even with `missing_terms` empty, and is never `Resolved`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub polarity_mismatch: bool,
    /// Other records of this same decision, linked `SAME_AS` (hivemind-83cj): the candidate is the
    /// earliest recorded of them that matches the description, and these are the rest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_recorded_as: Vec<RecordedCopy>,
}

impl ResolvedCandidate {
    /// Whether this is a close candidate rather than a full match: it lacks some of the words
    /// asked, or it is the opposite of what a negated description asked for.
    pub fn is_close(&self) -> bool {
        !self.missing_terms.is_empty() || self.polarity_mismatch
    }

    /// Whether an answer resolved to this candidate has something to say about how it was
    /// resolved: words it lacks, or other records of the same decision.
    pub fn needs_annotation(&self) -> bool {
        !self.missing_terms.is_empty() || !self.also_recorded_as.is_empty()
    }
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

/// Resolve a free-text description to a decision, for a verb that writes to it. `topic_hint`
/// narrows the candidate set the same way `search --topic` / `get_relevant_decisions --topic`
/// already do.
///
/// Resolved iff exactly one candidate occupies the best (lowest) rank tier — no numeric recency
/// margin (mayor decision, hivemind-tenv.1, 2026-09-07). Only decisions matching every term
/// compete for that; close candidates (missing some terms) are offered, as `Ambiguous`, only when
/// there is no full match, and never resolved: more than half of the terms, and at least two.
pub fn resolve_decision_by_description(
    graph: &impl GraphView,
    description: &str,
    topic_hint: Option<&str>,
) -> Result<QueryResponse<ResolveOutcome>> {
    resolve_for(graph, description, topic_hint, Asker::Writer)
}

/// Resolve a free-text description to a decision, for a verb that only shows it (`why`,
/// `verify`, `chain`, `compact-view`). Full matches resolve exactly as for a writer. The two
/// differences are for a description no decision matches in full (hivemind-3lko):
///
/// - a close candidate needs at least half of the terms, the bar `recall` uses, so a question
///   `recall` answers is not answered with "no decision matches" here;
/// - when close candidates are all there is, and one is closer than the rest (lacks fewer terms,
///   or lacks as many but has more of the terms it matched in its title or topic keys) and shares
///   at least two terms, it is `Resolved` with its `missing_terms` set, so the caller shows the
///   decision and names what it lacks. Equally close candidates stay an `Ambiguous` list.
pub fn resolve_decision_for_reading(
    graph: &impl GraphView,
    description: &str,
    topic_hint: Option<&str>,
) -> Result<QueryResponse<ResolveOutcome>> {
    resolve_for(graph, description, topic_hint, Asker::Reader)
}

fn resolve_for(
    graph: &impl GraphView,
    description: &str,
    topic_hint: Option<&str>,
    asker: Asker,
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

    let close = match asker {
        Asker::Writer => CloseMatch::Majority,
        Asker::Reader => CloseMatch::Half,
    };
    // Records of one decision, linked `SAME_AS`, are one candidate; records with no link between
    // them stay separate, so a tie among them stays `Ambiguous`.
    let mut also_recorded_as: HashMap<String, Vec<RecordedCopy>> = HashMap::new();
    let mut rows: Vec<ResolverCandidateRow> = Vec::new();
    for folded in fold_linked(
        graph,
        collect_resolver_candidates(graph, description, &topic_keys, close)?,
        |row| row.decision_id.as_str(),
        absorb_record,
    )? {
        if !folded.also_recorded_as.is_empty() {
            also_recorded_as.insert(folded.item.decision_id.clone(), folded.also_recorded_as);
        }
        rows.push(folded.item);
    }
    let only_close_candidates = !rows.is_empty() && rows.iter().all(ResolverCandidateRow::is_close);
    if !only_close_candidates {
        rows.retain(|row| !row.is_close());
    }
    rows.sort_by(|left, right| {
        (
            closeness(left),
            left.rank,
            Reverse(left.event_origin),
            &left.decision_id,
        )
            .cmp(&(
                closeness(right),
                right.rank,
                Reverse(right.event_origin),
                &right.decision_id,
            ))
    });

    let truncated = rows.len() > MAX_QUERY_RESULTS;
    rows.truncate(MAX_QUERY_RESULTS);

    let leader_is_clear = only_close_candidates
        && asker == Asker::Reader
        && close_candidate_leads(resolver_terms(description).len(), &rows);

    let mut candidates: Vec<ResolvedCandidate> = rows
        .into_iter()
        .map(|row| ResolvedCandidate {
            also_recorded_as: also_recorded_as
                .remove(&row.decision_id)
                .unwrap_or_default(),
            decision_id: row.decision_id,
            title: row.title,
            rank: row.rank,
            event_origin: row.event_origin,
            matched_fields: row.matched_fields,
            missing_terms: row.missing_terms,
            polarity_mismatch: row.polarity_mismatch,
        })
        .collect();

    let result_count = candidates.len();
    let outcome = if only_close_candidates {
        if leader_is_clear {
            ResolveOutcome::Resolved {
                candidate: candidates.remove(0),
            }
        } else {
            ResolveOutcome::Ambiguous { candidates }
        }
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

/// How far a candidate is from the description, smaller being closer: it lacks fewer of the
/// terms, and between two that lack the same number, the one whose title or topic keys carry more
/// of the terms it matched. A decision about the product's name says "product" and "Upheld" in its
/// title; one that happens to say "product" and "called" somewhere in a long rationale is not
/// about that, though it lacks just as many words. Where the words matched is already in
/// `matched_fields`; nothing is counted across the ledger and nothing is learned. A full match is
/// 0 on both (its order is the rank tier's).
fn closeness(row: &ResolverCandidateRow) -> (usize, Reverse<usize>) {
    (row.missing_terms.len(), Reverse(row.headline_terms))
}

/// Whether the first of `rows` (all close, closest first) is the one asked about: it is closer
/// than the next, by `closeness`, and shares enough terms to be named as the answer. Equal
/// closeness is not resolved: after it the lists are ordered by rank and recency, which say
/// nothing about which of two equally close decisions was meant. A decision of the opposite
/// polarity is never the one asked about, however many words it shares.
fn close_candidate_leads(term_count: usize, rows: &[ResolverCandidateRow]) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    let shared = term_count.saturating_sub(first.missing_terms.len());
    !first.polarity_mismatch
        && shared >= MIN_WORDS_TO_ANSWER_CLOSE
        && rows
            .get(1)
            .is_none_or(|next| closeness(first) < closeness(next))
}

/// Takes the match of another record of the same decision onto the one shown: the best rank, the
/// fields either matched, only the words neither record lacks, and the opposite polarity only when
/// neither record says what was asked.
fn absorb_record(shown: &mut ResolverCandidateRow, other: ResolverCandidateRow) {
    shown.rank = shown.rank.min(other.rank);
    shown.polarity_mismatch &= other.polarity_mismatch;
    for field in other.matched_fields {
        if !shown.matched_fields.contains(&field) {
            shown.matched_fields.push(field);
        }
    }
    shown
        .missing_terms
        .retain(|term| other.missing_terms.contains(term));
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
                polarity_mismatch: false,
                also_recorded_as: Vec::new(),
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

/// Says, beside `data` in a read verb's JSON envelope, how a description was resolved when that
/// is worth knowing: `close_match: {decision_id, title, missing_terms}` for a decision that
/// matched all but some of the words asked, and `also_recorded_as: [{decision_id, title}]` for
/// the other records of a decision recorded more than once. A description that matched in full
/// a decision recorded once adds nothing, so a key's presence is the whole signal. Shared by the
/// CLI's `--json`, the HTTP API and MCP so the three cannot name it differently.
pub fn annotate_resolution(envelope: &mut serde_json::Value, candidate: &ResolvedCandidate) {
    let Some(envelope) = envelope.as_object_mut() else {
        return;
    };
    if !candidate.missing_terms.is_empty() {
        envelope.insert(
            "close_match".to_owned(),
            serde_json::json!({
                "decision_id": candidate.decision_id,
                "title": candidate.title,
                "missing_terms": candidate.missing_terms,
            }),
        );
    }
    if !candidate.also_recorded_as.is_empty() {
        envelope.insert(
            "also_recorded_as".to_owned(),
            serde_json::json!(candidate.also_recorded_as),
        );
    }
}

#[cfg(test)]
mod tests;
