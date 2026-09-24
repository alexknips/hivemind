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
        include_str!("../../schemas/v0/decision.accepted.json"),
        include_str!("../../tests/fixtures/v0/delegation/decision.accepted.delegated.json"),
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
        include_str!("../../schemas/v0/hypothesis.recorded.json"),
        include_str!("../../tests/fixtures/v0/grounding/hypothesis.recorded.bet.json"),
        EventType::HypothesisRecorded,
    ),
    (
        include_str!("../../schemas/v0/relation.added.json"),
        include_str!("../../tests/fixtures/v0/relation.added.json"),
        EventType::RelationAdded,
    ),
    (
        include_str!("../../schemas/v0/relation.added.json"),
        include_str!("../../tests/fixtures/v0/grounding/relation.added.follows_from.json"),
        EventType::RelationAdded,
    ),
    (
        include_str!("../../schemas/v0/relation.removed.json"),
        include_str!("../../tests/fixtures/v0/grounding/relation.removed.follows_from.json"),
        EventType::RelationRemoved,
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
    (
        include_str!("../../schemas/v0/project.registered.json"),
        include_str!("../../tests/fixtures/v0/project.registered.json"),
        EventType::ProjectRegistered,
    ),
    (
        include_str!("../../schemas/v0/project.linked.json"),
        include_str!("../../tests/fixtures/v0/project.linked.json"),
        EventType::ProjectLinked,
    ),
    (
        include_str!("../../schemas/v0/project.unlinked.json"),
        include_str!("../../tests/fixtures/v0/project.unlinked.json"),
        EventType::ProjectUnlinked,
    ),
    (
        include_str!("../../schemas/v0/project.anchored.json"),
        include_str!("../../tests/fixtures/v0/project.anchored.json"),
        EventType::ProjectAnchored,
    ),
    (
        include_str!("../../schemas/v0/project.unanchored.json"),
        include_str!("../../tests/fixtures/v0/project.unanchored.json"),
        EventType::ProjectUnanchored,
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

fn delegated_acceptance_event() -> Event {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/delegation/decision.accepted.delegated.json"
    ))
    .unwrap()
}

#[test]
fn decision_accepted_carries_delegated_by_through_validation() {
    let payload = validate(&delegated_acceptance_event()).expect("delegated acceptance validates");
    assert_eq!(
        payload,
        EventPayload::DecisionAccepted(DecisionAcceptedPayload {
            decision_id: "dec-1".to_owned(),
            delegated_by: Some("human:alex".to_owned()),
        })
    );
}

#[test]
fn decision_accepted_without_delegated_by_still_parses_and_omits_it_on_write() {
    // Every event written before hivemind-zdsh.6 has no `delegated_by`; it must replay
    // unchanged, and re-serializing it must not invent the field.
    let event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/decision.accepted.json"
    ))
    .unwrap();
    let expected = DecisionAcceptedPayload {
        decision_id: "dec-1".to_owned(),
        delegated_by: None,
    };
    assert_eq!(
        validate(&event).unwrap(),
        EventPayload::DecisionAccepted(expected.clone())
    );
    assert_eq!(
        serde_json::to_value(&expected).unwrap(),
        json!({ "decision_id": "dec-1" })
    );
}

#[test]
fn decision_accepted_rejects_delegated_by_that_is_not_a_human() {
    for not_human in ["agent:claude:other", "alex", "human:", "human:  "] {
        let mut event = delegated_acceptance_event();
        event.payload["delegated_by"] = json!(not_human);
        assert!(
            matches!(
                validate(&event),
                Err(EventValidationError::DelegatedByNotHuman(_))
            ),
            "{not_human:?} must not be accepted as a delegator"
        );
    }
}

#[test]
fn decision_accepted_rejects_empty_delegated_by() {
    let mut event = delegated_acceptance_event();
    event.payload["delegated_by"] = json!(" ");
    assert!(matches!(
        validate(&event),
        Err(EventValidationError::EmptyField("payload.delegated_by"))
    ));
}

