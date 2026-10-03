//! The status-changing events of one decision, each with the status it left the decision in
//! (hivemind-fwog).
//!
//! A decision's status is derived, never stored: `status_from_positions` over who accepted it,
//! who rejected it and whether anyone superseded it. This read lists the events that set those
//! positions, newest first, so a reader can say "accepted 23 Sep by Alex, proposed 22 Sep by an
//! agent" and tell the status of that time from the status now. It writes nothing and edits
//! nothing; every entry is a fact the ledger holds, cited by its offset.
//!
//! Two kinds of source, because a position is set in two ways:
//! - A `decision.accepted`, `decision.rejected` or `decision.superseded` event of its own: dated
//!   by the event's own time and attributed to the event's actor, read from the ledger.
//! - A classified capture that states the position in the capture itself (`accepted_by`,
//!   `rejected_by`, `supersedes_id`). The graph holds that edge and no event of its own to date
//!   it. The edge was written by the capture's classification event, whose offset the capturing
//!   decision's node carries, so it is cited by that offset. The decision is dated at the time it
//!   was recorded (the source turn's time when the capture names one, else the batch's), so a
//!   decision proposed and accepted in one capture shows both at the same time. A replacing
//!   decision made this way dates the supersession at its own recorded time and offset.
//!
//! The graph keeps one edge per (decision, actor), so an acceptance the ledger holds as its own
//! event is not listed a second time from the graph.
//!
//! `status_after` is the one rule applied to the entries up to and including that one, so the
//! newest entry's `status_after` is the status now. Nothing is guessed: an event with no timestamp
//! carries `occurred_at: null`.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::events::{self, Event, EventId, EventPayload, EventType, TenantId};
use crate::ledger::EventLedger;
use crate::projector::{actor_kind, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{
    decision_node_exists, neighbor_pairs, node_row, optional_datetime, optional_int, query_error,
    query_timer_start, Direction,
};
use super::status::{derive_decision_status, status_from_positions, DecisionStatus};
use super::QueryResponse;

/// Ledger events read per page when looking for a decision's status events.
const LEDGER_READ_PAGE_SIZE: usize = 1_000;

/// The event types that set a status position on their own.
const STATUS_EVENT_TYPES: &[EventType] = &[
    EventType::DecisionAccepted,
    EventType::DecisionRejected,
    EventType::DecisionSuperseded,
];

/// What a status event did to the decision. Declared in the order the entries of one offset apply.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusEventKind {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
}

/// Who did it, with whether they are a human or an agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatusActor {
    pub id: String,
    /// `human`, `agent` or `unknown`, read from the actor-id prefix: the same rule that sets
    /// `kind` on the Actor node.
    pub kind: &'static str,
}

impl StatusActor {
    fn named(actor_id: String) -> Self {
        Self {
            kind: actor_kind(&actor_id),
            id: actor_id,
        }
    }
}

/// One status-changing event of a decision.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatusEvent {
    pub event: StatusEventKind,
    /// The event's own time; `null` when the event carries none.
    pub occurred_at: Option<DateTime<Utc>>,
    /// The ledger offset (`event_origin`) of the event that did it.
    pub offset: EventId,
    /// Who proposed, accepted, rejected or superseded it. Absent only on a `proposed` entry whose
    /// proposer the graph does not name, and on a `superseded` entry whose replacing decision names
    /// none.
    pub actor: Option<StatusActor>,
    /// On an `accepted` entry: the human whose delegated scope an agent's own acceptance fell
    /// within, when the event says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<String>,
    /// On a `superseded` entry: the newer decision that replaced this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// The status the one rule gives after this entry (and every entry before it).
    pub status_after: DecisionStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionStatusEvents {
    pub decision_id: String,
    /// The status now. Equals the newest entry's `status_after`.
    pub status: DecisionStatus,
    /// Newest first.
    pub events: Vec<StatusEvent>,
}

/// An entry before the replay gives it a `status_after`.
struct Entry {
    kind: StatusEventKind,
    occurred_at: Option<DateTime<Utc>>,
    offset: EventId,
    actor_id: Option<String>,
    delegated_by: Option<String>,
    superseded_by: Option<String>,
}

