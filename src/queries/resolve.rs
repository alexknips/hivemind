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
//! The gate for a description that matches in full is deliberately conservative. A verb that reads
//! resolves only when exactly one candidate occupies the best rank tier, and, among several that
//! hold every word, when it is about the question (see below). A verb that writes resolves only
//! when one decision alone holds every word, or the question names one outright (its id or title
//! is the question, or it records the question as asked): where the words sit orders its list but
//! never picks for it, since a rejection, a new title or a new premise cannot be taken back
//! (hivemind-293q).
//!
//! A description that names a word no decision contains ("why did we choose to move the demo
//! cell..." when the record never says "choose") is not a dead end: when no decision matches
//! every term, decisions matching most of them come back as an `Ambiguous` candidate list, each
//! carrying the terms it lacks. A verb that writes never auto-resolves a close candidate. A verb
//! that only reads (`why`, `verify`, ...) asks with `resolve_decision_for_reading`: it matches at
//! the bar `recall` uses, and answers with a close candidate that leads alone, saying what the
//! decision lacks, instead of making the asker repeat the question with `--pick`. Which candidate
//! leads is decided first by how much of the question its title, topic keys and recorded question
//! carry (`About`), then by how many terms it lacks (see `closeness`), so a decision about the
//! thing asked about beats one that only mentions the words somewhere in a long rationale, however
//! few words that one lacks. And a close candidate is answered with only when its own title and
//! topic keys hold at least `MIN_HEADLINE_WORDS_TO_ANSWER_CLOSE` of the question's words: one
//! shared word is what any decision on a broad subject has, and a decision that lacks the very
//! word the question is about is not the decision it asks about; it is listed.
//!
//! Holding every word is not being about the question, either: a decision whose long rationale
//! says all of a plain question's words matches in full, and a full match drops every close
//! candidate, so the decision whose title and topic keys carry the question but lacks one word
//! was never offered. A close candidate whose title and topic keys carry more of the question
//! (`About`) than every full match is promoted (`out_heading`): listed first, and answered with by
//! a verb that only reads, saying what it lacks. Among full matches, the one that is more about
//! the question comes first. It is answered with alone only when it is about the question in its
//! own right: with other decisions holding every word too, its own title and topic keys must hold
//! more than half of the question's words, or the full matches are listed (`full_match_leads`). A
//! leader that out-weighs the rest by one word in its title, with the others only in a long
//! rationale, is not the decision asked about because it leads by that word. A verb that writes
//! is answered with such a leader never: with other decisions holding every word, it gets the
//! list.
//!
//! A negation in the description ("don't adopt Kafka", "why didn't we ...", "do not ...") is not a
//! word to find: it never appears in `missing_terms`. It is polarity. A decision is the opposite of
//! a negated description when its own title says every word asked about outright, outside any
//! negation ("Adopt Kafka" for "why didn't we adopt Kafka"): it is a close candidate whose reason
//! is `POLARITY_REASON`, and is never resolved to, by a verb that writes or one that reads. A
//! title that leaves a word out, or denies it, is not the opposite of anything: it competes like
//! any other, on how much of the question it carries, so a negated question is not answered by
//! whatever decision happens to say "no" somewhere, nor sent past the decision asked about because
//! that decision states its answer positively.

use std::cmp::Reverse;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::same_as::{fold_linked, RecordedCopy};
use super::search::{
    collect_resolver_candidates, out_heading, CloseMatch, ResolverCandidateRow, Standing,
};
use super::shared::{
    optional_int, optional_string, query_error, query_timer_start, MAX_QUERY_RESULTS,
};
use super::terms::resolver_terms;
use super::QueryResponse;

/// Fewest description words a close candidate must share before a read verb answers with it
/// instead of listing it. Listing needs only half the words (as `recall` does); answering names
/// the decision as the one asked about, and one shared word out of two is too little for that.
const MIN_WORDS_TO_ANSWER_CLOSE: usize = 2;

/// Fewest of the question's words a close candidate's own title and topic keys must hold, as
/// words, before a read verb answers with it. A decision that lacks some of the words and holds
/// fewer than this in its headline has the rest only in a long rationale, its evidence, the
/// question it records or its recorder's fields, or lacks the one word the question is about: it
/// is listed, never named as the decision asked about. The question a decision records orders it
/// (`About`) but does not count here: it can be as long as a paragraph, and a decision with a
/// long one holds nearly every common word as a word.
const MIN_HEADLINE_WORDS_TO_ANSWER_CLOSE: usize = 2;