#[test]
fn decision_accepted_rejects_delegated_by_from_a_non_agent_accepter() {
    // A delegation qualifies an agent deciding for itself. A human (or an untyped actor)
    // accepting carries no delegation: a human who decides is recorded plainly.
    for accepter in ["human:alex", "human-a", "service:api"] {
        let mut event = delegated_acceptance_event();
        event.actor_id = accepter.to_owned();
        assert!(
            matches!(
                validate(&event),
                Err(EventValidationError::DelegationRequiresAgentAccepter(_))
            ),
            "{accepter:?} must not be able to carry a delegation"
        );
    }
}

#[test]
fn schema_rejects_delegated_by_that_is_not_a_human_actor() {
    let schema: Value =
        serde_json::from_str(include_str!("../../schemas/v0/decision.accepted.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    let mut event: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/delegation/decision.accepted.delegated.json"
    ))
    .unwrap();
    assert!(validator.is_valid(&event));

    event["payload"]["delegated_by"] = json!("agent:claude:other");
    assert!(!validator.is_valid(&event));
    event["payload"]["delegated_by"] = json!("human: ");
    assert!(!validator.is_valid(&event));
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
        tenant_id: Default::default(),
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
        tenant_id: Default::default(),
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
fn decision_proposed_rejects_quote_without_question() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/decision.proposed.json"
    ))
    .unwrap();
    event.payload["quote"] = json!("1a");

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::RequiresPairedField(
            "payload.quote",
            "payload.question"
        ))
    ));
}

#[test]
fn decision_proposed_rejects_question_without_quote() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/decision.proposed.json"
    ))
    .unwrap();
    event.payload["question"] = json!("Should a personal project be visible to the whole tenant?");

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::RequiresPairedField(
            "payload.question",
            "payload.quote"
        ))
    ));
}

#[test]
fn decision_proposed_accepts_paired_quote_and_question() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/decision.proposed.json"
    ))
    .unwrap();
    event.payload["quote"] = json!("1a");
    event.payload["question"] = json!("Should a personal project be visible to the whole tenant?");

    assert!(validate(&event).is_ok());
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
fn capture_item_decodes_pre_widening_scalar_accepted_rejected_by() {
    // Real pre-change on-disk shape: `accepted_by`/`rejected_by` were `Option<String>`
    // before widening to `Vec<String>` (hivemind-j4kw). Every previously-stored
    // `ingest.batch_classified` event has this shape forever — it must keep decoding.
    let raw = r#"{
        "kind": "decision",
        "title": "Use REST for HTTP API",
        "rationale": "REST maps naturally to resources.",
        "topic_keys": ["api-design"],
        "evidence_ids": [],
        "options": null,
        "chosen_option": null,
        "extraction_confidence": 0.9,
        "actor_id": "human:alice",
        "accepted_by": "human:bob",
        "rejected_by": null
    }"#;

    let capture: CaptureItem = serde_json::from_str(raw).expect("pre-widening shape decodes");
    assert_eq!(capture.accepted_by, vec!["human:bob".to_owned()]);
    assert_eq!(capture.rejected_by, Vec::<String>::new());
}

#[test]
fn capture_item_decodes_current_array_accepted_rejected_by() {
    let raw = r#"{
        "kind": "decision-request",
        "title": "full frontend rewrite in Vue 3",
        "rationale": "",
        "topic_keys": [],
        "evidence_ids": [],
        "options": null,
        "chosen_option": null,
        "extraction_confidence": 0.9,
        "actor_id": "human:kim",
        "accepted_by": [],
        "rejected_by": ["human:dev", "human:chen"]
    }"#;

    let capture: CaptureItem = serde_json::from_str(raw).expect("array shape decodes");
    assert_eq!(capture.accepted_by, Vec::<String>::new());
    assert_eq!(
        capture.rejected_by,
        vec!["human:dev".to_owned(), "human:chen".to_owned()]
    );
}

