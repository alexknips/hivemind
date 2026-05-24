use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use hivemind::events::{
    self, BlockerReportedPayload, BlockerResolvedPayload, DecisionBlockerPriority,
    DecisionIdPayload, DecisionProposedPayload, DecisionRejectedPayload, DecisionRequestedPayload,
    DecisionSupersededPayload, Event, EventPayload, EventSource, EventType, EventValidationError,
    EvidenceRecordedPayload, HypothesisRecordedPayload, NotificationAcknowledgedPayload,
    NotificationSentPayload, RelationAddedPayload, RelationKind as EventRelationKind,
};
use hivemind::projector::{NodeKind, RelationKind as ProjectorRelationKind};
use hivemind::queries::{DecisionStatus, HypothesisStatus, QueryResponse};
use hivemind::{CliError, CommandError, HivemindError, LedgerError, ProjectorError, QueryError};
use serde_json::{json, Value};
use uuid::Uuid;

const EVENT_TYPES: [EventType; 12] = [
    EventType::DecisionProposed,
    EventType::DecisionRequested,
    EventType::DecisionAccepted,
    EventType::DecisionRejected,
    EventType::DecisionSuperseded,
    EventType::EvidenceRecorded,
    EventType::HypothesisRecorded,
    EventType::RelationAdded,
    EventType::BlockerReported,
    EventType::BlockerResolved,
    EventType::NotificationSent,
    EventType::NotificationAcknowledged,
];

const EVENT_RELATION_KINDS: [EventRelationKind; 6] = [
    EventRelationKind::BasedOn,
    EventRelationKind::HasOption,
    EventRelationKind::Chose,
    EventRelationKind::Assumes,
    EventRelationKind::Supports,
    EventRelationKind::Refutes,
];

const NODE_KINDS: [NodeKind; 8] = [
    NodeKind::Decision,
    NodeKind::DecisionRequest,
    NodeKind::Actor,
    NodeKind::Blocker,
    NodeKind::Evidence,
    NodeKind::Notification,
    NodeKind::Option,
    NodeKind::Hypothesis,
];

const PROJECTOR_RELATION_KINDS: [ProjectorRelationKind; 18] = [
    ProjectorRelationKind::ProposedBy,
    ProjectorRelationKind::DecisionRequestedBy,
    ProjectorRelationKind::DecisionRequestForDecision,
    ProjectorRelationKind::DecisionRequestRequiredOwner,
    ProjectorRelationKind::AcceptedBy,
    ProjectorRelationKind::RejectedBy,
    ProjectorRelationKind::Supersedes,
    ProjectorRelationKind::BlockedActor,
    ProjectorRelationKind::BlockerForDecision,
    ProjectorRelationKind::BlockerRequiredOwner,
    ProjectorRelationKind::NotificationForBlocker,
    ProjectorRelationKind::NotificationRecipient,
    ProjectorRelationKind::BasedOn,
    ProjectorRelationKind::HasOption,
    ProjectorRelationKind::Chose,
    ProjectorRelationKind::Assumes,
    ProjectorRelationKind::Supports,
    ProjectorRelationKind::Refutes,
];

const DECISION_STATUSES: [DecisionStatus; 5] = [
    DecisionStatus::Proposed,
    DecisionStatus::Accepted,
    DecisionStatus::Rejected,
    DecisionStatus::Contested,
    DecisionStatus::Superseded,
];

const HYPOTHESIS_STATUSES: [HypothesisStatus; 3] = [
    HypothesisStatus::Open,
    HypothesisStatus::Supported,
    HypothesisStatus::Refuted,
];

#[test]
fn event_payload_variants_have_stable_event_types() {
    let cases = typed_payload_cases();
    let seen: BTreeSet<String> = cases
        .iter()
        .map(|(event_type, _)| event_type_name(*event_type).to_owned())
        .collect();

    assert_eq!(cases.len(), EVENT_TYPES.len());
    assert_eq!(seen, event_type_names());

    for (event_type, payload) in cases {
        assert_eq!(payload_variant_type(&payload), event_type);
        assert_eq!(payload.event_type(), event_type);

        let event = event_with_payload(event_type, payload_json(&payload));
        let validated = events::validate(&event).expect("minimal typed payload validates");
        assert_eq!(payload_variant_type(&validated), event_type);
    }
}

