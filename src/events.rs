use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type EventId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "decision.proposed")]
    DecisionProposed,
    #[serde(rename = "decision.requested")]
    DecisionRequested,
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
    #[serde(rename = "blocker.reported")]
    BlockerReported,
    #[serde(rename = "blocker.resolved")]
    BlockerResolved,
    #[serde(rename = "notification.sent")]
    NotificationSent,
    #[serde(rename = "notification.acknowledged")]
    NotificationAcknowledged,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    #[default]
    Cli,
    Agent,
    Human,
    Slack,
    Document,
    Api,
}

impl EventSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Agent => "agent",
            Self::Human => "human",
            Self::Slack => "slack",
            Self::Document => "document",
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

    pub fn human(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Human, Some(source_ref.into()))
    }

    pub fn slack(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Slack, Some(source_ref.into()))
    }

    pub fn document(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Document, Some(source_ref.into()))
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
pub struct DecisionRejectedPayload {
    pub decision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSupersededPayload {
    pub old_decision_id: String,
    pub new_decision_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DecisionBlockerPriority {
    #[serde(rename = "P0", alias = "p0")]
    P0,
    #[serde(rename = "P1", alias = "p1")]
    P1,
    #[serde(rename = "P2", alias = "p2")]
    P2,
    #[serde(rename = "P3", alias = "p3")]
    P3,
    #[serde(rename = "P4", alias = "p4")]
    P4,
}

impl DecisionBlockerPriority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
            Self::P4 => "P4",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "p0" => Some(Self::P0),
            "p1" => Some(Self::P1),
            "p2" => Some(Self::P2),
            "p3" => Some(Self::P3),
            "p4" => Some(Self::P4),
            _ => None,
        }
    }
}

