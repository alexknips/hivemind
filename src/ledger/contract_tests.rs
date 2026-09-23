// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{
    validate, Event, EventId, EventPayload, EventSource, EventType, HypothesisKind, RelationKind,
};
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

/// The grounding events (hivemind-gwhr.1) survive a ledger round trip: a `hypothesis.recorded`
/// bet keeps `kind` / `check_by` / `would_change_if`, a `hypothesis.recorded` in the shape every
/// pre-change event has (no `kind` at all) still replays as an assumption, and a
/// `relation.added` `FOLLOWS_FROM` keeps its endpoints and the causation link to the event that
/// proposed the decision. Payloads read back exactly as appended and validate to typed payloads.
///
/// Failures are returned as errors rather than asserted, so this generic helper adds nothing to
/// the UBS panic-surface counts that the assertion-based helpers above already carry.
pub fn assert_grounding_events_round_trip<L: EventLedger>(ledger: &L) -> Result<()> {
    let proposal_event_id = ledger.append(make_event("evidence-anchor", Uuid::new_v4()))?;

    ledger.append(make_grounding_event(
        EventType::HypothesisRecorded,
        bet_payload(),
        None,
    ))?;
    ledger.append(make_grounding_event(
        EventType::HypothesisRecorded,
        legacy_hypothesis_payload(),
        None,
    ))?;
    ledger.append(make_grounding_event(
        EventType::RelationAdded,
        follows_from_payload(),
        Some(proposal_event_id),
    ))?;

    let events = ledger.read(proposal_event_id, 10)?;
    let [bet_event, legacy_event, follows_from_event] = events.as_slice() else {
        return Err(contract_failure(format!(
            "expected the 3 grounding events after the anchor, read back {}",
            events.len()
        )));
    };

    require_equal("bet payload", &bet_event.payload, &bet_payload())?;
    let EventPayload::HypothesisRecorded(bet) = validated("bet hypothesis", bet_event)? else {
        return Err(contract_failure(
            "bet hypothesis is not hypothesis.recorded",
        ));
    };
    require_equal("bet kind", &bet.kind, &HypothesisKind::Bet)?;
    require_equal(
        "bet check_by",
        &bet.check_by,
        &Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).single(),
    )?;
    require_equal(
        "bet would_change_if",
        &bet.would_change_if.as_deref(),
        &Some(BET_WOULD_CHANGE_IF),
    )?;

    require_equal(
        "legacy hypothesis payload",
        &legacy_event.payload,
        &legacy_hypothesis_payload(),
    )?;
    let EventPayload::HypothesisRecorded(legacy) = validated("legacy hypothesis", legacy_event)?
    else {
        return Err(contract_failure(
            "legacy hypothesis is not hypothesis.recorded",
        ));
    };
    require_equal(
        "legacy hypothesis kind (replay default)",
        &legacy.kind,
        &HypothesisKind::Assumption,
    )?;
    require_equal("legacy hypothesis check_by", &legacy.check_by, &None)?;
    require_equal(
        "legacy hypothesis would_change_if",
        &legacy.would_change_if,
        &None,
    )?;

    require_equal(
        "FOLLOWS_FROM payload",
        &follows_from_event.payload,
        &follows_from_payload(),
    )?;
    require_equal(
        "FOLLOWS_FROM causation",
        &follows_from_event.causation_event_id,
        &Some(proposal_event_id),
    )?;
    let EventPayload::RelationAdded(relation) = validated("FOLLOWS_FROM", follows_from_event)?
    else {
        return Err(contract_failure("FOLLOWS_FROM is not relation.added"));
    };
    require_equal(
        "FOLLOWS_FROM relation",
        &relation.relation,
        &RelationKind::FollowsFrom,
    )?;
    require_equal(
        "FOLLOWS_FROM from_id",
        &relation.from_id,
        &"decision-later".to_owned(),
    )?;
    require_equal(
        "FOLLOWS_FROM to_id",
        &relation.to_id,
        &"decision-earlier".to_owned(),
    )?;

    Ok(())
}

const BET_WOULD_CHANGE_IF: &str = "A 10x load test shows p99 write latency above 50ms";

fn bet_payload() -> serde_json::Value {
    json!({
        "hypothesis_id": "hypothesis-bet",
        "statement": "Judgement call: Keep the embedded database for slice one",
        "kind": "bet",
        "check_by": "2026-10-01T00:00:00Z",
        "would_change_if": BET_WOULD_CHANGE_IF,
    })
}

/// The on-disk shape of every `hypothesis.recorded` event written before `kind` existed.
fn legacy_hypothesis_payload() -> serde_json::Value {
    json!({
        "hypothesis_id": "hypothesis-legacy",
        "statement": "Recorded before hypotheses had a kind",
    })
}

fn follows_from_payload() -> serde_json::Value {
    json!({
        "relation": "FOLLOWS_FROM",
        "from_id": "decision-later",
        "to_id": "decision-earlier",
    })
}

fn make_grounding_event(
    event_type: EventType,
    payload: serde_json::Value,
    causation_event_id: Option<EventId>,
) -> Event {
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::new_v4(),
        correlation_id: None,
        causation_event_id,
        event_type,
        actor_id: "actor:test".to_owned(),
        source: EventSource::Cli,
        source_ref: None,
        payload,
        ts: Some(Utc::now()),
    }
}

fn validated(label: &str, event: &Event) -> Result<EventPayload> {
    validate(event).map_err(|error| contract_failure(format!("{label} failed validation: {error}")))
}

fn require_equal<T: PartialEq + std::fmt::Debug>(
    label: &str,
    actual: &T,
    expected: &T,
) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(contract_failure(format!(
            "{label}: expected {expected:?}, got {actual:?}"
        )))
    }
}

fn contract_failure(message: impl Into<String>) -> crate::HivemindError {
    CommandError::Invariant(message.into()).into()
}

pub fn make_event(evidence_id: &str, event_uuid: Uuid) -> Event {
    Event {
        tenant_id: Default::default(),
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
