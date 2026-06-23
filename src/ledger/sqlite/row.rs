use chrono::{DateTime, Utc};
use rusqlite::Row;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType, TenantId};
use crate::Result;

use super::super::backend_error::storage_error;

pub(super) struct StoredEvent {
    pub tenant_id: String,
    pub event_uuid: String,
    pub event_type: &'static str,
    pub actor_id: String,
    pub source: &'static str,
    pub source_ref: Option<String>,
    pub correlation_id: Option<String>,
    pub causation_event_id: Option<i64>,
    pub payload: String,
    pub ts: String,
}

impl StoredEvent {
    pub fn from_event(event: Event) -> Result<Self> {
        let payload = serde_json::to_string(&event.payload).map_err(storage_error)?;
        let ts = event.ts.unwrap_or_else(Utc::now).to_rfc3339();
        let causation_event_id = event.causation_event_id.map(|id| id as i64);

        Ok(Self {
            tenant_id: event.tenant_id.as_str().to_owned(),
            event_uuid: event.event_uuid.to_string(),
            event_type: event_type_as_str(event.event_type),
            actor_id: event.actor_id,
            source: event.source.as_str(),
            source_ref: event.source_ref,
            correlation_id: event.correlation_id,
            causation_event_id,
            payload,
            ts,
        })
    }
}

pub(super) fn event_from_row(row: &Row<'_>) -> Result<Event> {
    let tenant_id_raw: String = row.get("tenant_id").map_err(storage_error)?;
    let event_id_raw: i64 = row.get("event_id").map_err(storage_error)?;
    let event_uuid_raw: String = row.get("event_uuid").map_err(storage_error)?;
    let event_type_raw: String = row.get("type").map_err(storage_error)?;
    let actor_id: String = row.get("actor_id").map_err(storage_error)?;
    let source_raw: String = row.get("source").map_err(storage_error)?;
    let source_ref: Option<String> = row.get("source_ref").map_err(storage_error)?;
    let correlation_id: Option<String> = row.get("correlation_id").map_err(storage_error)?;
    let causation_event_id_raw: Option<i64> =
        row.get("causation_event_id").map_err(storage_error)?;
    let payload_raw: String = row.get("payload").map_err(storage_error)?;
    let ts_raw: String = row.get("ts").map_err(storage_error)?;

    let event_id = u64::try_from(event_id_raw)
        .map_err(|error| storage_error(format!("invalid event_id in row: {error}")))?;
    let tenant_id = TenantId::new(tenant_id_raw)
        .map_err(|error| storage_error(format!("invalid tenant_id in row: {error}")))?;
    let event_uuid = Uuid::parse_str(&event_uuid_raw)
        .map_err(|error| storage_error(format!("invalid event_uuid in row: {error}")))?;
    let event_type = parse_event_type(&event_type_raw)?;
    let source = parse_event_source(&source_raw)?;
    let causation_event_id = causation_event_id_raw
        .map(|id| {
            u64::try_from(id).map_err(|error| {
                storage_error(format!("invalid causation_event_id in row: {error}"))
            })
        })
        .transpose()?;
    let payload = serde_json::from_str(&payload_raw).map_err(storage_error)?;
    let ts = DateTime::parse_from_rfc3339(&ts_raw)
        .map_err(|error| storage_error(format!("invalid timestamp in row: {error}")))?
        .with_timezone(&Utc);

    Ok(Event {
        tenant_id,
        event_id: Some(event_id),
        event_uuid,
        correlation_id,
        causation_event_id,
        event_type,
        actor_id,
        source,
        source_ref,
        payload,
        ts: Some(ts),
    })
}

fn event_type_as_str(event_type: EventType) -> &'static str {
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
        EventType::IngestBatchReceived => "ingest.batch_received",
        EventType::IngestBatchClassified => "ingest.batch_classified",
        EventType::DecisionScored => "decision.scored",
    }
}

fn parse_event_type(value: &str) -> Result<EventType> {
    match value {
        "decision.proposed" => Ok(EventType::DecisionProposed),
        "decision.requested" => Ok(EventType::DecisionRequested),
        "decision.accepted" => Ok(EventType::DecisionAccepted),
        "decision.rejected" => Ok(EventType::DecisionRejected),
        "decision.superseded" => Ok(EventType::DecisionSuperseded),
        "evidence.recorded" => Ok(EventType::EvidenceRecorded),
        "hypothesis.recorded" => Ok(EventType::HypothesisRecorded),
        "relation.added" => Ok(EventType::RelationAdded),
        "blocker.reported" => Ok(EventType::BlockerReported),
        "blocker.resolved" => Ok(EventType::BlockerResolved),
        "notification.sent" => Ok(EventType::NotificationSent),
        "notification.acknowledged" => Ok(EventType::NotificationAcknowledged),
        "ingest.batch_received" => Ok(EventType::IngestBatchReceived),
        "ingest.batch_classified" => Ok(EventType::IngestBatchClassified),
        "decision.scored" => Ok(EventType::DecisionScored),
        other => Err(storage_error(format!("unknown event type in row: {other}")).into()),
    }
}

fn parse_event_source(value: &str) -> Result<EventSource> {
    match value {
        "cli" => Ok(EventSource::Cli),
        "agent" => Ok(EventSource::Agent),
        "human" => Ok(EventSource::Human),
        "slack" => Ok(EventSource::Slack),
        "document" => Ok(EventSource::Document),
        "api" => Ok(EventSource::Api),
        other => Err(storage_error(format!("unknown event source in row: {other}")).into()),
    }
}
