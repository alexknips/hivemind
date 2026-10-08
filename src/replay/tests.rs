// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::cell::RefCell;

use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{Event, EventId, EventSource, EventType, TenantId};
use crate::ledger::contract_tests::make_event;
use crate::ledger::{EventLedger, InMemoryEventLedger, SqliteEventLedger};
use crate::{HivemindError, Result};

use super::{
    replay_events, replay_source, ReplayCounts, ReplayEvent, MAX_REPLAY_BATCH, SOURCE_BATCH_EVENTS,
};

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("hivemind-replay-{prefix}-{}", Uuid::new_v4()))
}

/// A source ledger whose event `n` (1-based) is caused by the event `cause_of(n)` returns.
fn source_with_chain(
    dir: &std::path::Path,
    count: usize,
    cause_of: impl Fn(usize) -> Option<EventId>,
) -> Result<SqliteEventLedger> {
    let source = SqliteEventLedger::open(dir)?;
    for n in 1..=count {
        let mut event = make_event(&format!("source-{n}"), Uuid::new_v4());
        event.causation_event_id = cause_of(n);
        event.actor_id = format!("agent:claude:source-{n}");
        event.source = EventSource::Agent;
        event.source_ref = Some(format!("ref-{n}"));
        source.append(event)?;
    }
    Ok(source)
}

fn replay_event(label: &str, cause: Option<Uuid>) -> ReplayEvent {
    ReplayEvent {
        event_uuid: Uuid::new_v4(),
        event_type: EventType::EvidenceRecorded,
        actor_id: "agent:claude:test".to_owned(),
        source: EventSource::Agent,
        source_ref: None,
        correlation_id: None,
        causation_event_uuid: cause,
        payload: json!({"evidence_id": label, "content": label, "source": "test"}),
        ts: Utc
            .with_ymd_and_hms(2026, 10, 7, 8, 5, 56)
            .single()
            .unwrap_or_default(),
    }
}

fn validation_message(error: HivemindError) -> String {
    match error {
        HivemindError::Command(CommandError::Validation(message)) => message,
        other => format!("not a validation error: {other}"),
    }
}

