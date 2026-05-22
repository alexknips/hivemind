// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use serde_json::{json, Value};

const FIXTURES: &[(&str, &str, EventType)] = &[
    (
        include_str!("../../schemas/v0/decision.proposed.json"),
        include_str!("../../tests/fixtures/v0/decision.proposed.json"),
        EventType::DecisionProposed,
    ),
    (
        include_str!("../../schemas/v0/decision.requested.json"),
        include_str!("../../tests/fixtures/v0/decision.requested.json"),
        EventType::DecisionRequested,
    ),
    (
        include_str!("../../schemas/v0/decision.accepted.json"),
        include_str!("../../tests/fixtures/v0/decision.accepted.json"),
        EventType::DecisionAccepted,
    ),
    (
        include_str!("../../schemas/v0/decision.rejected.json"),
        include_str!("../../tests/fixtures/v0/decision.rejected.json"),
        EventType::DecisionRejected,
    ),
    (
        include_str!("../../schemas/v0/decision.superseded.json"),
        include_str!("../../tests/fixtures/v0/decision.superseded.json"),
        EventType::DecisionSuperseded,
    ),
    (
        include_str!("../../schemas/v0/evidence.recorded.json"),
        include_str!("../../tests/fixtures/v0/evidence.recorded.json"),
        EventType::EvidenceRecorded,
    ),
    (
        include_str!("../../schemas/v0/hypothesis.recorded.json"),
        include_str!("../../tests/fixtures/v0/hypothesis.recorded.json"),
        EventType::HypothesisRecorded,
    ),
    (
        include_str!("../../schemas/v0/relation.added.json"),
        include_str!("../../tests/fixtures/v0/relation.added.json"),
        EventType::RelationAdded,
    ),
    (
        include_str!("../../schemas/v0/blocker.reported.json"),
        include_str!("../../tests/fixtures/v0/blocker.reported.json"),
        EventType::BlockerReported,
    ),
    (
        include_str!("../../schemas/v0/blocker.resolved.json"),
        include_str!("../../tests/fixtures/v0/blocker.resolved.json"),
        EventType::BlockerResolved,
    ),
    (
        include_str!("../../schemas/v0/notification.sent.json"),
        include_str!("../../tests/fixtures/v0/notification.sent.json"),
        EventType::NotificationSent,
    ),
    (
        include_str!("../../schemas/v0/notification.acknowledged.json"),
        include_str!("../../tests/fixtures/v0/notification.acknowledged.json"),
        EventType::NotificationAcknowledged,
    ),
];

#[test]
fn valid_fixtures_match_json_schemas_and_rust_validation() {
    for (schema, fixture, event_type) in FIXTURES {
        let schema: Value = serde_json::from_str(schema).expect("schema is valid json");
        let fixture: Value = serde_json::from_str(fixture).expect("fixture is valid json");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");

        assert!(validator.is_valid(&fixture), "fixture should match schema");

        let event: Event = serde_json::from_value(fixture).expect("fixture deserializes");
        let payload = validate(&event).expect("fixture validates");
        assert_eq!(event.event_type, *event_type);
        assert_eq!(payload.event_type(), *event_type);
    }
}

#[test]
fn schema_rejects_missing_required_payload_field() {
    let schema: Value =
        serde_json::from_str(include_str!("../../schemas/v0/decision.proposed.json")).unwrap();
    let mut fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/decision.proposed.json"
    ))
    .unwrap();

    fixture
        .pointer_mut("/payload")
        .and_then(Value::as_object_mut)
        .unwrap()
        .remove("title");

    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    assert!(!validator.is_valid(&fixture));
}

#[test]
fn rust_validation_rejects_empty_actor() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/evidence.recorded.json"
    ))
    .unwrap();
    event.actor_id = " ".to_owned();

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::EmptyField("actor_id"))
    ));
}

#[test]
fn rust_validation_rejects_payload_type_mismatch() {
    let event = Event {
        event_id: Some(1),
        event_uuid: Uuid::parse_str("018f5d8a-03fb-7df0-8e36-64d7410cfe00").unwrap(),
        correlation_id: Some("session-1".to_owned()),
        causation_event_id: None,
        event_type: EventType::DecisionAccepted,
        actor_id: "agent-a".to_owned(),
        source: EventSource::Agent,
        source_ref: Some("agent:codex:test-session".to_owned()),
        payload: json!({ "evidence_id": "ev-1", "content": "Wrong payload" }),
        ts: None,
    };

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::Payload { .. })
    ));
}

#[test]
fn blocker_report_requires_decision_or_topic_anchor() {
    let event = Event {
        event_id: Some(9),
        event_uuid: Uuid::parse_str("018f5d8a-03fb-7df0-8e36-64d7410cfe09").unwrap(),
        correlation_id: Some("session-1".to_owned()),
        causation_event_id: None,
        event_type: EventType::BlockerReported,
        actor_id: "agent-a".to_owned(),
        source: EventSource::Agent,
        source_ref: Some("agent:codex:session-1".to_owned()),
        payload: json!({
            "blocker_id": "blocker-1",
            "blocked_actor_id": "agent-a",
            "decision_id": null,
            "topic_keys": [],
            "blocked_ref": "run-1",
            "blocked_ref_type": "agent_run",
            "reason": "No owner can make the decision yet.",
            "priority": "P1",
            "last_progress_at": null,
            "required_owner_id": null
        }),
        ts: None,
    };

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::EmptyField(
            "payload.decision_id_or_topic_keys"
        ))
    ));
}

#[test]
fn notification_sent_requires_source_event_ids() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/notification.sent.json"
    ))
    .unwrap();
    event.payload["source_event_ids"] = json!([]);

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::EmptyList("payload.source_event_ids"))
    ));
}

#[test]
fn blocker_notification_events_require_source_provenance() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/blocker.reported.json"
    ))
    .unwrap();
    event.source_ref = None;

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::EmptyField("source_ref"))
    ));
}