/// The status events of one decision, newest first, or `None` when the decision does not exist.
/// Reads the ledger's accept, reject and supersede events and the decision's own edges. Read-only.
pub fn get_decision_status_events(
    graph: &impl GraphView,
    ledger: &impl EventLedger,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionStatusEvents>>> {
    let started = query_timer_start();
    if !decision_node_exists(graph, decision_id)? {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: None,
        });
    }

    let direct = direct_entries(ledger, decision_id)?;
    let recorded = recorded_of(graph, decision_id)?;
    let mut entries = Vec::new();
    entries.extend(proposed_entry(graph, decision_id, &recorded)?);
    entries.extend(capture_entries(graph, decision_id, &recorded, &direct)?);
    entries.extend(direct);
    // One offset can hold several entries (a capture that proposes, accepts and replaces); the
    // kind then orders them as they apply, and the actor and replacing decision settle ties.
    entries.sort_by(|left, right| {
        (left.offset, left.kind, &left.actor_id, &left.superseded_by).cmp(&(
            right.offset,
            right.kind,
            &right.actor_id,
            &right.superseded_by,
        ))
    });

    let (mut superseded, mut accepted, mut rejected) = (false, false, false);
    let mut events: Vec<StatusEvent> = entries
        .into_iter()
        .map(|entry| {
            match entry.kind {
                StatusEventKind::Proposed => {}
                StatusEventKind::Accepted => accepted = true,
                StatusEventKind::Rejected => rejected = true,
                StatusEventKind::Superseded => superseded = true,
            }
            StatusEvent {
                event: entry.kind,
                occurred_at: entry.occurred_at,
                offset: entry.offset,
                actor: entry.actor_id.map(StatusActor::named),
                delegated_by: entry.delegated_by,
                superseded_by: entry.superseded_by,
                status_after: status_from_positions(superseded, accepted, rejected),
            }
        })
        .collect();
    events.reverse();

    Ok(QueryResponse {
        result_count: 1,
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: Some(DecisionStatusEvents {
            decision_id: decision_id.to_owned(),
            status: derive_decision_status(graph, decision_id)?,
            events,
        }),
    })
}

/// The proposal, from the decision's own node: the same entry the timeline calls `recorded`. A
/// classified capture has no `decision.proposed` event to read, so the node carries the offset,
/// the time and (through `PROPOSED_BY`) the proposer. A decision that is only referenced, never
/// recorded, has none.
fn proposed_entry(
    graph: &impl GraphView,
    decision_id: &str,
    recorded: &Recorded,
) -> Result<Option<Entry>> {
    let Some(offset) = recorded.offset else {
        return Ok(None);
    };
    Ok(Some(Entry {
        kind: StatusEventKind::Proposed,
        occurred_at: recorded.at,
        offset,
        actor_id: proposer_of(graph, decision_id)?,
        delegated_by: None,
        superseded_by: None,
    }))
}

/// Where and when a decision was recorded, read from its own node: the offset of the event that
/// created it and its `occurred_at`. Either is absent on a decision that is only referenced.
struct Recorded {
    offset: Option<EventId>,
    at: Option<DateTime<Utc>>,
}

fn recorded_of(graph: &impl GraphView, decision_id: &str) -> Result<Recorded> {
    let Some(row) = node_row(graph, NodeKind::Decision, decision_id)? else {
        return Ok(Recorded {
            offset: None,
            at: None,
        });
    };
    Ok(Recorded {
        offset: optional_int(&row, "event_origin")
            .and_then(|origin| EventId::try_from(origin).ok()),
        at: optional_datetime(&row, "occurred_at")?,
    })
}

fn proposer_of(graph: &impl GraphView, decision_id: &str) -> Result<Option<String>> {
    Ok(neighbor_pairs(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::ProposedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    )?
    .into_iter()
    .next()
    .map(|(actor_id, _)| actor_id))
}

/// The accept, reject and supersede events that name this decision, each dated and attributed by
/// the event itself. Reads only those three event types, in pages, so a ledger full of ingest
/// batches costs no more than one without.
fn direct_entries(ledger: &impl EventLedger, decision_id: &str) -> Result<Vec<Entry>> {
    let tenant = TenantId::local();
    let mut entries = Vec::new();
    let mut offset = 0;
    loop {
        let page = ledger.read_types_for_tenant(
            &tenant,
            STATUS_EVENT_TYPES,
            &[],
            offset,
            LEDGER_READ_PAGE_SIZE,
        )?;
        let Some(last) = page.last().and_then(|event| event.event_id) else {
            break;
        };
        offset = last;
        let full_page = page.len() >= LEDGER_READ_PAGE_SIZE;
        for event in &page {
            entries.extend(direct_entry(event, decision_id)?);
        }
        if !full_page {
            break;
        }
    }
    Ok(entries)
}

