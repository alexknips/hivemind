//! The dated story of a decision, and the list of decisions that recently changed
//! (hivemind-bbnw.7).
//!
//! Every entry is a fact the ledger holds, cited by its event offset: when the question was
//! asked, when the decision was recorded, who accepted or rejected it and when, when it was
//! superseded, retitled or moved, and when something it rests on stopped standing. The graph
//! keeps no per-edge timestamps, so the dated facts are read from the ledger events themselves
//! (the same read `get_decisions_changed_since` does), plus the ask and the record from their own
//! graph nodes (a classified capture has no `decision.proposed` event to read).
//!
//! Rules this module keeps (Alex 09-21, qo11):
//! - It reports facts and durations, never a verdict on a person. There are no averages, no
//!   per-person figures and no grading. The one duration is `asked_to_decided_seconds` for one
//!   decision.
//! - Nothing is guessed. A decision nobody explicitly asked has no `asked` entry and no duration;
//!   an event with no timestamp carries `ts: null`. Entries follow the ledger's order, so a
//!   decision imported with its source's own time can show a `ts` earlier than the entry before
//!   it, and both are true.
//!
//! Propagation ("something it rests on stopped standing") is time-correct: it is written at the
//! moment a hypothesis the decision premised on was refuted, or a decision it follows from was
//! superseded or rejected, and only for decisions that already rested on it then. A decision that
//! started resting on a refuted premise later was stale from the start; its `still_holds` says so.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::events::{
    self, DecisionRejectedPayload, DecisionSupersededPayload, Event, EventId, EventPayload,
    EventType, RelationKind as EventRelationKind,
};
use crate::ledger::EventLedger;
use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::history::read_all_events;
use super::project_label::ProjectLabels;
use super::question::{asks_for_question, earliest_ask, question_id_of};
use super::shared::{
    decision_node_exists, neighbor_pairs, node_row, node_rows, normalized_limit, normalized_query,
    optional_datetime, optional_int, optional_string, query_error, query_timer_start, Direction,
};
use super::status::{DecisionStandings, DecisionStatus};
use super::QueryResponse;

