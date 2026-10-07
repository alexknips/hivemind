//! When each received batch was said, for the time a captured decision is projected at.
//!
//! A received batch never reaches the graph, and the projector reads one event at a time, so a
//! replay keeps the newest turn time of every batch it passes. A classification after the
//! replay's starting offset may name batches received before it (the server's graph cache
//! replays only what is new), and those are read from the ledger before the replay starts. The
//! answer is the same either way: a batch counts when it was received before the classification
//! that names it, whether the replay started at the first event or later.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::events::{
    classified_batch_ids, received_batch_newest_turn_time, Event, EventId, EventType, TenantId,
};
use crate::ledger::EventLedger;
use crate::Result;

/// How many received batches one read of their ids asks for: the city cell's ~100k received
/// batches take a handful of round trips.
const HEAD_PAGE: usize = 10_000;

/// The newest turn time of each received batch whose turns carry one. A batch with no turn time
/// has no entry: nothing is guessed for it.
#[derive(Debug, Default)]
pub(super) struct BatchTurnTimes(HashMap<String, DateTime<Utc>>);

impl BatchTurnTimes {
    /// Takes in `event` when it is a received batch whose turns carry a time.
    pub(super) fn note(&mut self, event: &Event) {
        if event.event_type != EventType::IngestBatchReceived {
            return;
        }
        let (Some(batch_id), Some(newest)) = (
            batch_id(event),
            received_batch_newest_turn_time(&event.payload),
        ) else {
            return;
        };
        self.0
            .entry(batch_id.to_owned())
            .and_modify(|current| *current = (*current).max(newest))
            .or_insert(newest);
    }

    /// The newest turn time across `batch_ids`, `None` when none of them carries one.
    pub(super) fn newest(&self, batch_ids: &[String]) -> Option<DateTime<Utc>> {
        batch_ids
            .iter()
            .filter_map(|batch_id| self.0.get(batch_id.as_str()).copied())
            .max()
    }

    /// The times of the batches that a replay from `offset` classifies without having received
    /// them in the same replay, read from the events up to `offset`. Empty for a replay from the
    /// start, and when every classification in the replay names batches it received itself: the
    /// ledger before `offset` is only read when something needs it, and then only the ids of its
    /// received batches and the events of the ones wanted, never the turn text of the rest.
    pub(super) fn before(
        ledger: &impl EventLedger,
        tenant_id: &TenantId,
        offset: EventId,
    ) -> Result<Self> {
        let mut times = Self::default();
        if offset == 0 {
            return Ok(times);
        }
        let wanted = unreceived_batches_classified_after(ledger, tenant_id, offset)?;
        if wanted.is_empty() {
            return Ok(times);
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
            times.note(&event);
        }
        Ok(times)
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