pub type BlockerPriority = DecisionBlockerPriority;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequestedPayload {
    pub topic_keys: Vec<String>,
    pub decision_id: Option<String>,
    pub reason: String,
    pub priority: DecisionBlockerPriority,
    pub required_owner_id: Option<String>,
    pub authority_class: String,
    pub requested_by: String,
    pub client_request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockerReportedPayload {
    pub blocker_id: String,
    pub blocked_actor_id: String,
    pub decision_id: Option<String>,
    #[serde(default)]
    pub topic_keys: Vec<String>,
    pub blocked_ref: String,
    pub blocked_ref_type: String,
    pub reason: String,
    pub priority: DecisionBlockerPriority,
    pub last_progress_at: Option<DateTime<Utc>>,
    pub required_owner_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationSentPayload {
    pub blocker_id: String,
    pub recipient_actor_id: String,
    pub channel: String,
    pub threshold_rule: String,
    pub source_event_ids: Vec<EventId>,
    pub dedupe_key: String,
    pub sent_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockerResolvedPayload {
    pub blocker_id: String,
    pub resolution_event_id: Option<EventId>,
    pub resolution_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationAcknowledgedPayload {
    pub notification_id: String,
    pub ack_at: DateTime<Utc>,
    pub snooze_until: Option<DateTime<Utc>>,
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
    DecisionRequested(DecisionRequestedPayload),
    DecisionAccepted(DecisionIdPayload),
    DecisionRejected(DecisionRejectedPayload),
    DecisionSuperseded(DecisionSupersededPayload),
    EvidenceRecorded(EvidenceRecordedPayload),
    HypothesisRecorded(HypothesisRecordedPayload),
    RelationAdded(RelationAddedPayload),
    BlockerReported(BlockerReportedPayload),
    BlockerResolved(BlockerResolvedPayload),
    NotificationSent(NotificationSentPayload),
    NotificationAcknowledged(NotificationAcknowledgedPayload),
}

impl EventPayload {
    pub fn event_type(&self) -> EventType {
        match self {
            Self::DecisionProposed(_) => EventType::DecisionProposed,
            Self::DecisionRequested(_) => EventType::DecisionRequested,
            Self::DecisionAccepted(_) => EventType::DecisionAccepted,
            Self::DecisionRejected(_) => EventType::DecisionRejected,
            Self::DecisionSuperseded(_) => EventType::DecisionSuperseded,
            Self::EvidenceRecorded(_) => EventType::EvidenceRecorded,
            Self::HypothesisRecorded(_) => EventType::HypothesisRecorded,
            Self::RelationAdded(_) => EventType::RelationAdded,
            Self::BlockerReported(_) => EventType::BlockerReported,
            Self::BlockerResolved(_) => EventType::BlockerResolved,
            Self::NotificationSent(_) => EventType::NotificationSent,
            Self::NotificationAcknowledged(_) => EventType::NotificationAcknowledged,
        }
    }

    pub fn to_value(&self) -> std::result::Result<Value, serde_json::Error> {
        match self {
            Self::DecisionProposed(payload) => serde_json::to_value(payload),
            Self::DecisionRequested(payload) => serde_json::to_value(payload),
            Self::DecisionAccepted(payload) => serde_json::to_value(payload),
            Self::DecisionRejected(payload) => serde_json::to_value(payload),
            Self::DecisionSuperseded(payload) => serde_json::to_value(payload),
            Self::EvidenceRecorded(payload) => serde_json::to_value(payload),
            Self::HypothesisRecorded(payload) => serde_json::to_value(payload),
            Self::RelationAdded(payload) => serde_json::to_value(payload),
            Self::BlockerReported(payload) => serde_json::to_value(payload),
            Self::BlockerResolved(payload) => serde_json::to_value(payload),
            Self::NotificationSent(payload) => serde_json::to_value(payload),
            Self::NotificationAcknowledged(payload) => serde_json::to_value(payload),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEnvelope {
    event_type: EventType,
    payload: EventPayload,
}

impl EventEnvelope {
    pub fn new(payload: EventPayload) -> Self {
        let event_type = payload.event_type();
        Self {
            event_type,
            payload,
        }
    }

    pub fn event_type(&self) -> EventType {
        self.event_type
    }

    pub fn payload(&self) -> &EventPayload {
        &self.payload
    }

    pub fn into_payload(self) -> EventPayload {
        self.payload
    }
}

impl From<EventPayload> for EventEnvelope {
    fn from(payload: EventPayload) -> Self {
        Self::new(payload)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventBuildError {
    #[error("payload for event type {event_type:?} could not be serialized: {source}")]
    PayloadSerialization {
        event_type: EventType,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBuilder {
    event_id: Option<EventId>,
    event_uuid: Uuid,
    correlation_id: Option<String>,
    causation_event_id: Option<EventId>,
    actor_id: String,
    source: EventSource,
    source_ref: Option<String>,
    envelope: EventEnvelope,
    ts: Option<DateTime<Utc>>,
}

impl EventBuilder {
    pub fn new(
        event_uuid: Uuid,
        actor_id: impl Into<String>,
        payload: impl Into<EventEnvelope>,
    ) -> Self {
        Self {
            event_id: None,
            event_uuid,
            correlation_id: None,
            causation_event_id: None,
            actor_id: actor_id.into(),
            source: EventSource::default(),
            source_ref: None,
            envelope: payload.into(),
            ts: None,
        }
    }

    pub fn event_id(mut self, event_id: Option<EventId>) -> Self {
        self.event_id = event_id;
        self
    }

    pub fn correlation_id(mut self, correlation_id: Option<String>) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    pub fn causation_event_id(mut self, causation_event_id: Option<EventId>) -> Self {
        self.causation_event_id = causation_event_id;
        self
    }

    pub fn provenance(mut self, provenance: EventProvenance) -> Self {
        self.source = provenance.source;
        self.source_ref = provenance.source_ref;
        self
    }

    pub fn timestamp(mut self, ts: Option<DateTime<Utc>>) -> Self {
        self.ts = ts;
        self
    }

    pub fn build(self) -> std::result::Result<Event, EventBuildError> {
        let event_type = self.envelope.event_type();
        let payload = self
            .envelope
            .into_payload()
            .to_value()
            .map_err(|source| EventBuildError::PayloadSerialization { event_type, source })?;

        Ok(Event {
            event_id: self.event_id,
            event_uuid: self.event_uuid,
            correlation_id: self.correlation_id,
            causation_event_id: self.causation_event_id,
            event_type,
            actor_id: self.actor_id,
            source: self.source,
            source_ref: self.source_ref,
            payload,
            ts: self.ts,
        })
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

    #[error("{0} must contain at least one value")]
    EmptyList(&'static str),

    #[error("{0} contains a non-positive event id")]
    InvalidEventIdListValue(&'static str),

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
        EventType::DecisionRequested => {
            require_event_provenance(event)?;
            let payload: DecisionRequestedPayload = parse_payload(event)?;
            require_non_empty_list("payload.topic_keys", &payload.topic_keys)?;
            require_non_empty_values("payload.topic_keys", &payload.topic_keys)?;
            require_optional_non_empty("payload.decision_id", payload.decision_id.as_deref())?;
            require_non_empty("payload.reason", &payload.reason)?;
            require_optional_non_empty(
                "payload.required_owner_id",
                payload.required_owner_id.as_deref(),
            )?;
            require_non_empty("payload.authority_class", &payload.authority_class)?;
            require_non_empty("payload.requested_by", &payload.requested_by)?;
            require_non_empty("payload.client_request_id", &payload.client_request_id)?;
            Ok(EventPayload::DecisionRequested(payload))
        }
        EventType::DecisionAccepted => {
            let payload: DecisionIdPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            Ok(EventPayload::DecisionAccepted(payload))
        }
        EventType::DecisionRejected => {
            let payload: DecisionRejectedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_optional_non_empty("payload.reason", payload.reason.as_deref())?;
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
        EventType::BlockerReported => {
            require_event_provenance(event)?;
            let payload: BlockerReportedPayload = parse_payload(event)?;
            require_non_empty("payload.blocker_id", &payload.blocker_id)?;
            require_non_empty("payload.blocked_actor_id", &payload.blocked_actor_id)?;
            require_optional_non_empty("payload.decision_id", payload.decision_id.as_deref())?;
            require_non_empty_values("payload.topic_keys", &payload.topic_keys)?;
            if payload.decision_id.is_none() && payload.topic_keys.is_empty() {
                return Err(EventValidationError::EmptyField(
                    "payload.decision_id_or_topic_keys",
                ));
            }
            require_non_empty("payload.blocked_ref", &payload.blocked_ref)?;
            require_non_empty("payload.blocked_ref_type", &payload.blocked_ref_type)?;
            require_non_empty("payload.reason", &payload.reason)?;
            require_optional_non_empty(
                "payload.required_owner_id",
                payload.required_owner_id.as_deref(),
            )?;
            Ok(EventPayload::BlockerReported(payload))
        }
        EventType::BlockerResolved => {
            require_event_provenance(event)?;
            let payload: BlockerResolvedPayload = parse_payload(event)?;
            require_non_empty("payload.blocker_id", &payload.blocker_id)?;
            require_optional_non_empty(
                "payload.resolution_reason",
                payload.resolution_reason.as_deref(),
            )?;
            if payload.resolution_event_id.is_none() && payload.resolution_reason.is_none() {
                return Err(EventValidationError::EmptyField(
                    "payload.resolution_event_id_or_resolution_reason",
                ));
            }
            Ok(EventPayload::BlockerResolved(payload))
        }
        EventType::NotificationSent => {
            require_event_provenance(event)?;
            let payload: NotificationSentPayload = parse_payload(event)?;
            require_non_empty("payload.blocker_id", &payload.blocker_id)?;
            require_non_empty("payload.recipient_actor_id", &payload.recipient_actor_id)?;
            require_non_empty("payload.channel", &payload.channel)?;
            require_non_empty("payload.threshold_rule", &payload.threshold_rule)?;
            require_non_empty_event_ids("payload.source_event_ids", &payload.source_event_ids)?;
            require_non_empty("payload.dedupe_key", &payload.dedupe_key)?;
            Ok(EventPayload::NotificationSent(payload))
        }
        EventType::NotificationAcknowledged => {
            require_event_provenance(event)?;
            let payload: NotificationAcknowledgedPayload = parse_payload(event)?;
            require_non_empty("payload.notification_id", &payload.notification_id)?;
            Ok(EventPayload::NotificationAcknowledged(payload))
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

fn require_event_provenance(event: &Event) -> std::result::Result<(), EventValidationError> {
    require_present_non_empty("source_ref", event.source_ref.as_deref())?;
    require_present_non_empty("correlation_id", event.correlation_id.as_deref())
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

fn require_present_non_empty(
    field: &'static str,
    value: Option<&str>,
) -> std::result::Result<(), EventValidationError> {
    match value {
        Some(value) => require_non_empty(field, value),
        None => Err(EventValidationError::EmptyField(field)),
    }
}

fn require_non_empty_list(
    field: &'static str,
    values: &[String],
) -> std::result::Result<(), EventValidationError> {
    if values.is_empty() {
        Err(EventValidationError::EmptyList(field))
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

fn require_non_empty_event_ids(
    field: &'static str,
    values: &[EventId],
) -> std::result::Result<(), EventValidationError> {
    if values.is_empty() {
        return Err(EventValidationError::EmptyList(field));
    }
    if values.contains(&0) {
        return Err(EventValidationError::InvalidEventIdListValue(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
