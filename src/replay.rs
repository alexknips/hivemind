//! Moving events from one ledger into another without changing what each one says
//! (hivemind-jawy).
//!
//! Write/ingest layer: a replayed event keeps its uuid, actor, source, source ref, correlation
//! id, payload and time. The one thing that cannot be kept is its `event_id`: the destination
//! numbers its events itself, and on a ledger that already holds events the numbers differ. So a
//! causation link travels as the *uuid* of the cause and is renumbered to wherever the cause sits
//! in the destination. Dedup is by `event_uuid`: running a replay again appends nothing the
//! destination already holds. Nothing here ranks, infers or rewrites.
//!
//! The same code runs behind `POST /v1/ledger/replay` (the cell appends what a client sends) and
//! behind `hivemind migrate --to <postgres-url>` (the CLI appends straight to the database), so
//! both renumber and check parity the same way.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{Event, EventId, EventSource, EventType, TenantId};
use crate::ledger::EventLedger;
use crate::Result;

/// Most events one replay request may carry. A larger request is refused, never cut short.
pub const MAX_REPLAY_BATCH: usize = 1_000;

/// Events a client puts in one request.
const SOURCE_BATCH_EVENTS: usize = 100;

/// Rough size a client lets one request grow to before it sends it (payload bytes).
const SOURCE_BATCH_BYTES: usize = 2 * 1024 * 1024;

/// One event on the wire between two ledgers: everything the event says, minus the numbers the
/// destination assigns (`event_id`) and the tenant it lands in (named by the request).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEvent {
    pub event_uuid: Uuid,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub actor_id: String,
    #[serde(default)]
    pub source: EventSource,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    /// The uuid of the event that caused this one. The source's `causation_event_id` is a number
    /// in the source ledger; this is the same link in a form that survives the move.
    #[serde(default)]
    pub causation_event_uuid: Option<Uuid>,
    pub payload: Value,
    /// When the event was recorded. Required: a replay keeps the original time, it never stamps
    /// "now".
    pub ts: DateTime<Utc>,
}

/// What one replay did, or with `dry_run` would do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCounts {
    /// Events in the batch.
    pub received: usize,
    /// Events the destination did not hold: appended by a real replay, only counted by a dry run.
    pub new_events: usize,
    /// Events whose uuid the destination already holds (or that repeat earlier in the batch).
    /// None of these is written again.
    pub already_present: usize,
}

impl ReplayCounts {
    fn add(&mut self, other: Self) {
        self.received += other.received;
        self.new_events += other.new_events;
        self.already_present += other.already_present;
    }
}

/// Appends `events`, in order, to `tenant_id` of `ledger`, keeping each event's uuid, actor,
/// source, source ref, correlation id, payload and time.
///
/// - An event whose uuid the ledger already holds is skipped, so a replay is safe to run again.
/// - A causation link is numbered by its cause's uuid: the cause must already be in the ledger or
///   earlier in `events`. Every such link is checked before the first append, so a batch with a
///   link that cannot be numbered writes nothing.
/// - `dry_run` only counts which events are new; it writes nothing and does not check causation
///   (the cause may sit in an earlier batch that a dry run did not write).
pub fn replay_events(
    ledger: &dyn EventLedger,
    tenant_id: &TenantId,
    events: Vec<ReplayEvent>,
    dry_run: bool,
) -> Result<ReplayCounts> {
    if events.len() > MAX_REPLAY_BATCH {
        return Err(CommandError::Validation(format!(
            "a replay carries at most {MAX_REPLAY_BATCH} events, got {}; send smaller batches",
            events.len()
        ))
        .into());
    }
    for event in &events {
        check_replayable(event)?;
    }

    let wanted: Vec<Uuid> = events
        .iter()
        .flat_map(|event| std::iter::once(event.event_uuid).chain(event.causation_event_uuid))
        .collect();
    let mut ids: HashMap<Uuid, EventId> =
        ledger.event_ids_for_uuids_for_tenant(tenant_id, &wanted)?;

    let mut counts = ReplayCounts {
        received: events.len(),
        ..ReplayCounts::default()
    };
    // Uuids the ledger holds, or will hold once the events before this one are appended.
    let mut held: HashSet<Uuid> = ids.keys().copied().collect();
    let mut is_new = Vec::with_capacity(events.len());
    for event in &events {
        if held.contains(&event.event_uuid) {
            counts.already_present += 1;
            is_new.push(false);
            continue;
        }
        if !dry_run {
            if let Some(cause) = event.causation_event_uuid {
                if !held.contains(&cause) {
                    return Err(unresolved_cause(event, cause));
                }
            }
        }
        held.insert(event.event_uuid);
        counts.new_events += 1;
        is_new.push(true);
    }
    if dry_run {
        return Ok(counts);
    }

    events
        .into_iter()
        .zip(is_new)
        .filter(|(_, new)| *new)
        .try_for_each(|(event, _)| {
            let causation_event_id = match event.causation_event_uuid {
                Some(cause) => Some(
                    *ids.get(&cause)
                        .ok_or_else(|| unresolved_cause(&event, cause))?,
                ),
                None => None,
            };
            let event_uuid = event.event_uuid;
            let event_id = ledger.append_for_tenant(
                tenant_id,
                Event {
                    tenant_id: tenant_id.clone(),
                    event_id: None,
                    event_uuid,
                    correlation_id: event.correlation_id,
                    causation_event_id,
                    event_type: event.event_type,
                    actor_id: event.actor_id,
                    source: event.source,
                    source_ref: event.source_ref,
                    payload: event.payload,
                    ts: Some(event.ts),
                },
            )?;
            ids.insert(event_uuid, event_id);
            Ok::<(), crate::HivemindError>(())
        })?;
    Ok(counts)
}