fn direct_entry(event: &Event, decision_id: &str) -> Result<Option<Entry>> {
    let offset = event
        .event_id
        .ok_or_else(|| query_error("event_id is required for status event queries"))?;
    let payload = events::validate(event)
        .map_err(|error| query_error(format!("invalid event {offset}: {error}")))?;
    let entry = |kind, delegated_by, superseded_by| Entry {
        kind,
        occurred_at: event.ts,
        offset,
        actor_id: Some(event.actor_id.clone()), // ubs:ignore: the entry owns its actor id
        delegated_by,
        superseded_by,
    };
    Ok(match payload {
        EventPayload::DecisionAccepted(accepted) if accepted.decision_id == decision_id => Some(
            entry(StatusEventKind::Accepted, accepted.delegated_by, None),
        ),
        EventPayload::DecisionRejected(rejected) if rejected.decision_id == decision_id => {
            Some(entry(StatusEventKind::Rejected, None, None))
        }
        EventPayload::DecisionSuperseded(superseded)
            if superseded.old_decision_id == decision_id
                && superseded.new_decision_id != decision_id =>
        {
            Some(entry(
                StatusEventKind::Superseded,
                None,
                Some(superseded.new_decision_id),
            ))
        }
        _ => None,
    })
}

/// The positions the graph holds that no event of their own explains: an acceptor or rejector a
/// classified capture named, and a newer decision whose capture named this one as the one it
/// replaces. Each is cited by the offset, and dated at the recorded time, of the decision that
/// made the capture (this one, or the replacing one): the capture's classification event wrote the
/// edge and created that decision's node.
fn capture_entries(
    graph: &impl GraphView,
    decision_id: &str,
    recorded: &Recorded,
    direct: &[Entry],
) -> Result<Vec<Entry>> {
    let named_by_event = |kind: StatusEventKind| -> BTreeSet<&str> {
        direct
            .iter()
            .filter(|entry| entry.kind == kind)
            .filter_map(|entry| entry.actor_id.as_deref())
            .collect()
    };
    let accepted_by_event = named_by_event(StatusEventKind::Accepted);
    let rejected_by_event = named_by_event(StatusEventKind::Rejected);
    let superseded_by_event: BTreeSet<&str> = direct
        .iter()
        .filter_map(|entry| entry.superseded_by.as_deref())
        .collect();

    let mut entries = Vec::new();
    for (kind, relation, by_event) in [
        (
            StatusEventKind::Accepted,
            RelationKind::AcceptedBy,
            &accepted_by_event,
        ),
        (
            StatusEventKind::Rejected,
            RelationKind::RejectedBy,
            &rejected_by_event,
        ),
    ] {
        for (actor_id, _) in neighbor_pairs(
            graph,
            NodeKind::Decision,
            decision_id,
            relation,
            NodeKind::Actor,
            Direction::Outgoing,
        )? {
            if by_event.contains(actor_id.as_str()) {
                continue;
            }
            entries.push(Entry {
                kind,
                occurred_at: recorded.at,
                offset: cited(recorded.offset, decision_id)?,
                actor_id: Some(actor_id),
                delegated_by: None,
                superseded_by: None,
            });
        }
    }
    // `SUPERSEDES` runs newer decision -> older decision, so the sources of an incoming edge are
    // the decisions that replaced this one.
    for (newer_id, _) in neighbor_pairs(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Incoming,
    )? {
        if newer_id == decision_id || superseded_by_event.contains(newer_id.as_str()) {
            continue;
        }
        let newer = recorded_of(graph, &newer_id)?;
        entries.push(Entry {
            kind: StatusEventKind::Superseded,
            occurred_at: newer.at,
            offset: cited(newer.offset, &newer_id)?,
            actor_id: proposer_of(graph, &newer_id)?,
            delegated_by: None,
            superseded_by: Some(newer_id),
        });
    }
    Ok(entries)
}

/// The offset of the capture that wrote a status edge, which is the offset of the decision that
/// made the capture. The projector gives every recorded decision one; a decision with a status
/// edge and none is refused rather than listed without a citation.
fn cited(offset: Option<EventId>, decision_id: &str) -> Result<EventId> {
    offset.ok_or_else(|| {
        query_error(format!(
            "{decision_id} names a status change but carries no ledger offset"
        ))
        .into()
    })
}

#[cfg(test)]
mod tests;
