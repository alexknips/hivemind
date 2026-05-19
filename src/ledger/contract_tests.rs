use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventId, EventSource, EventType};
use crate::ledger::EventLedger;
use crate::Result;

pub fn assert_monotonic_append<L: EventLedger>(ledger: &L) -> Result<()> {
    let event_ids = [
        ledger.append(make_event("evidence-1", Uuid::new_v4()))?,
        ledger.append(make_event("evidence-2", Uuid::new_v4()))?,
        ledger.append(make_event("evidence-3", Uuid::new_v4()))?,
    ];

    assert_eq!(event_ids, [1, 2, 3]);
    assert_eq!(ledger.latest_offset()?, 3);
    Ok(())
}

pub fn assert_dedup_by_event_uuid<L: EventLedger>(ledger: &L) -> Result<()> {
    let duplicate_uuid = Uuid::new_v4();

    let first_id = ledger.append(make_event("evidence-original", duplicate_uuid))?;
    let second_id = ledger.append(make_event("evidence-ignored", duplicate_uuid))?;

    assert_eq!(first_id, 1);
    assert_eq!(second_id, 1);
    assert_eq!(ledger.latest_offset()?, 1);

    let events = ledger.read(0, 10)?;
    assert_eq!(events.len(), 1);
    assert_eq!(event_id_from_payload(&events[0]), "evidence-original");

    Ok(())
}

pub fn assert_replay_from_zero_in_order<L: EventLedger>(ledger: &L) -> Result<()> {
    ledger.append(make_event("evidence-a", Uuid::new_v4()))?;
    ledger.append(make_event("evidence-b", Uuid::new_v4()))?;
    ledger.append(make_event("evidence-c", Uuid::new_v4()))?;

    let mut replayed_ids = Vec::new();
    ledger.replay_from(0, &mut |event| {
        replayed_ids.push(event.event_id.unwrap_or_default());
        Ok(())
    })?;

    assert_eq!(replayed_ids, vec![1, 2, 3]);

    Ok(())
}

pub fn assert_read_offset_and_limit<L: EventLedger>(ledger: &L) -> Result<()> {
    ledger.append(make_event("evidence-a", Uuid::new_v4()))?;
    ledger.append(make_event("evidence-b", Uuid::new_v4()))?;
    ledger.append(make_event("evidence-c", Uuid::new_v4()))?;

    let events = ledger.read(1, 1)?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, Some(2));

    let events = ledger.read(3, 5)?;
    assert!(events.is_empty());

    Ok(())
}

pub fn make_event(evidence_id: &str, event_uuid: Uuid) -> Event {
    Event {
        event_id: None,
        event_uuid,
        correlation_id: None,
        causation_event_id: None,
        event_type: EventType::EvidenceRecorded,
        actor_id: "actor:test".to_owned(),
        source: EventSource::Cli,
        source_ref: None,
        payload: json!({
            "evidence_id": evidence_id,
            "content": format!("content for {evidence_id}"),
            "source": "unit-test"
        }),
        ts: Some(Utc::now()),
    }
}

fn event_id_from_payload(event: &Event) -> &str {
    event
        .payload
        .get("evidence_id")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

#[allow(dead_code)]
fn _assert_event_ids_are_monotonic(ids: &[EventId]) {
    let mut previous = 0;
    for id in ids {
        assert!(*id > previous);
        previous = *id;
    }
}
