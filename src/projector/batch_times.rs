//! What a classified decision is projected with from the batches it was classified from: when
//! they were said, which capture session shipped them, and who the session was held with.
//!
//! A received batch never reaches the graph, and the projector reads one event at a time, so a
//! replay keeps the newest turn time and the session of every batch it passes. A classification
//! after the replay's starting offset may name batches received before it (the server's graph
//! cache replays only what is new), and those are read from the ledger before the replay
//! starts. The answer is the same either way: a batch counts when it was received before the
//! classification that names it, whether the replay started at the first event or later.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::events::{
    classified_batch_ids, received_batch_newest_turn_time, received_batch_session_id,
    session_agent_actor, Event, EventId, EventType, TenantId,
};
use crate::ledger::EventLedger;
use crate::Result;

/// How many received batches one read of their ids asks for: the city cell's ~100k received
/// batches take a handful of round trips.
const HEAD_PAGE: usize = 10_000;

/// What a replay knows of the batches it has passed: the newest turn time of each whose turns
/// carry one, the session of each that names one, and who submitted each and with which agent
/// tool. A batch with no turn time has no time entry, and a batch that names no session has no
/// session entry: nothing is guessed for either.
#[derive(Debug, Default)]
pub(super) struct ReceivedBatches {
    turn_times: HashMap<String, DateTime<Utc>>,
    sessions: HashMap<String, String>,
    submitters: HashMap<String, Submitter>,
}

/// Who shipped a received batch: the actor on the event, and the agent tool it names.
#[derive(Debug)]
struct Submitter {
    actor_id: String,
    agent_tool: String,
}

/// The conversation a classification was drawn from, as the batches it covers state it. Each part
/// is empty when the ledger received none of the batches (a `hivemind emit` capture names a fresh
/// batch id): who was in a conversation is never guessed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ClassifiedSession {
    /// The capture sessions that shipped the batches, sorted and without repeats.
    pub(super) session_ids: Vec<String>,
    /// Whoever submitted the first covered batch that was received: the one who started the
    /// session.
    pub(super) initiator: Option<String>,
    /// Every covered batch's submitter and the agent that ran beside them
    /// ([`session_agent_actor`]), in the order the batches are named, each once.
    pub(super) participants: Vec<String>,
}

impl ReceivedBatches {
    /// Takes in `event` when it is a received batch. A batch received more than once keeps the
    /// newest of its turn times and the first session it named.
    pub(super) fn note(&mut self, event: &Event) {
        if event.event_type != EventType::IngestBatchReceived {
            return;
        }
        let Some(batch_id) = batch_id(event) else {
            return;
        };
        if let Some(newest) = received_batch_newest_turn_time(&event.payload) {
            self.turn_times
                .entry(batch_id.to_owned())
                .and_modify(|current| *current = (*current).max(newest))
                .or_insert(newest);
        }
        if let Some(session_id) = received_batch_session_id(&event.payload) {
            self.sessions
                .entry(batch_id.to_owned())
                .or_insert_with(|| session_id.to_owned());
        }
        self.submitters
            .entry(batch_id.to_owned())
            .or_insert_with(|| Submitter {
                actor_id: event.actor_id.clone(),
                agent_tool: event
                    .payload
                    .get("agent_tool")
                    .and_then(|tool| tool.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            });
    }

    /// The newest turn time across `batch_ids`, `None` when none of them carries one.
    pub(super) fn newest(&self, batch_ids: &[String]) -> Option<DateTime<Utc>> {
        batch_ids
            .iter()
            .filter_map(|batch_id| self.turn_times.get(batch_id.as_str()).copied())
            .max()
    }

    /// The conversation `batch_ids` were shipped from: their sessions, who started it and who
    /// took part.
    pub(super) fn session(&self, batch_ids: &[String]) -> ClassifiedSession {
        let session_ids: BTreeSet<&str> = batch_ids
            .iter()
            .filter_map(|batch_id| self.sessions.get(batch_id.as_str()))
            .map(String::as_str)
            .collect();
        let mut session = ClassifiedSession {
            session_ids: session_ids.into_iter().map(str::to_owned).collect(),
            ..ClassifiedSession::default()
        };
        for submitter in batch_ids
            .iter()
            .filter_map(|batch_id| self.submitters.get(batch_id.as_str()))
        {
            let agent = session_agent_actor(&submitter.actor_id, &submitter.agent_tool);
            for actor in [Some(submitter.actor_id.as_str()), agent.as_deref()]
                .into_iter()
                .flatten()
                .filter(|actor| !actor.is_empty())
            {
                if session.initiator.is_none() && actor == submitter.actor_id {
                    session.initiator = Some(actor.to_owned());
                }
                // ubs:ignore: == compares actor ids (public attribution, not secrets or tokens)
                if !session.participants.iter().any(|known| known == actor) {
                    session.participants.push(actor.to_owned());
                }
            }
        }
        session
    }

    /// The batches that a replay from `offset` classifies without having received them in the
    /// same replay, read from the events up to `offset`. Empty for a replay from the
    /// start, and when every classification in the replay names batches it received itself: the
    /// ledger before `offset` is only read when something needs it, and then only the ids of its
    /// received batches and the events of the ones wanted, never the turn text of the rest.
    pub(super) fn before(
        ledger: &impl EventLedger,
        tenant_id: &TenantId,
        offset: EventId,
    ) -> Result<Self> {
        let mut received = Self::default();
        if offset == 0 {
            return Ok(received);
        }
        let wanted = unreceived_batches_classified_after(ledger, tenant_id, offset)?;
        if wanted.is_empty() {
            return Ok(received);
        }

        let mut event_ids: Vec<EventId> = Vec::new();
        let mut cursor = 0;
        'pages: loop {
            let rows = ledger.read_fields_for_tenant(
                tenant_id,
                EventType::IngestBatchReceived,
                &["batch_id"],
                cursor,
                HEAD_PAGE,
            )?;
            for row in &rows {
                if row.event_id > offset {
                    break 'pages;
                }
                if matches!(row.fields.first(), Some(Some(id)) if wanted.contains(id.as_str())) {
                    event_ids.push(row.event_id);
                }
            }
            match rows.last() {
                Some(last) if rows.len() == HEAD_PAGE => cursor = last.event_id,
                _ => break,
            }
        }
        for event in ledger.read_ids_for_tenant(tenant_id, &event_ids)? {
            received.note(&event);
        }
        Ok(received)
    }
}

/// The ids of batches that a classification after `offset` names and that the replay does not
/// itself receive before that classification.
fn unreceived_batches_classified_after(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    offset: EventId,
) -> Result<HashSet<String>> {
    let mut received: HashSet<String> = HashSet::new();
    let mut wanted: HashSet<String> = HashSet::new();
    ledger.replay_from_for_tenant(tenant_id, offset, &mut |event| {
        match event.event_type {
            EventType::IngestBatchReceived => {
                if let Some(id) = batch_id(event) {
                    received.insert(id.to_owned());
                }
            }
            EventType::IngestBatchClassified => wanted.extend(
                classified_batch_ids(&event.payload)
                    .into_iter()
                    .filter(|id| !received.contains(id.as_str())),
            ),
            _ => {}
        }
        Ok(())
    })?;
    Ok(wanted)
}

fn batch_id(event: &Event) -> Option<&str> {
    event.payload.get("batch_id").and_then(|id| id.as_str())
}