#[test]
fn validate_rejects_shape_distinct_event_type_payload_mismatches() {
    let cases = typed_payload_cases();

    for (payload_type, payload) in &cases {
        for event_type in EVENT_TYPES {
            if event_type == *payload_type || shape_compatible(*payload_type, event_type) {
                continue;
            }

            let event = event_with_payload(event_type, payload_json(payload));
            assert!(
                matches!(
                    events::validate(&event),
                    Err(EventValidationError::Payload { .. })
                ),
                "expected {event_type:?} to reject payload for {payload_type:?}"
            );
        }
    }
}

#[test]
fn schema_and_fixture_sets_match_event_types() {
    let schema_dir = manifest_path(&["schemas", "v0"]);
    let fixture_dir = manifest_path(&["tests", "fixtures", "v0"]);
    let expected_names = event_type_names();

    assert_eq!(json_file_stems(&schema_dir), expected_names);
    assert_eq!(json_file_stems(&fixture_dir), expected_names);

    for event_type in EVENT_TYPES {
        let event_name = event_type_name(event_type);
        assert_eq!(
            serde_json::to_value(event_type).expect("event type serializes"),
            json!(event_name)
        );

        let schema = read_json(&schema_dir.join(format!("{event_name}.json")));
        assert_eq!(
            schema.pointer("/properties/type/const"),
            Some(&json!(event_name))
        );

        let fixture = read_json(&fixture_dir.join(format!("{event_name}.json")));
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        assert!(
            validator.is_valid(&fixture),
            "{event_name} fixture matches schema"
        );

        let event: Event = serde_json::from_value(fixture).expect("fixture deserializes");
        assert_eq!(event.event_type, event_type);

        let typed_payload =
            typed_payload_from_value(event.event_type, event.payload.clone()).unwrap();
        assert_eq!(payload_variant_type(&typed_payload), event_type);

        let validated = events::validate(&event).expect("fixture validates");
        assert_eq!(payload_variant_type(&validated), event_type);
    }
}

#[test]
fn event_relation_serialization_aliases_round_trip() {
    for relation in EVENT_RELATION_KINDS {
        let (canonical, alias) = event_relation_contract(relation);

        assert_eq!(
            serde_json::to_value(relation).expect("relation serializes"),
            json!(canonical)
        );
        assert_eq!(
            serde_json::from_value::<EventRelationKind>(json!(canonical)).unwrap(),
            relation
        );
        assert_eq!(
            serde_json::from_value::<EventRelationKind>(json!(alias)).unwrap(),
            relation
        );
    }
}

#[test]
fn graph_kind_contracts_match_plan_semantics() {
    assert_eq!(NodeKind::ALL, NODE_KINDS);
    assert_eq!(ProjectorRelationKind::ALL, PROJECTOR_RELATION_KINDS);

    let mut node_tables = BTreeSet::new();
    for kind in NODE_KINDS {
        let table = node_kind_contract(kind);
        assert_eq!(kind.table_name(), table);
        assert!(node_tables.insert(table), "duplicate node table: {table}");
    }

    let mut relation_tables = BTreeSet::new();
    for kind in PROJECTOR_RELATION_KINDS {
        let (table, from, to) = projector_relation_contract(kind);
        assert_eq!(kind.table_name(), table);
        assert_eq!(kind.endpoints(), (from, to));
        assert!(
            relation_tables.insert(table),
            "duplicate relation table: {table}"
        );
    }
}