/// What one entry of a timeline says happened.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineFact {
    /// The question the decision answers was explicitly asked (`hivemind ask`, `request_decision`).
    Asked {
        request_id: String,
        question_id: String,
    },
    /// The decision was recorded. Its own time is the answer time.
    Recorded,
    Accepted,
    Rejected {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A newer decision replaced this one.
    Superseded {
        by_id: String,
    },
    /// This decision replaced an older one.
    Supersedes {
        replaces_id: String,
    },
    Retitled {
        from: String,
        to: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Moved {
        from: String,
        to: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A hypothesis this decision premised on was refuted by evidence.
    PremiseRefuted {
        hypothesis_id: String,
        evidence_id: String,
    },
    /// A decision this one follows from was superseded.
    PremiseSuperseded {
        decision_id: String,
        by_id: String,
    },
    /// A decision this one follows from was rejected.
    PremiseRejected {
        decision_id: String,
    },
}

impl TimelineFact {
    /// Whether this fact revised the decision or changed whether it still stands: superseded,
    /// retitled, moved, or a premise no longer standing. Asking, recording, accepting and
    /// rejecting are the decision's ordinary life (a rejection shows in `get_contested_decisions`).
    pub fn is_revision(&self) -> bool {
        matches!(
            self,
            Self::Superseded { .. }
                | Self::Retitled { .. }
                | Self::Moved { .. }
                | Self::PremiseRefuted { .. }
                | Self::PremiseSuperseded { .. }
                | Self::PremiseRejected { .. }
        )
    }

    /// Where a fact sorts among entries of the same ledger offset.
    fn rank(&self) -> u8 {
        match self {
            Self::Asked { .. } => 0,
            Self::Recorded => 1,
            _ => 2,
        }
    }
}

/// One dated fact about a decision.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TimelineEntry {
    /// The ledger offset of the event this entry reads; `citation_id` (`event:<offset>`) cites it.
    pub event_origin: EventId,
    pub citation_id: String,
    /// The event's own time; absent when the event carries none.
    pub ts: Option<DateTime<Utc>>,
    /// Who wrote the event; absent only when the graph does not name a recorder.
    pub actor_id: Option<String>,
    #[serde(flatten)]
    pub fact: TimelineFact,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionTimeline {
    pub decision_id: String,
    /// When the question this decision answers was first explicitly asked, if ever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<DateTime<Utc>>,
    /// When the decision was recorded (the same time `why` shows as `answered_at`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
    /// Seconds from the first explicit ask to the decision: a fact about this one decision, and
    /// absent when it was never asked or the ask came after it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_to_decided_seconds: Option<i64>,
    /// Oldest first, in ledger order.
    pub entries: Vec<TimelineEntry>,
}

/// The dated story of one decision, or `None` when the decision does not exist. Reads the whole
/// ledger once (as `get_decisions_changed_since` does) for the events, and the graph for the ask
/// and the record. Read-only.
pub fn get_decision_timeline(
    graph: &impl GraphView,
    ledger: &impl EventLedger,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionTimeline>>> {
    let started = query_timer_start();
    if !decision_node_exists(graph, decision_id)? {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: None,
        });
    }

    let events = read_all_events(ledger)?;
    let mut entries: Vec<TimelineEntry> = ledger_facts(&events)?
        .into_iter()
        .filter(|fact| fact.decision_id == decision_id)
        .map(|fact| fact.entry)
        .collect();
    entries.extend(graph_entries(graph, decision_id)?);
    entries.sort_by_key(|entry| (entry.event_origin, entry.fact.rank()));

    let asked_at = entries
        .iter()
        .filter(|entry| matches!(entry.fact, TimelineFact::Asked { .. }))
        .filter_map(|entry| entry.ts)
        .min();
    let decided_at = entries
        .iter()
        .find(|entry| entry.fact == TimelineFact::Recorded)
        .and_then(|entry| entry.ts);
    let asked_to_decided_seconds = asked_at
        .zip(decided_at)
        .filter(|(asked, decided)| asked <= decided)
        .map(|(asked, decided)| (decided - asked).num_seconds());

    let timeline = DecisionTimeline {
        decision_id: decision_id.to_owned(),
        asked_at,
        decided_at,
        asked_to_decided_seconds,
        entries,
    };
    Ok(QueryResponse {
        result_count: 1,
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: Some(timeline),
    })
}

/// The asks of the decision's question and the decision's own record, from their graph nodes.
fn graph_entries(graph: &impl GraphView, decision_id: &str) -> Result<Vec<TimelineEntry>> {
    let mut entries = Vec::new();

    if let Some(question_id) = question_id_of(graph, decision_id)? {
        for ask in asks_for_question(graph, &question_id)? {
            let Some(origin) = ask
                .event_origin
                .and_then(|origin| EventId::try_from(origin).ok())
            else {
                continue;
            };
            entries.push(TimelineEntry {
                event_origin: origin,
                citation_id: format!("event:{origin}"),
                ts: Some(ask.asked_at),
                actor_id: ask.requested_by,
                fact: TimelineFact::Asked {
                    request_id: ask.request_id,
                    question_id: question_id.clone(), // ubs:ignore: one clone per ask of one question
                },
            });
        }
    }

    if let Some(row) = node_row(graph, NodeKind::Decision, decision_id)? {
        let origin = optional_int(&row, "event_origin").and_then(|o| EventId::try_from(o).ok());
        if let Some(origin) = origin {
            let recorder = neighbor_pairs(
                graph,
                NodeKind::Decision,
                decision_id,
                RelationKind::ProposedBy,
                NodeKind::Actor,
                Direction::Outgoing,
            )?
            .into_iter()
            .next()
            .map(|(actor_id, _)| actor_id);
            entries.push(TimelineEntry {
                event_origin: origin,
                citation_id: format!("event:{origin}"),
                ts: optional_datetime(&row, "occurred_at")?,
                actor_id: recorder,
                fact: TimelineFact::Recorded,
            });
        }
    }
    Ok(entries)
}

/// One ledger fact about one decision.
struct LedgerFact {
    decision_id: String,
    entry: TimelineEntry,
}

/// What each decision already rests on as the ledger is read in order, so propagation reaches
/// only the decisions that rested on a premise when it stopped standing. The same links
/// `get_recent_activity` follows for its stale-premise rows (a proposal's hypotheses, an
/// `ASSUMES` relation from an option or the decision, a `FOLLOWS_FROM` relation), but as of each
/// event rather than as of the end of the ledger.
#[derive(Default)]
struct Premises {
    /// Hypothesis id -> decisions that premised on it so far.
    assumers: BTreeMap<String, BTreeSet<String>>,
    /// Option id -> the decision that chose it, so an option's assumption counts for its decision.
    chosen_by: BTreeMap<String, String>,
    /// Premise decision id -> decisions that follow from it so far.
    followers: BTreeMap<String, BTreeSet<String>>,
}

impl Premises {
    fn followers_of(&self, premise_decision_id: &str) -> Vec<String> {
        self.followers
            .get(premise_decision_id)
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }
}

/// The event types `ledger_facts` reads; every other payload is skipped unparsed, so a ledger full
/// of ingest batches costs no more than one without.
fn reads_payload(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::DecisionProposed
            | EventType::DecisionAccepted
            | EventType::DecisionRejected
            | EventType::DecisionSuperseded
            | EventType::DecisionRetitled
            | EventType::DecisionMoved
            | EventType::RelationAdded
    )
}

