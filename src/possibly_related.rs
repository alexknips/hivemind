//! Possibly related decisions: an inferred suggestion beside the graph, never a recorded relation
//! (hivemind-xarm).
//!
//! The graph holds the relations someone recorded. Most decisions have none to another decision,
//! yet every capture carries the topic keys it was filed under and, for a classifier capture, the
//! capture session it was recorded out of. [`possibly_related`] turns those two facts into a
//! short, ranked list per decision, labelled as inferred, so a reader sees where to look without
//! anything claiming that two decisions depend on one another.
//!
//! # What is inferred, and from what
//! 1. **Same conversation.** Decisions the classifier recorded out of one capture session were
//!    extracted from one conversation, whether it recorded that session in one
//!    `ingest.batch_classified` event or in several (the session is the one that shipped the
//!    batches the event names: `CaptureFact::session_ids`). When no session is known for a
//!    decision, one classification event recording both (the same `event_origin`) stands in.
//!    Ranked first.
//! 2. **A shared specific topic key.** A topic key shared by the two decisions, counted only when
//!    it is *specific*, ranked by how few decisions carry it.
//!
//! A topic key is **generic** when more than [`generic_key_cutoff`] decisions carry it: an area
//! tag ("refinery", "ci") says which part of the work a decision came from, not that two
//! decisions inform each other, and linking on it draws one hairball. A generic key links
//! nothing; the keys of the asked decision that were set aside are listed in
//! [`PossiblyRelated::ignored_topic_keys`], so the filter is never silent. The cutoff is
//! `max(10, 2% of the titled decisions)`: a fixed floor for a small ledger, a share of the ledger
//! once it grows, so "specific" keeps meaning "names a handful of decisions".
//!
//! # Ranking, and why it is traceable
//! Same-conversation decisions first, then by the sum over shared specific keys of
//! `1 / (carried_by - 1)` (a key only two decisions carry counts 1, one twelve carry counts
//! 1/11), then by decision id. Every number that goes into the sum is in the answer
//! ([`RelatedDecision::shared_topic_keys`] with each key's `carried_by`), so the order can be
//! recomputed from the response alone. There is no score in the response and no model behind it.
//!
//! # What is left out
//! The decision itself, and every decision already joined to it by a recorded relation
//! (`SUPERSEDES`, `FOLLOWS_FROM`, `SAME_AS`): those are recorded, shown as such, and not
//! "possibly" anything. A decision with no title (a node only referenced by a request or a
//! blocker) is not a candidate. A relation two decisions share through a third node (the same
//! evidence or question) is not looked for here.
//!
//! # Limits stated up front
//! A decision recorded by hand (a `decision.proposed`) belongs to no capture session and has an
//! event of its own, so its list rests on topic keys alone. A session is the `session_id` of the
//! received batches a classification names, so a classification naming batches the ledger never
//! received (a `hivemind emit` capture mints a fresh batch id) names none and is grouped by its
//! event alone.
//!
//! # Placement
//! Layer 3 (`ARCHITECTURE.md` → Layer Boundary): ranking and inference live here, outside
//! `queries/` and `commands/`, which never import this module. It reads the graph through
//! `queries` facts only: no model, no network, no write. The session it reads is a property the
//! projection puts on a captured decision, so a ledger projected before that property existed
//! has to be projected again to carry it.
//!
//! # Cost
//! A fixed number of bulk reads (every titled decision once, three relation scans, the `SAME_AS`
//! scan and the three status scans) plus the work of ranking; a page never costs more reads, only
//! more rows.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;

use crate::error::QueryError;
use crate::projector::GraphView;
use crate::queries::{
    decision_capture_facts, recorded_decision_links, CaptureFact, DecisionStandings,
    DecisionStatus, QueryResponse,
};
use crate::Result;

/// Items per page when a caller does not say.
pub const DEFAULT_LIMIT: usize = 25;

/// The most items one page holds, whatever a caller asks for.
pub const MAX_LIMIT: usize = 200;

/// A topic key carried by no more than this many decisions is always specific.
const GENERIC_KEY_FLOOR: usize = 10;

/// Once the ledger holds more than `GENERIC_KEY_SHARE_DIVISOR × GENERIC_KEY_FLOOR` titled
/// decisions, a key is generic when more than `1 / GENERIC_KEY_SHARE_DIVISOR` of them carry it.
const GENERIC_KEY_SHARE_DIVISOR: usize = 50;

/// What a response says about itself: the three-layer rule, in the answer, where a UI cannot
/// drop it by accident.
pub const INFERRED_NOTE: &str = "Possibly related, inferred: these decisions were recorded out of the same conversation or share a specific topic key. No one recorded a relation between them.";

/// The layer a response belongs to: the graph's recorded edges are `recorded`; this is `inferred`.
pub const INFERRED_LAYER: &str = "inferred";

