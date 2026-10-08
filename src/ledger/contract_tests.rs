// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{
    validate, Event, EventId, EventPayload, EventSource, EventType, HypothesisKind, RelationKind,
    TenantId,
};
use crate::ledger::EventLedger;
use crate::replay::{replay_events, ReplayCounts, ReplayEvent};
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

/// The typed, field and by-id reads (`read_types_for_tenant`, `read_fields_for_tenant`,
/// `read_ids_for_tenant`) agree on every backend: only the asked types come back, oldest first, with offset and limit counted over the
/// matching events; the named payload keys are left out and nothing else is; and the by-id read
/// returns exactly the events with those ids, skipping ids that name no event.
///
/// Failures are returned as errors rather than asserted, like the grounding helper below.
pub fn assert_typed_and_id_reads<L: EventLedger>(ledger: &L, tenant_id: &TenantId) -> Result<()> {
    let append = |event_type, payload| {
        ledger.append_for_tenant(tenant_id, make_grounding_event(event_type, payload, None))
    };
    let received_a = append(EventType::IngestBatchReceived, received_payload("batch-a"))?;
    let evidence = append(EventType::EvidenceRecorded, json!({ "evidence_id": "e-1" }))?;
    let received_b = append(EventType::IngestBatchReceived, received_payload("batch-b"))?;
    let classified = append(EventType::IngestBatchClassified, classified_payload())?;
    let received_c = append(EventType::IngestBatchReceived, received_payload("batch-c"))?;

    let heads = ledger.read_types_for_tenant(
        tenant_id,
        &[EventType::IngestBatchReceived],
        &["turns"],
        0,
        10,
    )?;
    require_equal(
        "received batches, oldest first",
        &ids_of(&heads),
        &vec![Some(received_a), Some(received_b), Some(received_c)],
    )?;
    require_equal(
        "turns left out of every head",
        &heads
            .iter()
            .any(|event| event.payload.get("turns").is_some()),
        &false,
    )?;
    require_equal(
        "the other payload keys kept",
        &heads.first().map(|event| {
            (
                event.payload.get("batch_id"),
                event.payload.get("session_id"),
            )
        }),
        &Some((Some(&json!("batch-a")), Some(&json!("session-1")))),
    )?;

    let whole =
        ledger.read_types_for_tenant(tenant_id, &[EventType::IngestBatchReceived], &[], 0, 10)?;
    require_equal(
        "turns kept when nothing is omitted",
        &whole
            .iter()
            .all(|event| event.payload.get("turns") == Some(&json!([{ "text": "hello" }]))),
        &true,
    )?;

    let both = ledger.read_types_for_tenant(
        tenant_id,
        &[
            EventType::IngestBatchClassified,
            EventType::IngestBatchReceived,
        ],
        &["captures"],
        0,
        10,
    )?;
    require_equal(
        "two types interleave in event order",
        &ids_of(&both),
        &vec![
            Some(received_a),
            Some(received_b),
            Some(classified),
            Some(received_c),
        ],
    )?;
    require_equal(
        "captures left out of the classification, its batch ids kept",
        &both.get(2).map(|event| {
            (
                event.payload.get("captures").is_some(),
                event.payload.get("batch_ids").cloned(),
            )
        }),
        &Some((false, Some(json!(["batch-a", "batch-b"])))),
    )?;

    let after_first = ledger.read_types_for_tenant(
        tenant_id,
        &[EventType::IngestBatchReceived],
        &[],
        received_a,
        1,
    )?;
    require_equal(
        "offset and limit count matching events only",
        &ids_of(&after_first),
        &vec![Some(received_b)],
    )?;

    let none =
        ledger.read_types_for_tenant(tenant_id, &[EventType::DecisionProposed], &[], 0, 10)?;
    require_equal("no event of the type", &none.len(), &0)?;
    let zero =
        ledger.read_types_for_tenant(tenant_id, &[EventType::IngestBatchReceived], &[], 0, 0)?;
    require_equal("a limit of zero", &zero.len(), &0)?;

    let fields = ledger.read_fields_for_tenant(
        tenant_id,
        EventType::IngestBatchReceived,
        &["batch_id", "session_id", "turns", "absent"],
        0,
        10,
    )?;
    require_equal(
        "fields: the received batches, oldest first",
        &fields
            .iter()
            .map(|row| Some(row.event_id))
            .collect::<Vec<_>>(),
        &vec![Some(received_a), Some(received_b), Some(received_c)],
    )?;
    require_equal(
        "fields: strings in the order asked; an array and an absent key read as None",
        &fields.first().map(|row| row.fields.clone()),
        &Some(vec![
            Some("batch-a".to_owned()),
            Some("session-1".to_owned()),
            None,
            None,
        ]),
    )?;
    require_equal(
        "fields: the actor and the time come with the row",
        &fields
            .first()
            .map(|row| (row.actor_id.as_str(), row.ts.is_some())),
        &Some(("actor:test", true)),
    )?;
    let after_first_fields = ledger.read_fields_for_tenant(
        tenant_id,
        EventType::IngestBatchReceived,
        &[],
        received_a,
        1,
    )?;
    require_equal(
        "fields: offset and limit count matching events only; no keys asked, none returned",
        &after_first_fields
            .iter()
            .map(|row| (row.event_id, row.fields.len()))
            .collect::<Vec<_>>(),
        &vec![(received_b, 0)],
    )?;
    let classified_fields = ledger.read_fields_for_tenant(
        tenant_id,
        EventType::IngestBatchClassified,
        &["batch_id"],
        0,
        10,
    )?;
    require_equal(
        "fields: another type, its own rows",
        &classified_fields
            .iter()
            .map(|row| (row.event_id, row.fields.clone()))
            .collect::<Vec<_>>(),
        &vec![(classified, vec![Some("batch-a".to_owned())])],
    )?;
    let zero_fields = ledger.read_fields_for_tenant(
        tenant_id,
        EventType::IngestBatchReceived,
        &["batch_id"],
        0,
        0,
    )?;
    require_equal("fields: a limit of zero", &zero_fields.len(), &0)?;

    let by_id =
        ledger.read_ids_for_tenant(tenant_id, &[received_c, evidence, 9_999, received_c])?;
    require_equal(
        "by id: oldest first, once each, unknown skipped",
        &ids_of(&by_id),
        &vec![Some(evidence), Some(received_c)],
    )?;
    require_equal(
        "by id: the payload comes whole",
        &by_id
            .get(1)
            .and_then(|event| event.payload.get("turns").cloned()),
        &Some(json!([{ "text": "hello" }])),
    )?;
    let no_ids = ledger.read_ids_for_tenant(tenant_id, &[])?;
    require_equal("no ids asked", &no_ids.len(), &0)?;

    Ok(())
}