/// Every dated fact the decision events of the ledger hold, in ledger order.
fn ledger_facts(events: &[Event]) -> Result<Vec<LedgerFact>> {
    let mut facts = Vec::new();
    let mut premises = Premises::default();

    for event in events {
        if !reads_payload(event.event_type) {
            continue;
        }
        let origin = event
            .event_id
            .ok_or_else(|| query_error("event_id is required for timeline queries"))?;
        let payload = events::validate(event)
            .map_err(|error| query_error(format!("invalid event {origin}: {error}")))?;

        let mut push = |decision_id: &str, fact: TimelineFact| {
            facts.push(LedgerFact {
                decision_id: decision_id.to_owned(),
                entry: TimelineEntry {
                    event_origin: origin,
                    citation_id: format!("event:{origin}"),
                    ts: event.ts,
                    actor_id: Some(event.actor_id.clone()), // ubs:ignore: the entry owns its actor id
                    fact,
                },
            });
        };

        match payload {
            EventPayload::DecisionProposed(proposed) => {
                for hypothesis_id in proposed.hypothesis_ids {
                    premises
                        .assumers
                        .entry(hypothesis_id)
                        .or_default()
                        .insert(proposed.decision_id.clone()); // ubs:ignore: ids are owned by the index
                }
            }
            EventPayload::DecisionAccepted(accepted) => {
                push(&accepted.decision_id, TimelineFact::Accepted);
            }
            EventPayload::DecisionRejected(DecisionRejectedPayload {
                decision_id,
                reason,
            }) => {
                for follower in premises.followers_of(&decision_id) {
                    if follower != decision_id {
                        push(
                            &follower,
                            TimelineFact::PremiseRejected {
                                decision_id: decision_id.clone(), // ubs:ignore: one per follower
                            },
                        );
                    }
                }
                push(&decision_id, TimelineFact::Rejected { reason });
            }
            EventPayload::DecisionSuperseded(DecisionSupersededPayload {
                old_decision_id,
                new_decision_id,
            }) => {
                for follower in premises.followers_of(&old_decision_id) {
                    if follower != old_decision_id && follower != new_decision_id {
                        push(
                            &follower,
                            TimelineFact::PremiseSuperseded {
                                decision_id: old_decision_id.clone(), // ubs:ignore: one per follower
                                by_id: new_decision_id.clone(), // ubs:ignore: one per follower
                            },
                        );
                    }
                }
                push(
                    &old_decision_id,
                    TimelineFact::Superseded {
                        by_id: new_decision_id.clone(), // ubs:ignore: once per event
                    },
                );
                push(
                    &new_decision_id,
                    TimelineFact::Supersedes {
                        replaces_id: old_decision_id,
                    },
                );
            }
            EventPayload::DecisionRetitled(retitled) => push(
                &retitled.decision_id,
                TimelineFact::Retitled {
                    from: retitled.from,
                    to: retitled.to,
                    reason: retitled.reason,
                },
            ),
            EventPayload::DecisionMoved(moved) => push(
                &moved.decision_id,
                TimelineFact::Moved {
                    from: moved.from,
                    to: moved.to,
                    reason: moved.reason,
                },
            ),
            EventPayload::RelationAdded(relation) => match relation.relation {
                EventRelationKind::Assumes => {
                    let decision_id = premises
                        .chosen_by
                        .get(&relation.from_id)
                        .cloned()
                        .unwrap_or(relation.from_id);
                    premises
                        .assumers
                        .entry(relation.to_id)
                        .or_default()
                        .insert(decision_id);
                }
                EventRelationKind::Chose => {
                    premises.chosen_by.insert(relation.to_id, relation.from_id);
                }
                EventRelationKind::FollowsFrom => {
                    premises
                        .followers
                        .entry(relation.to_id)
                        .or_default()
                        .insert(relation.from_id);
                }
                EventRelationKind::Refutes => {
                    let assumers: Vec<String> = premises
                        .assumers
                        .get(&relation.to_id)
                        .into_iter()
                        .flatten()
                        .cloned()
                        .collect();
                    for decision_id in assumers {
                        push(
                            &decision_id,
                            TimelineFact::PremiseRefuted {
                                hypothesis_id: relation.to_id.clone(), // ubs:ignore: one per assumer
                                evidence_id: relation.from_id.clone(), // ubs:ignore: one per assumer
                            },
                        );
                    }
                }
                EventRelationKind::BasedOn
                | EventRelationKind::HasOption
                | EventRelationKind::Supports
                | EventRelationKind::SameAs
                | EventRelationKind::Answers => {}
            },
            _ => {}
        }
    }
    Ok(facts)
}