#[test]
fn query_dto_contracts_serialize_stable_fields() {
    for status in DECISION_STATUSES {
        assert_eq!(
            serde_json::to_value(status).expect("decision status serializes"),
            json!(decision_status_contract(status))
        );
    }

    for status in HYPOTHESIS_STATUSES {
        assert_eq!(
            serde_json::to_value(status).expect("hypothesis status serializes"),
            json!(hypothesis_status_contract(status))
        );
    }

    let response = QueryResponse {
        result_count: 1,
        truncated: false,
        latency_ms: 7,
        data: json!({ "id": "decision-1" }),
    };
    let value = serde_json::to_value(response).expect("query response serializes");
    let object = value.as_object().expect("response is an object");
    let keys: BTreeSet<String> = object.keys().cloned().collect();

    assert_eq!(
        keys,
        BTreeSet::from([
            "data".to_owned(),
            "latency_ms".to_owned(),
            "result_count".to_owned(),
            "truncated".to_owned(),
        ])
    );
    assert_eq!(object["result_count"], json!(1));
    assert_eq!(object["truncated"], json!(false));
    assert_eq!(object["latency_ms"], json!(7));
    assert_eq!(object["data"], json!({ "id": "decision-1" }));
}

#[test]
fn domain_error_conversions_preserve_error_category() {
    let cases: [(HivemindError, &str); 5] = [
        (
            LedgerError::Storage("disk full".to_owned()).into(),
            "ledger",
        ),
        (
            ProjectorError::Projection("missing node".to_owned()).into(),
            "projector",
        ),
        (
            CommandError::Invariant("actor_id is required".to_owned()).into(),
            "command",
        ),
        (QueryError::Execution("timeout".to_owned()).into(), "query"),
        (
            CliError::InvalidInput("--actor is empty".to_owned()).into(),
            "cli",
        ),
    ];

    for (error, expected_category) in cases {
        assert_eq!(error_category(&error), expected_category);
    }
}

fn event_type_name(event_type: EventType) -> &'static str {
    match event_type {
        EventType::DecisionProposed => "decision.proposed",
        EventType::DecisionRequested => "decision.requested",
        EventType::DecisionAccepted => "decision.accepted",
        EventType::DecisionRejected => "decision.rejected",
        EventType::DecisionSuperseded => "decision.superseded",
        EventType::EvidenceRecorded => "evidence.recorded",
        EventType::HypothesisRecorded => "hypothesis.recorded",
        EventType::RelationAdded => "relation.added",
        EventType::BlockerReported => "blocker.reported",
        EventType::BlockerResolved => "blocker.resolved",
        EventType::NotificationSent => "notification.sent",
        EventType::NotificationAcknowledged => "notification.acknowledged",
    }
}

fn payload_variant_type(payload: &EventPayload) -> EventType {
    match payload {
        EventPayload::DecisionProposed(_) => EventType::DecisionProposed,
        EventPayload::DecisionRequested(_) => EventType::DecisionRequested,
        EventPayload::DecisionAccepted(_) => EventType::DecisionAccepted,
        EventPayload::DecisionRejected(_) => EventType::DecisionRejected,
        EventPayload::DecisionSuperseded(_) => EventType::DecisionSuperseded,
        EventPayload::EvidenceRecorded(_) => EventType::EvidenceRecorded,
        EventPayload::HypothesisRecorded(_) => EventType::HypothesisRecorded,
        EventPayload::RelationAdded(_) => EventType::RelationAdded,
        EventPayload::BlockerReported(_) => EventType::BlockerReported,
        EventPayload::BlockerResolved(_) => EventType::BlockerResolved,
        EventPayload::NotificationSent(_) => EventType::NotificationSent,
        EventPayload::NotificationAcknowledged(_) => EventType::NotificationAcknowledged,
    }
}

