use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type EventId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "decision.proposed")]
    DecisionProposed,
    #[serde(rename = "decision.accepted")]
    DecisionAccepted,
    #[serde(rename = "decision.rejected")]
    DecisionRejected,
    #[serde(rename = "decision.superseded")]
    DecisionSuperseded,
    #[serde(rename = "evidence.recorded")]
    EvidenceRecorded,
    #[serde(rename = "hypothesis.recorded")]
    HypothesisRecorded,
    #[serde(rename = "relation.added")]
    RelationAdded,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    #[default]
    Cli,
    Agent,
    Slack,
    Api,
}

impl EventSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Agent => "agent",
            Self::Slack => "slack",
            Self::Api => "api",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventProvenance {
    pub source: EventSource,
    pub source_ref: Option<String>,
}

impl EventProvenance {
    pub fn new(source: EventSource, source_ref: Option<String>) -> Self {
        Self { source, source_ref }
    }

    pub fn cli() -> Self {
        Self::new(EventSource::Cli, None)
    }

    pub fn agent(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Agent, Some(source_ref.into()))
    }

    pub fn slack(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Slack, Some(source_ref.into()))
    }

    pub fn api(source_ref: Option<String>) -> Self {
        Self::new(EventSource::Api, source_ref)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event_id: Option<EventId>,
    pub event_uuid: Uuid,
    pub correlation_id: Option<String>,
    pub causation_event_id: Option<EventId>,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub actor_id: String,
    #[serde(default)]
    pub source: EventSource,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub payload: Value,
    pub ts: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionProposedPayload {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    #[serde(default)]
    pub topic_keys: Vec<String>,
    #[serde(default)]
    pub option_ids: Vec<String>,
    pub chosen_option_id: Option<String>,
    #[serde(default)]
    pub hypothesis_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionIdPayload {
    pub decision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSupersededPayload {
    pub old_decision_id: String,
    pub new_decision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecordedPayload {
    pub evidence_id: String,
    pub content: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HypothesisRecordedPayload {
    pub hypothesis_id: String,
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationKind {
    #[serde(rename = "BASED_ON", alias = "based_on")]
    BasedOn,
    #[serde(rename = "HAS_OPTION", alias = "has_option")]
    HasOption,
    #[serde(rename = "CHOSE", alias = "chose")]
    Chose,
    #[serde(rename = "ASSUMES", alias = "assumes")]
    Assumes,
    #[serde(rename = "SUPPORTS", alias = "supports")]
    Supports,
    #[serde(rename = "REFUTES", alias = "refutes")]
    Refutes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationAddedPayload {
    pub relation: RelationKind,
    pub from_id: String,
    pub to_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventPayload {
    DecisionProposed(DecisionProposedPayload),
    DecisionAccepted(DecisionIdPayload),
    DecisionRejected(DecisionIdPayload),
    DecisionSuperseded(DecisionSupersededPayload),
    EvidenceRecorded(EvidenceRecordedPayload),
    HypothesisRecorded(HypothesisRecordedPayload),
    RelationAdded(RelationAddedPayload),
}

impl EventPayload {
    pub fn event_type(&self) -> EventType {
        match self {
            Self::DecisionProposed(_) => EventType::DecisionProposed,
            Self::DecisionAccepted(_) => EventType::DecisionAccepted,
            Self::DecisionRejected(_) => EventType::DecisionRejected,
            Self::DecisionSuperseded(_) => EventType::DecisionSuperseded,
            Self::EvidenceRecorded(_) => EventType::EvidenceRecorded,
            Self::HypothesisRecorded(_) => EventType::HypothesisRecorded,
            Self::RelationAdded(_) => EventType::RelationAdded,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventValidationError {
    #[error("event_id must be positive when present")]
    InvalidEventId,

    #[error("causation_event_id must be positive when present")]
    InvalidCausationEventId,

    #[error("{0} must not be empty")]
    EmptyField(&'static str),

    #[error("{0} contains an empty value")]
    EmptyListValue(&'static str),

    #[error("payload does not match event type {event_type:?}: {source}")]
    Payload {
        event_type: EventType,
        #[source]
        source: serde_json::Error,
    },
}

pub fn validate(event: &Event) -> std::result::Result<EventPayload, EventValidationError> {
    validate_common(event)?;

    match event.event_type {
        EventType::DecisionProposed => {
            let payload: DecisionProposedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_non_empty("payload.title", &payload.title)?;
            require_non_empty("payload.rationale", &payload.rationale)?;
            require_non_empty_values("payload.topic_keys", &payload.topic_keys)?;
            require_non_empty_values("payload.option_ids", &payload.option_ids)?;
            require_optional_non_empty(
                "payload.chosen_option_id",
                payload.chosen_option_id.as_deref(),
            )?;
            require_non_empty_values("payload.hypothesis_ids", &payload.hypothesis_ids)?;
            require_non_empty_values("payload.evidence_ids", &payload.evidence_ids)?;
            Ok(EventPayload::DecisionProposed(payload))
        }
        EventType::DecisionAccepted => {
            let payload: DecisionIdPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            Ok(EventPayload::DecisionAccepted(payload))
        }
        EventType::DecisionRejected => {
            let payload: DecisionIdPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            Ok(EventPayload::DecisionRejected(payload))
        }
        EventType::DecisionSuperseded => {
            let payload: DecisionSupersededPayload = parse_payload(event)?;
            require_non_empty("payload.old_decision_id", &payload.old_decision_id)?;
            require_non_empty("payload.new_decision_id", &payload.new_decision_id)?;
            Ok(EventPayload::DecisionSuperseded(payload))
        }
        EventType::EvidenceRecorded => {
            let payload: EvidenceRecordedPayload = parse_payload(event)?;
            require_non_empty("payload.evidence_id", &payload.evidence_id)?;
            require_non_empty("payload.content", &payload.content)?;
            require_optional_non_empty("payload.source", payload.source.as_deref())?;
            Ok(EventPayload::EvidenceRecorded(payload))
        }
        EventType::HypothesisRecorded => {
            let payload: HypothesisRecordedPayload = parse_payload(event)?;
            require_non_empty("payload.hypothesis_id", &payload.hypothesis_id)?;
            require_non_empty("payload.statement", &payload.statement)?;
            Ok(EventPayload::HypothesisRecorded(payload))
        }
        EventType::RelationAdded => {
            let payload: RelationAddedPayload = parse_payload(event)?;
            require_non_empty("payload.from_id", &payload.from_id)?;
            require_non_empty("payload.to_id", &payload.to_id)?;
            Ok(EventPayload::RelationAdded(payload))
        }
    }
}

fn validate_common(event: &Event) -> std::result::Result<(), EventValidationError> {
    if matches!(event.event_id, Some(0)) {
        return Err(EventValidationError::InvalidEventId);
    }

    if matches!(event.causation_event_id, Some(0)) {
        return Err(EventValidationError::InvalidCausationEventId);
    }

    require_non_empty("actor_id", &event.actor_id)?;
    require_optional_non_empty("source_ref", event.source_ref.as_deref())?;
    require_optional_non_empty("correlation_id", event.correlation_id.as_deref())
}

fn parse_payload<T>(event: &Event) -> std::result::Result<T, EventValidationError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(event.payload.clone()).map_err(|source| EventValidationError::Payload {
        event_type: event.event_type,
        source,
    })
}

fn require_non_empty(
    field: &'static str,
    value: &str,
) -> std::result::Result<(), EventValidationError> {
    if value.trim().is_empty() {
        Err(EventValidationError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn require_optional_non_empty(
    field: &'static str,
    value: Option<&str>,
) -> std::result::Result<(), EventValidationError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        Err(EventValidationError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn require_non_empty_values(
    field: &'static str,
    values: &[String],
) -> std::result::Result<(), EventValidationError> {
    if values.iter().any(|value| value.trim().is_empty()) {
        Err(EventValidationError::EmptyListValue(field))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const FIXTURES: &[(&str, &str, EventType)] = &[
        (
            include_str!("../schemas/v0/decision.proposed.json"),
            include_str!("../tests/fixtures/v0/decision.proposed.json"),
            EventType::DecisionProposed,
        ),
        (
            include_str!("../schemas/v0/decision.accepted.json"),
            include_str!("../tests/fixtures/v0/decision.accepted.json"),
            EventType::DecisionAccepted,
        ),
        (
            include_str!("../schemas/v0/decision.rejected.json"),
            include_str!("../tests/fixtures/v0/decision.rejected.json"),
            EventType::DecisionRejected,
        ),
        (
            include_str!("../schemas/v0/decision.superseded.json"),
            include_str!("../tests/fixtures/v0/decision.superseded.json"),
            EventType::DecisionSuperseded,
        ),
        (
            include_str!("../schemas/v0/evidence.recorded.json"),
            include_str!("../tests/fixtures/v0/evidence.recorded.json"),
            EventType::EvidenceRecorded,
        ),
        (
            include_str!("../schemas/v0/hypothesis.recorded.json"),
            include_str!("../tests/fixtures/v0/hypothesis.recorded.json"),
            EventType::HypothesisRecorded,
        ),
        (
            include_str!("../schemas/v0/relation.added.json"),
            include_str!("../tests/fixtures/v0/relation.added.json"),
            EventType::RelationAdded,
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
            serde_json::from_str(include_str!("../schemas/v0/decision.proposed.json")).unwrap();
        let mut fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/v0/decision.proposed.json"))
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
        let mut event: Event =
            serde_json::from_str(include_str!("../tests/fixtures/v0/evidence.recorded.json"))
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
}