#[test]
fn ingest_batch_classified_grouped_batch_ids_matches_schema() {
    // hivemind-zdsh.18: session-grouped classification writes `batch_ids`
    // (plural) covering every batch the one model call classified. The
    // fixture carries two batch ids; confirm the published v0 schema
    // accepts it and Rust deserialization/validation round-trips it.
    let schema: Value = serde_json::from_str(include_str!(
        "../../schemas/v0/ingest.batch_classified.json"
    ))
    .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/ingest.batch_classified.json"
    ))
    .unwrap();

    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    assert!(
        validator.is_valid(&fixture),
        "grouped fixture matches schema"
    );

    let event: Event = serde_json::from_value(fixture).expect("fixture deserializes");
    let payload = validate(&event).expect("fixture validates");
    let EventPayload::IngestBatchClassified(payload) = payload else {
        panic!("expected IngestBatchClassified payload, got {payload:?}");
    };
    assert_eq!(
        payload.batch_ids,
        vec![
            "session-abc:0-512".to_owned(),
            "session-abc:512-1024".to_owned()
        ]
    );
}

#[test]
fn hypothesis_recorded_without_kind_replays_as_assumption() {
    // Real pre-change on-disk shape: every hypothesis.recorded event before this field
    // existed has no `kind` at all. It must keep decoding, and default to Assumption.
    let event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/hypothesis.recorded.json"
    ))
    .unwrap();

    let payload = validate(&event).expect("fixture validates");
    let EventPayload::HypothesisRecorded(payload) = payload else {
        panic!("expected HypothesisRecorded payload");
    };
    assert_eq!(payload.kind, HypothesisKind::Assumption);
    assert_eq!(payload.check_by, None);
    assert_eq!(payload.would_change_if, None);
}

#[test]
fn hypothesis_recorded_bet_fixture_carries_kind_check_by_and_would_change_if() {
    let event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/grounding/hypothesis.recorded.bet.json"
    ))
    .unwrap();

    let payload = validate(&event).expect("bet hypothesis validates");
    let EventPayload::HypothesisRecorded(payload) = payload else {
        panic!("expected HypothesisRecorded payload");
    };
    assert_eq!(payload.kind, HypothesisKind::Bet);
    assert_eq!(
        payload.check_by,
        Some("2026-10-01T00:00:00Z".parse().unwrap())
    );
    assert_eq!(
        payload.would_change_if.as_deref(),
        Some("A 10x load test shows p99 write latency above 50ms")
    );
}

#[test]
fn hypothesis_recorded_rejects_empty_would_change_if() {
    let mut event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/hypothesis.recorded.json"
    ))
    .unwrap();
    event.payload["would_change_if"] = json!("   ");

    assert!(matches!(
        validate(&event),
        Err(EventValidationError::EmptyField("payload.would_change_if"))
    ));
}

#[test]
fn relation_added_follows_from_fixture_links_a_decision_to_its_premise() {
    let event: Event = serde_json::from_str(include_str!(
        "../../tests/fixtures/v0/grounding/relation.added.follows_from.json"
    ))
    .unwrap();

    let payload = validate(&event).expect("FOLLOWS_FROM relation validates");
    let EventPayload::RelationAdded(payload) = payload else {
        panic!("expected RelationAdded payload");
    };
    assert_eq!(payload.relation, RelationKind::FollowsFrom);
    assert_eq!(payload.from_id, "decision-keep-embedded-db");
    assert_eq!(payload.to_id, "decision-slice-1-scope");
}

#[test]
fn relation_kind_accepts_follows_from_alias_and_wire_value() {
    let wire: RelationKind = serde_json::from_value(json!("FOLLOWS_FROM")).unwrap();
    let alias: RelationKind = serde_json::from_value(json!("follows_from")).unwrap();
    assert_eq!(wire, RelationKind::FollowsFrom);
    assert_eq!(alias, RelationKind::FollowsFrom);
    assert_eq!(serde_json::to_value(wire).unwrap(), json!("FOLLOWS_FROM"));
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