fn typed_payload_from_value(
    event_type: EventType,
    payload: Value,
) -> serde_json::Result<EventPayload> {
    Ok(match event_type {
        EventType::DecisionProposed => {
            EventPayload::DecisionProposed(serde_json::from_value(payload)?)
        }
        EventType::DecisionRequested => {
            EventPayload::DecisionRequested(serde_json::from_value(payload)?)
        }
        EventType::DecisionAccepted => {
            EventPayload::DecisionAccepted(serde_json::from_value(payload)?)
        }
        EventType::DecisionRejected => {
            EventPayload::DecisionRejected(serde_json::from_value(payload)?)
        }
        EventType::DecisionSuperseded => {
            EventPayload::DecisionSuperseded(serde_json::from_value(payload)?)
        }
        EventType::EvidenceRecorded => {
            EventPayload::EvidenceRecorded(serde_json::from_value(payload)?)
        }
        EventType::HypothesisRecorded => {
            EventPayload::HypothesisRecorded(serde_json::from_value(payload)?)
        }
        EventType::RelationAdded => EventPayload::RelationAdded(serde_json::from_value(payload)?),
        EventType::BlockerReported => {
            EventPayload::BlockerReported(serde_json::from_value(payload)?)
        }
        EventType::BlockerResolved => {
            EventPayload::BlockerResolved(serde_json::from_value(payload)?)
        }
        EventType::NotificationSent => {
            EventPayload::NotificationSent(serde_json::from_value(payload)?)
        }
        EventType::NotificationAcknowledged => {
            EventPayload::NotificationAcknowledged(serde_json::from_value(payload)?)
        }
    })
}