/// What a replay of a whole source did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceReplay {
    /// Events the source ledger holds for the tenant.
    pub source_events: usize,
    /// The counts summed over every batch.
    pub counts: ReplayCounts,
}

/// Reads every event of `source_tenant` from `source`, oldest first, and hands them in batches to
/// `send(batch, dry_run)`: the destination's side of the replay, a local [`replay_events`] call
/// or an HTTP request.
///
/// The source's `causation_event_id` is turned into the cause's uuid on the way out. A link whose
/// cause is not an earlier event of the source ledger is refused, before anything is sent for
/// that batch, because it could not be numbered in the destination either.
pub fn replay_source(
    source: &dyn EventLedger,
    source_tenant: &TenantId,
    dry_run: bool,
    send: &mut dyn FnMut(Vec<ReplayEvent>, bool) -> Result<ReplayCounts>,
) -> Result<SourceReplay> {
    let mut uuid_of: HashMap<EventId, Uuid> = HashMap::new();
    let mut batch: Vec<ReplayEvent> = Vec::new();
    let mut batch_bytes = 0usize;
    let mut source_events = 0usize;
    let mut counts = ReplayCounts::default();

    source.replay_from_for_tenant(source_tenant, 0, &mut |event| {
        let event_id = event.event_id.ok_or_else(|| {
            CommandError::Invariant(format!(
                "source event {} has no event id; cannot replay it",
                event.event_uuid
            ))
        })?;
        let ts = event.ts.ok_or_else(|| {
            CommandError::Invariant(format!(
                "source event {} (offset {event_id}) has no recorded time; a replay keeps the original time",
                event.event_uuid
            ))
        })?;
        let causation_event_uuid = match event.causation_event_id {
            None => None,
            Some(cause) => Some(*uuid_of.get(&cause).ok_or_else(|| {
                CommandError::Invariant(format!(
                    "source event {} (offset {event_id}) names offset {cause} as its cause, which is not an earlier event of the source ledger",
                    event.event_uuid
                ))
            })?),
        };
        uuid_of.insert(event_id, event.event_uuid);

        let replay = ReplayEvent {
            event_uuid: event.event_uuid,
            event_type: event.event_type,
            actor_id: event.actor_id.clone(),
            source: event.source,
            source_ref: event.source_ref.clone(),
            correlation_id: event.correlation_id.clone(),
            causation_event_uuid,
            payload: event.payload.clone(),
            ts,
        };
        check_replayable(&replay)?;
        batch_bytes += replay.payload.to_string().len();
        batch.push(replay);
        source_events += 1;

        if batch.len() >= SOURCE_BATCH_EVENTS || batch_bytes >= SOURCE_BATCH_BYTES {
            counts.add(send(std::mem::take(&mut batch), dry_run)?);
            batch_bytes = 0;
        }
        Ok(())
    })?;
    if !batch.is_empty() {
        counts.add(send(batch, dry_run)?);
    }

    Ok(SourceReplay {
        source_events,
        counts,
    })
}

/// Refuses an event a replay cannot carry faithfully.
fn check_replayable(event: &ReplayEvent) -> Result<()> {
    if event.actor_id.trim().is_empty() {
        return Err(CommandError::Validation(format!(
            "event {}: actor_id must not be empty; every event has an actor",
            event.event_uuid
        ))
        .into());
    }
    if let Some((type_name, field)) = embedded_event_ids(event) {
        return Err(CommandError::Validation(format!(
            "event {} ({type_name}) carries event ids in payload.{field}; the destination numbers its events itself, so those ids would point at other events. Nothing was written",
            event.event_uuid,
        ))
        .into());
    }
    Ok(())
}

/// The event type's name and the payload field of `event` that holds event ids, when it holds any. These two event types
/// are the only ones whose payload refers to other events by number; renumbering them is not
/// supported, so a replay refuses them rather than let them dangle.
fn embedded_event_ids(event: &ReplayEvent) -> Option<(&'static str, &'static str)> {
    match event.event_type {
        EventType::NotificationSent
            if event
                .payload
                .get("source_event_ids")
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty()) =>
        {
            Some(("notification.sent", "source_event_ids"))
        }
        EventType::BlockerResolved
            if event
                .payload
                .get("resolution_event_id")
                .is_some_and(|id| !id.is_null()) =>
        {
            Some(("blocker.resolved", "resolution_event_id"))
        }
        _ => None,
    }
}

fn unresolved_cause(event: &ReplayEvent, cause: Uuid) -> crate::HivemindError {
    CommandError::Validation(format!(
        "event {} names event {cause} as its cause, which is neither in the destination nor earlier in this batch. Nothing was written",
        event.event_uuid
    ))
    .into()
}

#[cfg(test)]
mod tests;