/// Most decisions a topic key may be carried by and still be specific, in a ledger of
/// `titled_decisions`.
pub const fn generic_key_cutoff(titled_decisions: usize) -> usize {
    let share = titled_decisions / GENERIC_KEY_SHARE_DIVISOR;
    if share > GENERIC_KEY_FLOOR {
        share
    } else {
        GENERIC_KEY_FLOOR
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PossiblyRelatedRequest {
    /// Items per page; `0` means [`DEFAULT_LIMIT`], and more than [`MAX_LIMIT`] is cut to it.
    pub limit: usize,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

/// A topic key and how many decisions carry it: the number the ranking and the filter use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopicReach {
    pub key: String,
    pub carried_by: usize,
}

/// One decision that may be related to the asked one, and the facts that say why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RelatedDecision {
    pub decision_id: String,
    pub title: String,
    pub status: DecisionStatus,
    /// Recorded out of the same conversation as the asked decision: the same capture session, or,
    /// when no session is known for them, the same classification event.
    pub same_conversation: bool,
    /// The specific topic keys both decisions carry, sorted by key.
    pub shared_topic_keys: Vec<TopicReach>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PossiblyRelated {
    pub decision_id: String,
    /// Always [`INFERRED_LAYER`].
    pub layer: &'static str,
    /// Always [`INFERRED_NOTE`].
    pub note: &'static str,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    /// The asked decision's topic keys that too many decisions carry to link any of them.
    pub ignored_topic_keys: Vec<TopicReach>,
    pub items: Vec<RelatedDecision>,
}

/// One candidate before the page is cut: the facts the order rests on.
struct Candidate {
    decision_id: String,
    title: String,
    same_conversation: bool,
    shared: Vec<TopicReach>,
    weight: f64,
}

/// Whether two decisions were recorded out of one conversation: they share a capture session, or
/// one classification event recorded both. An event's decisions share the event's sessions, so the
/// event test only adds the decisions whose batch ids name no session.
fn from_one_conversation(asked: &CaptureFact, other: &CaptureFact) -> bool {
    let shares_session = asked
        .session_ids
        .iter()
        .any(|session| other.session_ids.contains(session));
    shares_session || (asked.event_origin.is_some() && other.event_origin == asked.event_origin)
}

/// The decisions possibly related to `decision_id`, best first; `data` is `None` when there is no
/// such titled decision.
pub fn possibly_related(
    graph: &impl GraphView,
    decision_id: &str,
    request: &PossiblyRelatedRequest,
) -> Result<QueryResponse<Option<PossiblyRelated>>> {
    // ubs:ignore: Instant measures response latency only; it does not generate secrets.
    let started = Instant::now();
    let decision_id = decision_id.trim();
    if decision_id.is_empty() {
        return Err(QueryError::Execution("decision id must not be empty".to_owned()).into());
    }
    let limit = match request.limit {
        0 => DEFAULT_LIMIT,
        limit => limit.min(MAX_LIMIT),
    };
    let cursor = request
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_owned);
    let offset = match cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor.parse::<usize>().map_err(|error| {
            QueryError::Execution(format!("cursor must be a non-negative offset: {error}"))
        })?,
    };

    let facts = decision_capture_facts(graph)?;
    let Some(asked) = facts.iter().find(|fact| fact.decision_id == decision_id) else {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: None,
        });
    };

    // How many decisions carry each topic key (a decision counts once for a key it repeats).
    let mut carried_by: BTreeMap<&str, usize> = BTreeMap::new();
    for fact in &facts {
        let keys: BTreeSet<&str> = fact.topic_keys.iter().map(String::as_str).collect();
        for key in keys {
            *carried_by.entry(key).or_default() += 1;
        }
    }
    let cutoff = generic_key_cutoff(facts.len());
    let reach_of = |key: &str| TopicReach {
        key: key.to_owned(),
        carried_by: carried_by.get(key).copied().unwrap_or(0),
    };

    let asked_keys: BTreeSet<&str> = asked.topic_keys.iter().map(String::as_str).collect();
    let (specific_keys, generic_keys): (BTreeSet<&str>, BTreeSet<&str>) = asked_keys
        .into_iter()
        .partition(|key| carried_by.get(key).copied().unwrap_or(0) <= cutoff);
    let ignored_topic_keys: Vec<TopicReach> = generic_keys.into_iter().map(reach_of).collect();

    let recorded = recorded_decision_links(graph, decision_id)?;
    let mut candidates: Vec<Candidate> = Vec::new();
    for fact in &facts {
        if fact.decision_id == asked.decision_id || recorded.contains(&fact.decision_id) {
            continue;
        }
        let same_conversation = from_one_conversation(asked, fact);
        let own_keys: BTreeSet<&str> = fact.topic_keys.iter().map(String::as_str).collect();
        let shared: Vec<TopicReach> = specific_keys
            .intersection(&own_keys)
            .map(|key| reach_of(key))
            .collect();
        if !same_conversation && shared.is_empty() {
            continue;
        }
        let weight: f64 = shared
            .iter()
            .map(|reach| 1.0 / reach.carried_by.saturating_sub(1).max(1) as f64)
            .sum();
        candidates.push(Candidate {
            decision_id: fact.decision_id.clone(),
            title: fact.title.clone(),
            same_conversation,
            shared,
            weight,
        });
    }
    candidates.sort_by(|left, right| {
        right
            .same_conversation
            .cmp(&left.same_conversation)
            .then_with(|| right.weight.total_cmp(&left.weight))
            .then_with(|| left.decision_id.cmp(&right.decision_id))
    });

    let total_matches = candidates.len();
    let page: Vec<Candidate> = candidates.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(page.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    let standings = if page.is_empty() {
        None
    } else {
        Some(DecisionStandings::load(graph)?)
    };
    let items: Vec<RelatedDecision> = page
        .into_iter()
        .map(|candidate| RelatedDecision {
            status: standings
                .as_ref()
                .map_or(DecisionStatus::Proposed, |standings| {
                    standings.status_of(&candidate.decision_id)
                }),
            decision_id: candidate.decision_id,
            title: candidate.title,
            same_conversation: candidate.same_conversation,
            shared_topic_keys: candidate.shared,
        })
        .collect();

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: Some(PossiblyRelated {
            decision_id: asked.decision_id.clone(),
            layer: INFERRED_LAYER,
            note: INFERRED_NOTE,
            limit,
            cursor,
            next_cursor,
            total_matches,
            ignored_topic_keys,
            items,
        }),
    })
}

#[cfg(test)]
mod tests;