/// Why a decision that has every word asked is still only a close candidate: the description is
/// negated and the decision's title says all of it outright. Shown beside the candidate wherever
/// `missing_terms` would be, so a reader sees the opposite polarity instead of being told it lacks
/// "not".
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
    /// Reused tier from `evaluate_search_match`; 0 = exact id, title or recorded-question match, lower is better.
    pub rank: u8,
    /// Ledger offset at creation — recency tiebreak, never a ranking input on its own.
    pub event_origin: i64,
    pub matched_fields: Vec<String>,
    /// Description terms this decision does not contain. Empty for a full match; non-empty only
    /// for the close candidates offered when no decision matches every term.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_terms: Vec<String>,
    /// The description is negated and this decision's title says every word of it outright
    /// (`POLARITY_REASON`). Such a decision is a close candidate even with `missing_terms` empty,
    /// and is never `Resolved`.
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
/// Resolved iff exactly one decision matches every term, or the description names one outright
/// (its id or title is the description, or it records the description as asked): with several
/// that hold every word, where the words sit orders the list but never picks, so the call writes
/// nothing and lists them (hivemind-293q). There is no numeric recency margin (mayor decision,
/// hivemind-tenv.1, 2026-09-07). Close candidates (missing some terms) are offered, as
/// `Ambiguous`, only when there is no full match, and never resolved: more than half of the
/// terms, and at least two.
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
/// - when close candidates are all there is, and the one that is most about the question is also
///   closer than the rest (lacks fewer terms, or lacks as many but has more of the terms it
///   matched in its title or topic keys), shares at least two terms and holds at least two of them
///   in its own title or topic keys, it is `Resolved` with its `missing_terms` set, so the caller
///   shows the decision and names what it lacks. Equally close candidates, and a candidate whose
///   headline holds fewer than two of the words, stay an `Ambiguous` list (hivemind-tfde).
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
    // A verb that only shows a decision reads the description as `recall` does, so it names what
    // `recall` just named: a word's other forms and the words that stand in for it. A verb that
    // writes matches a word and its inflections only; a synonym never picks what it writes to.
    let related = asker == Asker::Reader;
    // Records of one decision, linked `SAME_AS`, are one candidate; records with no link between
    // them stay separate, so a tie among them stays `Ambiguous`.
    let mut also_recorded_as: HashMap<String, Vec<RecordedCopy>> = HashMap::new();
    let mut rows: Vec<ResolverCandidateRow> = Vec::new();
    for folded in fold_linked(
        graph,
        collect_resolver_candidates(graph, description, &topic_keys, close, related)?,
        |row| row.decision_id.as_str(),
        absorb_record,
    )? {
        if !folded.also_recorded_as.is_empty() {
            also_recorded_as.insert(folded.item.decision_id.clone(), folded.also_recorded_as);
        }
        rows.push(folded.item);
    }
    let only_close_candidates = !rows.is_empty() && rows.iter().all(ResolverCandidateRow::is_close);
    let standings: Vec<Standing> = rows
        .iter()
        .map(|row| Standing {
            missing: row.missing_terms.len(),
            opposite: row.polarity_mismatch,
            about: row.about,
        })
        .collect();
    for (row, promoted) in rows.iter_mut().zip(out_heading(&standings)) {
        row.promoted = promoted;
    }
    if !only_close_candidates {
        rows.retain(|row| row.promoted || !row.is_close());
    }
    rows.sort_by(|left, right| {
        right
            .promoted
            .cmp(&left.promoted)
            .then_with(|| {
                right
                    .missing_terms
                    .is_empty()
                    .cmp(&left.missing_terms.is_empty())
            })
            .then_with(|| right.about.cmp_about(&left.about))
            .then_with(|| left.missing_terms.len().cmp(&right.missing_terms.len()))
            .then_with(|| right.headline_terms.cmp(&left.headline_terms))
            .then_with(|| left.stand_in_terms.cmp(&right.stand_in_terms))
            .then_with(|| left.rank.cmp(&right.rank))
            .then_with(|| right.event_origin.cmp(&left.event_origin))
            .then_with(|| left.decision_id.cmp(&right.decision_id))
    });

    let truncated = rows.len() > MAX_QUERY_RESULTS;
    rows.truncate(MAX_QUERY_RESULTS);

    // The leader's ties among the full matches: the rows that hold as much of what the question
    // is about, lean on no more stand-in words and sit in the same rank tier.
    let leader_ties = rows.first().map_or(0, |leader| {
        rows.iter()
            .filter(|row| {
                row.about.cmp_about(&leader.about).is_eq()
                    && row.stand_in_terms == leader.stand_in_terms
                    && row.rank == leader.rank
            })
            .count()
    });
    let term_count = resolver_terms(description).len();
    let leader_is_clear = asker == Asker::Reader && close_candidate_leads(term_count, &rows);
    let full_leader_is_about_it = full_match_leads(asker, term_count, &rows);

    let leader_promoted = rows.first().is_some_and(|leader| leader.promoted);
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
    let outcome = if only_close_candidates || leader_promoted {
        if leader_is_clear {
            ResolveOutcome::Resolved {
                candidate: candidates.remove(0),
            }
        } else {
            ResolveOutcome::Ambiguous { candidates }
        }
    } else if !candidates.is_empty() {
        if leader_ties == 1 && full_leader_is_about_it {
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
/// of the terms it matched, then the one that matched fewer of them only through a stand-in word
/// (a decision that says "agent" is closer to a question about an agent than one that says "bot").
/// A decision about the product's name says "product" and "Upheld" in its title; one that happens
/// to say "product" and "called" somewhere in a long rationale is not about that, though it lacks
/// just as many words. Where the words matched is already in `matched_fields`; nothing is counted
/// across the ledger and nothing is learned. A full match is 0 on the headline count (its order
/// is `About`, then the stand-in count and the rank tier), and a verb that writes never reads
/// stand-ins, so it is 0 on the last one too.
///
/// This is how close the candidate is, which says whether the first of a list leads it clearly.
/// The order of the list is decided before it: by `About`, so a decision whose title says the
/// question comes ahead of one that lacks a word fewer and holds the others only in its rationale.
fn closeness(row: &ResolverCandidateRow) -> (usize, Reverse<usize>, usize) {
    (
        row.missing_terms.len(),
        Reverse(row.headline_terms),
        row.stand_in_terms,
    )
}

/// Whether the first of `rows` (a close candidate, the most about the question first) is the one
/// asked about: its title and topic keys hold enough of the question's words
/// (`MIN_HEADLINE_WORDS_TO_ANSWER_CLOSE`), it shares enough terms to be named as the answer, and it
/// is closer than the next, by `closeness`, or the next is a decision it was promoted over
/// (`out_heading`). A first that is the most about the question but lacks more words than the next
/// is not resolved, and neither is an equally close one: after it the lists are ordered by rank
/// and recency, which say nothing about which of two decisions was meant. A decision of the
/// opposite polarity is never the one asked about, however many words it shares.
fn close_candidate_leads(term_count: usize, rows: &[ResolverCandidateRow]) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    let shared = term_count.saturating_sub(first.missing_terms.len());
    !first.polarity_mismatch
        && shared >= MIN_WORDS_TO_ANSWER_CLOSE
        && first.headline_words >= MIN_HEADLINE_WORDS_TO_ANSWER_CLOSE
        && rows.get(1).is_none_or(|next| {
            (first.promoted && !next.promoted) || closeness(first) < closeness(next)
        })
}

/// Whether the first of `rows` (full matches, the most about the question first) is the one asked
/// about, so that a call answers with it alone. When other decisions hold every word too, a leader
/// that out-weighs them (`About`) can do it by one word in its title or topic keys, with no more
/// than half of the question there and the rest only in a long rationale: it is not thereby the
/// decision asked about, and naming it hides the others behind a confident answer (hivemind-5ctc).
/// So it is answered with only when its own title and topic keys hold more than half of the
/// question's words; otherwise the full matches are listed, most about the question first. The
/// only decision that holds every word has none to be listed among, and the decision the question
/// names outright (its id or title is the question, or it records the question as asked) is the
/// one asked about whatever its words: both are answered with as before. A close candidate has
/// its own gate (`close_candidate_leads`).
///
/// A verb that writes is held to a stricter gate: where the words sit orders its list but never
/// picks for it (hivemind-293q). A wrong answer for a verb that shows is a wrong decision shown;
/// for one that writes it is a rejection, a new title or a new premise on the record of a decision
/// nobody meant, which append-only storage cannot take back, and the help of every such verb says
/// a description that matches more than one decision lists the candidates and writes nothing. So
/// with other decisions holding every word too, a writer is answered with the leader only when
/// the question names it outright; the only decision that holds every word is picked as before.
fn full_match_leads(asker: Asker, term_count: usize, rows: &[ResolverCandidateRow]) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    match asker {
        Asker::Writer => rows.len() == 1 || first.about.is_named(),
        Asker::Reader => {
            rows.len() == 1 || first.about.is_named() || first.headline_words * 2 > term_count
        }
    }
}

/// Takes the match of another record of the same decision onto the one shown: the best rank, the
/// fields either matched, only the words neither record lacks, and the opposite polarity only when
/// neither record says what was asked.
fn absorb_record(shown: &mut ResolverCandidateRow, other: ResolverCandidateRow) {
    shown.rank = shown.rank.min(other.rank);
    shown.stand_in_terms = shown.stand_in_terms.min(other.stand_in_terms);
    shown.headline_words = shown.headline_words.max(other.headline_words);
    if other.about.cmp_about(&shown.about).is_gt() {
        shown.about = other.about;
    }
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