fn typed_payload_cases() -> Vec<(EventType, EventPayload)> {
    vec![
        (
            EventType::DecisionProposed,
            EventPayload::DecisionProposed(DecisionProposedPayload {
                decision_id: "decision:minimal".to_owned(),
                title: "Choose a storage backend".to_owned(),
                rationale: "Contract tests need one valid payload per event type".to_owned(),
                topic_keys: Vec::new(),
                option_ids: Vec::new(),
                chosen_option_id: None,
                hypothesis_ids: Vec::new(),
                evidence_ids: Vec::new(),
            }),
        ),
        (
            EventType::DecisionRequested,
            EventPayload::DecisionRequested(DecisionRequestedPayload {
                topic_keys: vec!["release".to_owned()],
                decision_id: Some("decision:minimal".to_owned()),
                reason: "A human owner must approve the release path".to_owned(),
                priority: DecisionBlockerPriority::P1,
                required_owner_id: Some("human:owner".to_owned()),
                authority_class: "human_required".to_owned(),
                requested_by: "agent:contract-test".to_owned(),
                client_request_id: "request:minimal".to_owned(),
            }),
        ),
        (
            EventType::DecisionAccepted,
            EventPayload::DecisionAccepted(DecisionIdPayload {
                decision_id: "decision:minimal".to_owned(),
            }),
        ),
        (
            EventType::DecisionRejected,
            EventPayload::DecisionRejected(DecisionRejectedPayload {
                decision_id: "decision:minimal".to_owned(),
                reason: Some("Contract tests need disagreement rationale".to_owned()),
            }),
        ),
        (
            EventType::DecisionSuperseded,
            EventPayload::DecisionSuperseded(DecisionSupersededPayload {
                old_decision_id: "decision:old".to_owned(),
                new_decision_id: "decision:new".to_owned(),
            }),
        ),
        (
            EventType::EvidenceRecorded,
            EventPayload::EvidenceRecorded(EvidenceRecordedPayload {
                evidence_id: "evidence:minimal".to_owned(),
                content: "Type contracts are useful before behavior tests".to_owned(),
                source: None,
            }),
        ),
        (
            EventType::HypothesisRecorded,
            EventPayload::HypothesisRecorded(HypothesisRecordedPayload {
                hypothesis_id: "hypothesis:minimal".to_owned(),
                statement: "The domain model remains internally consistent".to_owned(),
            }),
        ),
        (
            EventType::RelationAdded,
            EventPayload::RelationAdded(RelationAddedPayload {
                relation: EventRelationKind::Supports,
                from_id: "evidence:minimal".to_owned(),
                to_id: "hypothesis:minimal".to_owned(),
            }),
        ),
        (
            EventType::BlockerReported,
            EventPayload::BlockerReported(BlockerReportedPayload {
                blocker_id: "blocker:minimal".to_owned(),
                blocked_actor_id: "agent:contract-test".to_owned(),
                decision_id: Some("decision:minimal".to_owned()),
                topic_keys: Vec::new(),
                blocked_ref: "run:minimal".to_owned(),
                blocked_ref_type: "agent_run".to_owned(),
                reason: "Contract tests need one valid blocker payload".to_owned(),
                priority: DecisionBlockerPriority::P1,
                last_progress_at: None,
                required_owner_id: Some("human:owner".to_owned()),
            }),
        ),
        (
            EventType::BlockerResolved,
            EventPayload::BlockerResolved(BlockerResolvedPayload {
                blocker_id: "blocker:minimal".to_owned(),
                resolution_event_id: Some(1),
                resolution_reason: None,
            }),
        ),
        (
            EventType::NotificationSent,
            EventPayload::NotificationSent(NotificationSentPayload {
                blocker_id: "blocker:minimal".to_owned(),
                recipient_actor_id: "human:owner".to_owned(),
                channel: "slack".to_owned(),
                threshold_rule: "p1_human_required_direct_15m".to_owned(),
                source_event_ids: vec![9],
                dedupe_key: "tenant:decision:minimal:blocker:minimal:P1".to_owned(),
                sent_at: chrono::DateTime::parse_from_rfc3339("2026-05-19T11:30:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            }),
        ),
        (
            EventType::NotificationAcknowledged,
            EventPayload::NotificationAcknowledged(NotificationAcknowledgedPayload {
                notification_id: "notification:minimal".to_owned(),
                ack_at: chrono::DateTime::parse_from_rfc3339("2026-05-19T11:45:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                snooze_until: None,
            }),
        ),
    ]
}

fn payload_json(payload: &EventPayload) -> Value {
    match payload {
        EventPayload::DecisionProposed(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::DecisionRequested(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::DecisionAccepted(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::DecisionRejected(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::DecisionSuperseded(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::EvidenceRecorded(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::HypothesisRecorded(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::RelationAdded(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::BlockerReported(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::BlockerResolved(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::NotificationSent(payload) => serde_json::to_value(payload).unwrap(),
        EventPayload::NotificationAcknowledged(payload) => serde_json::to_value(payload).unwrap(),
    }
}

fn event_with_payload(event_type: EventType, payload: Value) -> Event {
    Event {
        event_id: Some(1),
        event_uuid: Uuid::nil(),
        correlation_id: Some("contract-test".to_owned()),
        causation_event_id: None,
        event_type,
        actor_id: "agent:contract-test".to_owned(),
        source: EventSource::Agent,
        source_ref: Some("type-contracts".to_owned()),
        payload,
        ts: None,
    }
}

fn shape_compatible(left: EventType, right: EventType) -> bool {
    matches!(
        (left, right),
        (EventType::DecisionAccepted, EventType::DecisionRejected)
    )
}

fn event_type_names() -> BTreeSet<String> {
    EVENT_TYPES
        .iter()
        .map(|event_type| event_type_name(*event_type).to_owned())
        .collect()
}

fn json_file_stems(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .expect("contract directory is readable")
        .map(|entry| entry.expect("directory entry is readable").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .map(|path| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .expect("json filename is utf-8")
                .to_owned()
        })
        .collect()
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).expect("contract json is readable");
    serde_json::from_str(&text).expect("contract json parses")
}

fn manifest_path(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.extend(parts);
    path
}

fn event_relation_contract(kind: EventRelationKind) -> (&'static str, &'static str) {
    match kind {
        EventRelationKind::BasedOn => ("BASED_ON", "based_on"),
        EventRelationKind::HasOption => ("HAS_OPTION", "has_option"),
        EventRelationKind::Chose => ("CHOSE", "chose"),
        EventRelationKind::Assumes => ("ASSUMES", "assumes"),
        EventRelationKind::Supports => ("SUPPORTS", "supports"),
        EventRelationKind::Refutes => ("REFUTES", "refutes"),
    }
}

fn node_kind_contract(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Decision => "Decision",
        NodeKind::DecisionRequest => "DecisionRequest",
        NodeKind::Actor => "Actor",
        NodeKind::Blocker => "Blocker",
        NodeKind::Evidence => "Evidence",
        NodeKind::Notification => "Notification",
        NodeKind::Option => "Option",
        NodeKind::Hypothesis => "Hypothesis",
    }
}

fn projector_relation_contract(kind: ProjectorRelationKind) -> (&'static str, NodeKind, NodeKind) {
    match kind {
        ProjectorRelationKind::ProposedBy => ("PROPOSED_BY", NodeKind::Decision, NodeKind::Actor),
        ProjectorRelationKind::DecisionRequestedBy => (
            "DECISION_REQUESTED_BY",
            NodeKind::DecisionRequest,
            NodeKind::Actor,
        ),
        ProjectorRelationKind::DecisionRequestForDecision => (
            "DECISION_REQUEST_FOR_DECISION",
            NodeKind::DecisionRequest,
            NodeKind::Decision,
        ),
        ProjectorRelationKind::DecisionRequestRequiredOwner => (
            "DECISION_REQUEST_REQUIRED_OWNER",
            NodeKind::DecisionRequest,
            NodeKind::Actor,
        ),
        ProjectorRelationKind::AcceptedBy => ("ACCEPTED_BY", NodeKind::Decision, NodeKind::Actor),
        ProjectorRelationKind::RejectedBy => ("REJECTED_BY", NodeKind::Decision, NodeKind::Actor),
        ProjectorRelationKind::Supersedes => ("SUPERSEDES", NodeKind::Decision, NodeKind::Decision),
        ProjectorRelationKind::BlockedActor => {
            ("BLOCKED_ACTOR", NodeKind::Blocker, NodeKind::Actor)
        }
        ProjectorRelationKind::BlockerForDecision => (
            "BLOCKER_FOR_DECISION",
            NodeKind::Blocker,
            NodeKind::Decision,
        ),
        ProjectorRelationKind::BlockerRequiredOwner => {
            ("BLOCKER_REQUIRED_OWNER", NodeKind::Blocker, NodeKind::Actor)
        }
        ProjectorRelationKind::NotificationForBlocker => (
            "NOTIFICATION_FOR_BLOCKER",
            NodeKind::Notification,
            NodeKind::Blocker,
        ),
        ProjectorRelationKind::NotificationRecipient => (
            "NOTIFICATION_RECIPIENT",
            NodeKind::Notification,
            NodeKind::Actor,
        ),
        ProjectorRelationKind::BasedOn => ("BASED_ON", NodeKind::Decision, NodeKind::Evidence),
        ProjectorRelationKind::HasOption => ("HAS_OPTION", NodeKind::Decision, NodeKind::Option),
        ProjectorRelationKind::Chose => ("CHOSE", NodeKind::Decision, NodeKind::Option),
        ProjectorRelationKind::Assumes => ("ASSUMES", NodeKind::Decision, NodeKind::Hypothesis),
        ProjectorRelationKind::Supports => ("SUPPORTS", NodeKind::Evidence, NodeKind::Hypothesis),
        ProjectorRelationKind::Refutes => ("REFUTES", NodeKind::Evidence, NodeKind::Hypothesis),
    }
}

fn decision_status_contract(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn hypothesis_status_contract(status: HypothesisStatus) -> &'static str {
    match status {
        HypothesisStatus::Open => "open",
        HypothesisStatus::Supported => "supported",
        HypothesisStatus::Refuted => "refuted",
    }
}

fn error_category(error: &HivemindError) -> &'static str {
    match error {
        HivemindError::Ledger(_) => "ledger",
        HivemindError::Projector(_) => "projector",
        HivemindError::Command(_) => "command",
        HivemindError::Query(_) => "query",
        HivemindError::Cli(_) => "cli",
    }
}