// ---------------------------------------------------------------------------
// Changed decisions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangedDecisionsRequest {
    /// Only changes at or after this time. `None` reads the whole ledger.
    pub since: Option<DateTime<Utc>>,
    /// Only changes at or before this time.
    pub until: Option<DateTime<Utc>>,
    pub limit: usize,
    /// The `next_cursor` of the previous page: `<ledger offset>:<decision id>` of the last item
    /// it showed.
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChangedDecisionsResults {
    /// The window asked for, echoed so a caller sees which changes the list covers.
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: usize,
    pub cursor: Option<String>,
    /// Set when more decisions changed in the window; pass it back as `cursor`.
    pub next_cursor: Option<String>,
    /// Every decision that changed in the window, however many pages that takes.
    pub total_matches: usize,
    pub items: Vec<ChangedDecisionView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChangedDecisionView {
    pub decision_id: String,
    /// The decision's title now.
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub project: Option<String>,
    pub project_label: String,
    /// Where the decision stands now.
    pub status: DecisionStatus,
    /// When the question this decision answers was first explicitly asked, if ever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<DateTime<Utc>>,
    /// When the decision was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
    /// The time of the latest change in the window; absent when it carries none.
    pub last_changed_at: Option<DateTime<Utc>>,
    /// The changes in the window, oldest first: superseded, retitled, moved, or a premise no
    /// longer standing. Each is cited by its ledger offset.
    pub changes: Vec<TimelineEntry>,
}

/// Decisions that were revised or superseded, or whose premise stopped standing, inside the
/// window, most recently changed first, one page at a time. Facts only: what changed, when, by
/// whom and by what (the replacing decision, the refuting evidence); no judgement of whether the
/// change was good. An event with no timestamp falls inside every window, undated, so nothing
/// hides.
pub fn get_changed_decisions(
    graph: &impl GraphView,
    ledger: &impl EventLedger,
    request: &ChangedDecisionsRequest,
) -> Result<QueryResponse<ChangedDecisionsResults>> {
    let started = query_timer_start();
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let after = cursor.as_deref().map(parse_changed_cursor).transpose()?;

    let events = read_all_events(ledger)?;
    let mut changes: BTreeMap<String, Vec<TimelineEntry>> = BTreeMap::new();
    for fact in ledger_facts(&events)? {
        if !fact.entry.fact.is_revision() || !in_window(fact.entry.ts, request) {
            continue;
        }
        changes
            .entry(fact.decision_id)
            .or_default()
            .push(fact.entry);
    }

    let decisions = node_rows(graph, NodeKind::Decision)?;
    // Every decision the window touched, newest change first. A change naming a decision the
    // graph has no node for cannot be shown as a decision.
    let mut changed: Vec<(EventId, String, Vec<TimelineEntry>)> = changes
        .into_iter()
        .filter(|(decision_id, _)| decisions.contains_key(decision_id))
        .filter_map(|(decision_id, entries)| {
            let last_origin = entries.last()?.event_origin;
            Some((last_origin, decision_id, entries))
        })
        .collect();
    changed.sort_by(|(left_origin, left_id, _), (right_origin, right_id, _)| {
        (right_origin, left_id).cmp(&(left_origin, right_id))
    });

    let total_matches = changed.len();
    let mut remaining: Vec<(EventId, String, Vec<TimelineEntry>)> = changed
        .into_iter()
        .filter(|(origin, decision_id, _)| {
            after.as_ref().is_none_or(|(after_origin, after_id)| {
                *origin < *after_origin
                    || (origin == after_origin && decision_id.as_str() > after_id.as_str())
            })
        })
        .collect();
    let more = remaining.len() > limit;
    remaining.truncate(limit);
    let next_cursor = more
        .then(|| {
            remaining
                .last()
                .map(|(origin, decision_id, _)| format!("{origin}:{decision_id}"))
        })
        .flatten();

    // Only the page is filled in: the decision's words, project, standing, and ask and record times.
    let standings = DecisionStandings::load(graph)?;
    let labels = ProjectLabels::from_graph(graph)?;
    let mut items = Vec::with_capacity(remaining.len());
    for (_, decision_id, entries) in remaining {
        let Some(row) = decisions.get(&decision_id) else {
            continue;
        };
        let (asked_at, decided_at) = asked_and_decided_at(graph, row)?;
        let project = optional_string(row, "project");
        items.push(ChangedDecisionView {
            title: optional_string(row, "title").unwrap_or_default(),
            slug: optional_string(row, "slug"),
            project_label: labels.label_of(project.as_deref()),
            project,
            status: standings.status_of(&decision_id),
            asked_at,
            decided_at,
            last_changed_at: entries.last().and_then(|entry| entry.ts),
            changes: entries,
            decision_id,
        });
    }

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: ChangedDecisionsResults {
            since: request.since,
            until: request.until,
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

/// `(asked_at, decided_at)` of one decision.
pub(super) type AskedAndDecidedAt = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// When the question a decision answers was first explicitly asked (if ever) and when the
/// decision itself was recorded, read from its own node and the `Ask` nodes of its question.
pub(super) fn asked_and_decided_at(
    graph: &impl GraphView,
    decision_row: &GraphRow,
) -> Result<AskedAndDecidedAt> {
    let decided_at = optional_datetime(decision_row, "occurred_at")?;
    let asked_at = match optional_string(decision_row, "id") {
        Some(decision_id) => match question_id_of(graph, &decision_id)? {
            Some(question_id) => earliest_ask(graph, &question_id)?,
            None => None,
        },
        None => None,
    };
    Ok((asked_at, decided_at))
}

/// A `next_cursor` of the changed list: the list is ordered by the ledger offset of a decision's
/// latest change, newest first, then by decision id, and several decisions can share one offset
/// (a superseded premise and every decision that follows from it), so the offset alone would
/// skip some of them at a page boundary.
fn parse_changed_cursor(cursor: &str) -> Result<(EventId, String)> {
    let malformed = || {
        query_error(format!(
            "cursor must be a `next_cursor` of this list (`<ledger offset>:<decision id>`), got {cursor}"
        ))
    };
    let (origin, decision_id) = cursor.split_once(':').ok_or_else(malformed)?;
    let origin = origin.parse::<EventId>().map_err(|_| malformed())?;
    Ok((origin, decision_id.to_owned()))
}

fn in_window(ts: Option<DateTime<Utc>>, request: &ChangedDecisionsRequest) -> bool {
    let Some(ts) = ts else {
        return true;
    };
    request.since.is_none_or(|since| ts >= since) && request.until.is_none_or(|until| ts <= until)
}

#[cfg(test)]
mod tests;