fn ids_of(events: &[Event]) -> Vec<Option<EventId>> {
    events.iter().map(|event| event.event_id).collect()
}

fn received_payload(batch_id: &str) -> serde_json::Value {
    json!({
        "batch_id": batch_id,
        "session_id": "session-1",
        "agent_tool": "claude",
        "turns": [{ "text": "hello" }],
    })
}

fn classified_payload() -> serde_json::Value {
    json!({
        "batch_id": "batch-a",
        "batch_ids": ["batch-a", "batch-b"],
        "captures": [{ "kind": "decision", "title": "bulky" }],
    })
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

/// The uuid lookup a replay relies on: every uuid the tenant holds maps to its own event id, a
/// uuid it does not hold is absent, and an empty question is an empty answer.
pub fn assert_event_ids_for_uuids<L: EventLedger>(ledger: &L, tenant_id: &TenantId) -> Result<()> {
    let uuids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let labels = ["lookup-0", "lookup-1", "lookup-2"];
    let ids = labels
        .into_iter()
        .zip(uuids)
        .map(|(label, uuid)| ledger.append_for_tenant(tenant_id, make_event(label, uuid)))
        .collect::<Result<Vec<_>>>()?;
    let ([first, second, _], [first_id, second_id, _]) = (
        uuids,
        <[EventId; 3]>::try_from(ids)
            .map_err(|_| contract_failure("three appends must give three event ids"))?,
    );

    let found = ledger
        .event_ids_for_uuids_for_tenant(tenant_id, &[second, Uuid::new_v4(), first, second])?;
    let wanted = HashMap::from([(first, first_id), (second, second_id)]);
    if found != wanted {
        return Err(contract_failure(format!(
            "uuid lookup answered {found:?}, wanted {wanted:?}"
        )));
    }
    if !ledger
        .event_ids_for_uuids_for_tenant(tenant_id, &[])?
        .is_empty()
    {
        return Err(contract_failure("an empty uuid lookup must answer nothing"));
    }
    Ok(())
}

/// A replay into a ledger that already holds events (hivemind-jawy): the replayed events keep
/// their uuid, actor, source, source ref, correlation id, payload and time; each causation link
/// lands on the cause's NEW event id, not the number it had in the source; a second run writes
/// nothing; a dry run writes nothing; a link to a cause that is nowhere refuses the batch.
pub fn assert_replay_into_non_empty_ledger<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
) -> Result<()> {
    let base = Utc
        .with_ymd_and_hms(2026, 10, 7, 8, 5, 0)
        .single()
        .ok_or_else(|| contract_failure("fixed replay time is not a single instant"))?;
    let replayed = |label: &str, cause: Option<Uuid>, second: i64| ReplayEvent {
        event_uuid: Uuid::new_v4(),
        event_type: EventType::EvidenceRecorded,
        actor_id: format!("agent:claude:{label}"),
        source: EventSource::Agent,
        source_ref: Some(format!("ref-{label}")),
        correlation_id: Some(format!("corr-{label}")),
        causation_event_uuid: cause,
        payload: json!({"evidence_id": label, "content": format!("content for {label}"), "source": "replay"}),
        ts: base + chrono::Duration::seconds(second),
    };

    // The destination already holds three events, so every replayed event lands at a number
    // that differs from the one it had in the source.
    ["resident-a", "resident-b", "resident-c"]
        .into_iter()
        .try_for_each(|label| {
            ledger
                .append_for_tenant(tenant_id, make_event(label, Uuid::new_v4()))
                .map(|_| ())
        })?;

    let a = replayed("a", None, 10);
    let b = replayed("b", Some(a.event_uuid), 11);
    let c = replayed("c", Some(b.event_uuid), 12);
    let batch = vec![a.clone(), b.clone(), c.clone()];

    let counts = replay_events(ledger, tenant_id, batch.clone(), false)?;
    let wanted = ReplayCounts {
        received: 3,
        new_events: 3,
        already_present: 0,
    };
    if counts != wanted {
        return Err(contract_failure(format!("first replay counted {counts:?}")));
    }
    let landed = ledger.read_for_tenant(tenant_id, 3, 10)?;
    let kept = |event: &Event, source: &ReplayEvent| {
        event.event_uuid == source.event_uuid
            && event.actor_id == source.actor_id
            && event.source == source.source
            && event.source_ref == source.source_ref
            && event.correlation_id == source.correlation_id
            && event.payload == source.payload
            && event.ts == Some(source.ts)
    };
    if let Some((event, _)) = landed
        .iter()
        .zip(&batch)
        .find(|(event, source)| !kept(event, source))
    {
        return Err(contract_failure(format!(
            "a replayed event did not keep what it said: {event:?}"
        )));
    }
    let [landed_a, landed_b, landed_c] = <[Event; 3]>::try_from(landed)
        .map_err(|_| contract_failure("expected 3 replayed events after the 3 residents"))?;
    if landed_a.causation_event_id.is_some()
        || landed_b.causation_event_id != landed_a.event_id
        || landed_c.causation_event_id != landed_b.event_id
    {
        return Err(contract_failure(format!(
            "causation was not renumbered by cause uuid: {:?}",
            [&landed_a, &landed_b, &landed_c].map(|e| (e.event_id, e.causation_event_id))
        )));
    }

    let again = replay_events(ledger, tenant_id, batch, false)?;
    let wanted = ReplayCounts {
        received: 3,
        new_events: 0,
        already_present: 3,
    };
    if again != wanted || ledger.latest_offset_for_tenant(tenant_id)? != 6 {
        return Err(contract_failure(format!(
            "a re-run must write nothing, counted {again:?}"
        )));
    }

    let d = replayed("d", Some(c.event_uuid), 13);
    let dry = replay_events(ledger, tenant_id, vec![d.clone()], true)?;
    let wanted = ReplayCounts {
        received: 1,
        new_events: 1,
        already_present: 0,
    };
    if dry != wanted || ledger.latest_offset_for_tenant(tenant_id)? != 6 {
        return Err(contract_failure(format!(
            "a dry run must count and write nothing, counted {dry:?}"
        )));
    }

    let orphan = replayed("orphan", Some(Uuid::new_v4()), 14);
    if replay_events(ledger, tenant_id, vec![d, orphan], false).is_ok()
        || ledger.latest_offset_for_tenant(tenant_id)? != 6
    {
        return Err(contract_failure(
            "a batch with an unresolvable cause must write nothing",
        ));
    }
    Ok(())
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