#[test]
fn a_source_moves_into_a_non_empty_destination_with_causation_renumbered_by_uuid() -> Result<()> {
    let dir = temp_dir("move");
    // Event n is caused by event n-1, so the source holds 4 links.
    let source = source_with_chain(&dir, 5, |n| (n > 1).then(|| (n - 1) as EventId))?;
    let tenant = TenantId::local();

    let destination = InMemoryEventLedger::new();
    for index in 0..7 {
        destination.append(make_event(&format!("resident-{index}"), Uuid::new_v4()))?;
    }

    let mut send = |batch, dry_run| replay_events(&destination, &tenant, batch, dry_run);
    let moved = replay_source(&source, &tenant, false, &mut send)?;
    assert_eq!(moved.source_events, 5);
    assert_eq!(
        moved.counts,
        ReplayCounts {
            received: 5,
            new_events: 5,
            already_present: 0
        }
    );

    let source_events = source.read(0, 10)?;
    let landed = destination.read(7, 10)?;
    assert_eq!(landed.len(), 5);
    for (index, (from, to)) in source_events.iter().zip(&landed).enumerate() {
        assert_eq!(to.event_uuid, from.event_uuid);
        assert_eq!(to.actor_id, from.actor_id);
        assert_eq!(to.source, from.source);
        assert_eq!(to.source_ref, from.source_ref);
        assert_eq!(to.payload, from.payload);
        assert_eq!(to.ts, from.ts);
        // Source event n (id n) was caused by id n-1; at the destination the same link points at
        // the cause's new id, which is 7 further on.
        let expected = (index > 0).then(|| to.event_id.unwrap_or_default() - 1);
        assert_eq!(to.causation_event_id, expected, "event {}", index + 1);
        assert!(
            to.event_id.unwrap_or_default() > 7,
            "landed after the residents"
        );
    }

    // A second run finds every uuid and writes nothing.
    let again = replay_source(&source, &tenant, false, &mut send)?;
    assert_eq!(again.counts.new_events, 0);
    assert_eq!(again.counts.already_present, 5);
    assert_eq!(destination.latest_offset()?, 12);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_dry_run_counts_what_would_move_and_writes_nothing() -> Result<()> {
    let dir = temp_dir("dry-run");
    let source = source_with_chain(&dir, 4, |n| (n > 1).then(|| (n - 1) as EventId))?;
    let tenant = TenantId::local();
    let destination = InMemoryEventLedger::new();

    let mut send = |batch, dry_run| replay_events(&destination, &tenant, batch, dry_run);
    let dry = replay_source(&source, &tenant, true, &mut send)?;
    assert_eq!(dry.counts.new_events, 4);
    assert_eq!(dry.counts.already_present, 0);
    assert_eq!(destination.latest_offset()?, 0);

    // After the real move, the same dry run is the parity check: nothing is new.
    replay_source(&source, &tenant, false, &mut send)?;
    let parity = replay_source(&source, &tenant, true, &mut send)?;
    assert_eq!(parity.counts.new_events, 0);
    assert_eq!(parity.counts.already_present, 4);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_long_source_goes_in_batches_and_a_link_across_a_batch_boundary_still_resolves() -> Result<()> {
    let dir = temp_dir("batches");
    let count = SOURCE_BATCH_EVENTS * 2 + 50;
    let source = source_with_chain(&dir, count, |n| (n > 1).then(|| (n - 1) as EventId))?;
    let tenant = TenantId::local();
    let destination = InMemoryEventLedger::new();
    destination.append(make_event("resident", Uuid::new_v4()))?;

    let sizes = RefCell::new(Vec::new());
    let mut send = |batch: Vec<ReplayEvent>, dry_run| {
        sizes.borrow_mut().push(batch.len());
        replay_events(&destination, &tenant, batch, dry_run)
    };
    let moved = replay_source(&source, &tenant, false, &mut send)?;
    assert_eq!(moved.source_events, count);
    assert_eq!(
        *sizes.borrow(),
        vec![SOURCE_BATCH_EVENTS, SOURCE_BATCH_EVENTS, 50]
    );

    // The first event of the second batch is caused by the last event of the first.
    let landed = destination.read(1, count)?;
    for (index, event) in landed.iter().enumerate().skip(1) {
        assert_eq!(
            event.causation_event_id,
            landed[index - 1].event_id,
            "event {}",
            index + 1
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_source_link_to_nothing_is_refused_before_anything_is_sent() -> Result<()> {
    let dir = temp_dir("dangling");
    // Event 2 names event 99 as its cause; the source has no such event.
    let source = source_with_chain(&dir, 2, |n| (n == 2).then_some(99))?;
    let tenant = TenantId::local();
    let destination = InMemoryEventLedger::new();

    let mut send = |batch, dry_run| replay_events(&destination, &tenant, batch, dry_run);
    let error = replay_source(&source, &tenant, false, &mut send)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        error.contains("not an earlier event of the source ledger"),
        "unexpected error: {error}"
    );
    assert_eq!(destination.latest_offset()?, 0, "nothing was sent");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_batch_with_a_cause_that_is_nowhere_writes_nothing() -> Result<()> {
    let destination = InMemoryEventLedger::new();
    let tenant = TenantId::local();
    let fine = replay_event("fine", None);
    let orphan = replay_event("orphan", Some(Uuid::new_v4()));

    let error = replay_events(&destination, &tenant, vec![fine, orphan], false)
        .err()
        .map(validation_message)
        .unwrap_or_default();
    assert!(
        error.contains("neither in the destination nor earlier"),
        "{error}"
    );
    assert_eq!(
        destination.latest_offset()?,
        0,
        "the fine event was not written either"
    );

    // The same batch as a dry run only counts.
    let counts = replay_events(
        &destination,
        &tenant,
        vec![
            replay_event("a", None),
            replay_event("b", Some(Uuid::new_v4())),
        ],
        true,
    )?;
    assert_eq!(counts.new_events, 2);
    Ok(())
}

#[test]
fn an_event_that_repeats_inside_a_batch_is_written_once() -> Result<()> {
    let destination = InMemoryEventLedger::new();
    let tenant = TenantId::local();
    let first = replay_event("twice", None);
    let counts = replay_events(&destination, &tenant, vec![first.clone(), first], false)?;
    assert_eq!(
        counts,
        ReplayCounts {
            received: 2,
            new_events: 1,
            already_present: 1
        }
    );
    assert_eq!(destination.latest_offset()?, 1);
    Ok(())
}

#[test]
fn an_event_whose_payload_names_event_ids_is_refused() -> Result<()> {
    let destination = InMemoryEventLedger::new();
    let tenant = TenantId::local();

    let mut notification = replay_event("notification", None);
    notification.event_type = EventType::NotificationSent;
    notification.payload = json!({"source_event_ids": [3, 4]});
    let mut resolved = replay_event("resolved", None);
    resolved.event_type = EventType::BlockerResolved;
    resolved.payload =
        json!({"blocker_id": "b", "resolution_event_id": 9, "resolution_reason": null});

    for event in [notification, resolved] {
        let error = replay_events(&destination, &tenant, vec![event], false)
            .err()
            .map(validation_message)
            .unwrap_or_default();
        assert!(error.contains("carries event ids in payload."), "{error}");
    }

    // A blocker resolved by a reason alone names no event, so it moves.
    let mut by_reason = replay_event("by-reason", None);
    by_reason.event_type = EventType::BlockerResolved;
    by_reason.payload =
        json!({"blocker_id": "b", "resolution_event_id": null, "resolution_reason": "done"});
    assert_eq!(
        replay_events(&destination, &tenant, vec![by_reason], false)?.new_events,
        1
    );
    Ok(())
}

#[test]
fn an_anonymous_event_and_an_oversized_batch_are_refused() -> Result<()> {
    let destination = InMemoryEventLedger::new();
    let tenant = TenantId::local();

    let mut anonymous = replay_event("anonymous", None);
    anonymous.actor_id = "  ".to_owned();
    let error = replay_events(&destination, &tenant, vec![anonymous], false)
        .err()
        .map(validation_message)
        .unwrap_or_default();
    assert!(error.contains("actor_id must not be empty"), "{error}");

    let too_many: Vec<ReplayEvent> = (0..=MAX_REPLAY_BATCH)
        .map(|n| replay_event(&format!("bulk-{n}"), None))
        .collect();
    let error = replay_events(&destination, &tenant, too_many, false)
        .err()
        .map(validation_message)
        .unwrap_or_default();
    assert!(error.contains("at most"), "{error}");
    assert_eq!(destination.latest_offset()?, 0);
    Ok(())
}

#[test]
fn the_wire_form_keeps_the_time_and_takes_no_destination_numbers() {
    let event = replay_event("wire", Some(Uuid::new_v4()));
    let wire = serde_json::to_value(&event).unwrap_or_default();
    assert_eq!(wire["type"], "evidence.recorded");
    assert!(wire.get("event_id").is_none());
    assert_eq!(
        serde_json::from_value::<ReplayEvent>(wire.clone()).ok(),
        Some(event)
    );

    // No time, no replay: a missing `ts` is refused, never stamped with "now".
    let mut without_time = wire.clone();
    if let Some(object) = without_time.as_object_mut() {
        object.remove("ts");
    }
    assert!(serde_json::from_value::<ReplayEvent>(without_time).is_err());

    // The destination assigns event ids and the request names the tenant.
    let mut with_id = wire;
    if let Some(object) = with_id.as_object_mut() {
        object.insert("event_id".to_owned(), json!(5));
    }
    assert!(serde_json::from_value::<ReplayEvent>(with_id).is_err());
}

#[test]
fn event_fields_other_than_causation_are_not_touched_by_a_replay() -> Result<()> {
    let destination = InMemoryEventLedger::new();
    let tenant = TenantId::local();
    let mut event = replay_event("kept", None);
    event.correlation_id = Some("corr-1".to_owned());
    event.source_ref = Some("ref-1".to_owned());
    replay_events(&destination, &tenant, vec![event.clone()], false)?;

    let stored: Event = destination
        .read(0, 1)?
        .into_iter()
        .next()
        .ok_or_else(|| CommandError::Invariant("event was not appended".to_owned()))?;
    assert_eq!(stored.event_uuid, event.event_uuid);
    assert_eq!(stored.correlation_id.as_deref(), Some("corr-1"));
    assert_eq!(stored.source_ref.as_deref(), Some("ref-1"));
    assert_eq!(stored.actor_id, event.actor_id);
    assert_eq!(stored.ts, Some(event.ts));
    Ok(())
}
