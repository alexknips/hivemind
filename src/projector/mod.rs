use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::ProjectorError;
use crate::events::{
    self, Event, EventId, EventPayload, RelationKind as EventRelationKind, TenantId,
};
use crate::ledger::EventLedger;
use crate::Result;

#[cfg(feature = "graph-kuzu")]
pub mod kuzu;
pub mod memory;
#[cfg(feature = "shared-backend-postgres")]
pub mod postgres;

pub type GraphProperties = BTreeMap<String, GraphValue>;
pub type GraphParams = BTreeMap<String, GraphValue>;
pub type GraphRow = BTreeMap<String, GraphValue>;

#[derive(Clone, Debug, PartialEq)]
pub enum GraphValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringList(Vec<String>),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Decision,
    DecisionRequest,
    Actor,
    Blocker,
    Evidence,
    Notification,
    Option,
    Hypothesis,
}

impl NodeKind {
    pub const ALL: [Self; 8] = [
        Self::Decision,
        Self::DecisionRequest,
        Self::Actor,
        Self::Blocker,
        Self::Evidence,
        Self::Notification,
        Self::Option,
        Self::Hypothesis,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::DecisionRequest => "DecisionRequest",
            Self::Actor => "Actor",
            Self::Blocker => "Blocker",
            Self::Evidence => "Evidence",
            Self::Notification => "Notification",
            Self::Option => "Option",
            Self::Hypothesis => "Hypothesis",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    ProposedBy,
    DecisionRequestedBy,
    DecisionRequestForDecision,
    DecisionRequestRequiredOwner,
    AcceptedBy,
    RejectedBy,
    Supersedes,
    BlockedActor,
    BlockerForDecision,
    BlockerRequiredOwner,
    NotificationForBlocker,
    NotificationRecipient,
    BasedOn,
    HasOption,
    Chose,
    Assumes,
    Supports,
    Refutes,
}

impl RelationKind {
    pub const ALL: [Self; 18] = [
        Self::ProposedBy,
        Self::DecisionRequestedBy,
        Self::DecisionRequestForDecision,
        Self::DecisionRequestRequiredOwner,
        Self::AcceptedBy,
        Self::RejectedBy,
        Self::Supersedes,
        Self::BlockedActor,
        Self::BlockerForDecision,
        Self::BlockerRequiredOwner,
        Self::NotificationForBlocker,
        Self::NotificationRecipient,
        Self::BasedOn,
        Self::HasOption,
        Self::Chose,
        Self::Assumes,
        Self::Supports,
        Self::Refutes,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::ProposedBy => "PROPOSED_BY",
            Self::DecisionRequestedBy => "DECISION_REQUESTED_BY",
            Self::DecisionRequestForDecision => "DECISION_REQUEST_FOR_DECISION",
            Self::DecisionRequestRequiredOwner => "DECISION_REQUEST_REQUIRED_OWNER",
            Self::AcceptedBy => "ACCEPTED_BY",
            Self::RejectedBy => "REJECTED_BY",
            Self::Supersedes => "SUPERSEDES",
            Self::BlockedActor => "BLOCKED_ACTOR",
            Self::BlockerForDecision => "BLOCKER_FOR_DECISION",
            Self::BlockerRequiredOwner => "BLOCKER_REQUIRED_OWNER",
            Self::NotificationForBlocker => "NOTIFICATION_FOR_BLOCKER",
            Self::NotificationRecipient => "NOTIFICATION_RECIPIENT",
            Self::BasedOn => "BASED_ON",
            Self::HasOption => "HAS_OPTION",
            Self::Chose => "CHOSE",
            Self::Assumes => "ASSUMES",
            Self::Supports => "SUPPORTS",
            Self::Refutes => "REFUTES",
        }
    }

    pub const fn endpoints(self) -> (NodeKind, NodeKind) {
        match self {
            Self::ProposedBy | Self::AcceptedBy | Self::RejectedBy => {
                (NodeKind::Decision, NodeKind::Actor)
            }
            Self::DecisionRequestedBy | Self::DecisionRequestRequiredOwner => {
                (NodeKind::DecisionRequest, NodeKind::Actor)
            }
            Self::DecisionRequestForDecision => (NodeKind::DecisionRequest, NodeKind::Decision),
            Self::Supersedes => (NodeKind::Decision, NodeKind::Decision),
            Self::BlockedActor | Self::BlockerRequiredOwner => (NodeKind::Blocker, NodeKind::Actor),
            Self::BlockerForDecision => (NodeKind::Blocker, NodeKind::Decision),
            Self::NotificationForBlocker => (NodeKind::Notification, NodeKind::Blocker),
            Self::NotificationRecipient => (NodeKind::Notification, NodeKind::Actor),
            Self::BasedOn => (NodeKind::Decision, NodeKind::Evidence),
            Self::HasOption | Self::Chose => (NodeKind::Decision, NodeKind::Option),
            Self::Assumes => (NodeKind::Decision, NodeKind::Hypothesis),
            Self::Supports | Self::Refutes => (NodeKind::Evidence, NodeKind::Hypothesis),
        }
    }
}

pub trait GraphView {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()>;

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()>;

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>>;

    fn wipe(&self) -> Result<()>;
}

pub fn project_event(graph: &impl GraphView, event: &Event) -> Result<()> {
    let payload = events::validate(event).map_err(projector_error)?;
    let event_origin = event_origin(event)?;
    let origin_properties = origin_properties(event, event_origin);

    upsert_actor(graph, &event.actor_id, &origin_properties)?;

    match payload {
        EventPayload::DecisionProposed(payload) => {
            let mut decision_properties = origin_properties.clone();
            decision_properties.insert(
                "title".to_owned(),
                GraphValue::String(payload.title.clone()),
            );
            decision_properties.insert(
                "rationale".to_owned(),
                GraphValue::String(payload.rationale.clone()),
            );
            decision_properties.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(payload.topic_keys.clone()),
            );
            graph.upsert_node(
                NodeKind::Decision,
                &payload.decision_id,
                &decision_properties,
            )?;
            graph.upsert_edge(
                RelationKind::ProposedBy,
                &payload.decision_id,
                &event.actor_id,
                &origin_properties,
            )?;

            for option_id in &payload.option_ids {
                graph.upsert_node(NodeKind::Option, option_id, &origin_properties)?;
                graph.upsert_edge(
                    RelationKind::HasOption,
                    &payload.decision_id,
                    option_id,
                    &origin_properties,
                )?;
            }

            if let Some(chosen_option_id) = &payload.chosen_option_id {
                graph.upsert_node(NodeKind::Option, chosen_option_id, &origin_properties)?;
                graph.upsert_edge(
                    RelationKind::Chose,
                    &payload.decision_id,
                    chosen_option_id,
                    &origin_properties,
                )?;
            }

            for hypothesis_id in &payload.hypothesis_ids {
                graph.upsert_edge(
                    RelationKind::Assumes,
                    &payload.decision_id,
                    hypothesis_id,
                    &origin_properties,
                )?;
            }

            for evidence_id in &payload.evidence_ids {
                graph.upsert_edge(
                    RelationKind::BasedOn,
                    &payload.decision_id,
                    evidence_id,
                    &origin_properties,
                )?;
            }
        }
        EventPayload::DecisionRequested(payload) => {
            let request_id = event.event_uuid.to_string();
            let mut request_properties = origin_properties.clone();
            request_properties.insert(
                "decision_id".to_owned(),
                optional_string_value(payload.decision_id.as_deref()),
            );
            request_properties.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(payload.topic_keys.clone()),
            );
            request_properties.insert(
                "reason".to_owned(),
                GraphValue::String(payload.reason.clone()),
            );
            request_properties.insert(
                "priority".to_owned(),
                GraphValue::String(payload.priority.as_str().to_owned()),
            );
            request_properties.insert(
                "required_owner_id".to_owned(),
                optional_string_value(payload.required_owner_id.as_deref()),
            );
            request_properties.insert(
                "authority_class".to_owned(),
                GraphValue::String(payload.authority_class.clone()),
            );
            request_properties.insert(
                "requested_by".to_owned(),
                GraphValue::String(payload.requested_by.clone()),
            );
            request_properties.insert(
                "client_request_id".to_owned(),
                GraphValue::String(payload.client_request_id.clone()),
            );
            graph.upsert_node(NodeKind::DecisionRequest, &request_id, &request_properties)?;

            upsert_actor(graph, &payload.requested_by, &origin_properties)?;
            graph.upsert_edge(
                RelationKind::DecisionRequestedBy,
                &request_id,
                &payload.requested_by,
                &origin_properties,
            )?;

            if let Some(required_owner_id) = &payload.required_owner_id {
                upsert_actor(graph, required_owner_id, &origin_properties)?;
                graph.upsert_edge(
                    RelationKind::DecisionRequestRequiredOwner,
                    &request_id,
                    required_owner_id,
                    &origin_properties,
                )?;
            }

            if let Some(decision_id) = &payload.decision_id {
                ensure_node_reference(graph, NodeKind::Decision, decision_id, &origin_properties)?;
                graph.upsert_edge(
                    RelationKind::DecisionRequestForDecision,
                    &request_id,
                    decision_id,
                    &origin_properties,
                )?;
            }
        }
        EventPayload::DecisionAccepted(payload) => graph.upsert_edge(
            RelationKind::AcceptedBy,
            &payload.decision_id,
            &event.actor_id,
            &origin_properties,
        )?,
        EventPayload::DecisionRejected(payload) => graph.upsert_edge(
            RelationKind::RejectedBy,
            &payload.decision_id,
            &event.actor_id,
            &origin_properties,
        )?,
        EventPayload::DecisionSuperseded(payload) => graph.upsert_edge(
            RelationKind::Supersedes,
            &payload.new_decision_id,
            &payload.old_decision_id,
            &origin_properties,
        )?,
        EventPayload::EvidenceRecorded(payload) => {
            let mut evidence_properties = origin_properties.clone();
            evidence_properties.insert(
                "content".to_owned(),
                GraphValue::String(payload.content.clone()),
            );
            graph.upsert_node(
                NodeKind::Evidence,
                &payload.evidence_id,
                &evidence_properties,
            )?;
        }
        EventPayload::HypothesisRecorded(payload) => {
            let mut hypothesis_properties = origin_properties.clone();
            hypothesis_properties.insert(
                "statement".to_owned(),
                GraphValue::String(payload.statement.clone()),
            );
            graph.upsert_node(
                NodeKind::Hypothesis,
                &payload.hypothesis_id,
                &hypothesis_properties,
            )?;
        }
        EventPayload::RelationAdded(payload) => graph.upsert_edge(
            relation_kind(payload.relation),
            &payload.from_id,
            &payload.to_id,
            &origin_properties,
        )?,
        EventPayload::BlockerReported(payload) => {
            let mut blocker_properties = origin_properties.clone();
            blocker_properties.insert(
                "blocked_actor_id".to_owned(),
                GraphValue::String(payload.blocked_actor_id.clone()),
            );
            blocker_properties.insert(
                "decision_id".to_owned(),
                optional_string_value(payload.decision_id.as_deref()),
            );
            blocker_properties.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(payload.topic_keys.clone()),
            );
            blocker_properties.insert(
                "blocked_ref".to_owned(),
                GraphValue::String(payload.blocked_ref.clone()),
            );
            blocker_properties.insert(
                "blocked_ref_type".to_owned(),
                GraphValue::String(payload.blocked_ref_type.clone()),
            );
            blocker_properties.insert(
                "reason".to_owned(),
                GraphValue::String(payload.reason.clone()),
            );
            blocker_properties.insert(
                "priority".to_owned(),
                GraphValue::String(payload.priority.as_str().to_owned()),
            );
            blocker_properties.insert(
                "last_progress_at".to_owned(),
                payload
                    .last_progress_at
                    .map(|timestamp| GraphValue::String(timestamp.to_rfc3339()))
                    .unwrap_or(GraphValue::Null),
            );
            blocker_properties.insert(
                "required_owner_id".to_owned(),
                optional_string_value(payload.required_owner_id.as_deref()),
            );
            blocker_properties.insert("reported_at".to_owned(), event_timestamp(event));
            blocker_properties.insert(
                "reported_event_origin".to_owned(),
                GraphValue::Int(event_origin),
            );
            graph.upsert_node(NodeKind::Blocker, &payload.blocker_id, &blocker_properties)?;

            upsert_actor(graph, &payload.blocked_actor_id, &origin_properties)?;
            graph.upsert_edge(
                RelationKind::BlockedActor,
                &payload.blocker_id,
                &payload.blocked_actor_id,
                &origin_properties,
            )?;

            if let Some(required_owner_id) = &payload.required_owner_id {
                upsert_actor(graph, required_owner_id, &origin_properties)?;
                graph.upsert_edge(
                    RelationKind::BlockerRequiredOwner,
                    &payload.blocker_id,
                    required_owner_id,
                    &origin_properties,
                )?;
            }

            if let Some(decision_id) = &payload.decision_id {
                ensure_node_reference(graph, NodeKind::Decision, decision_id, &origin_properties)?;
                graph.upsert_edge(
                    RelationKind::BlockerForDecision,
                    &payload.blocker_id,
                    decision_id,
                    &origin_properties,
                )?;
            }
        }
        EventPayload::BlockerResolved(payload) => {
            let mut blocker_properties = origin_properties.clone();
            blocker_properties.insert("resolved_at".to_owned(), event_timestamp(event));
            blocker_properties.insert(
                "resolution_event_id".to_owned(),
                payload
                    .resolution_event_id
                    .and_then(|id| i64::try_from(id).ok())
                    .map_or(GraphValue::Null, GraphValue::Int),
            );
            blocker_properties.insert(
                "resolution_reason".to_owned(),
                payload
                    .resolution_reason
                    .map_or(GraphValue::Null, GraphValue::String),
            );
            blocker_properties.insert(
                "resolved_event_origin".to_owned(),
                GraphValue::Int(event_origin),
            );
            graph.upsert_node(NodeKind::Blocker, &payload.blocker_id, &blocker_properties)?;
        }
        EventPayload::NotificationSent(payload) => {
            let notification_id = event.event_uuid.to_string();
            let mut notification_properties = origin_properties.clone();
            notification_properties.insert(
                "blocker_id".to_owned(),
                GraphValue::String(payload.blocker_id.clone()),
            );
            notification_properties.insert(
                "recipient_actor_id".to_owned(),
                GraphValue::String(payload.recipient_actor_id.clone()),
            );
            notification_properties.insert(
                "channel".to_owned(),
                GraphValue::String(payload.channel.clone()),
            );
            notification_properties.insert(
                "threshold_rule".to_owned(),
                GraphValue::String(payload.threshold_rule.clone()),
            );
            notification_properties.insert(
                "source_event_ids".to_owned(),
                GraphValue::StringList(
                    payload
                        .source_event_ids
                        .iter()
                        .map(|event_id| event_id.to_string())
                        .collect(),
                ),
            );
            notification_properties.insert(
                "dedupe_key".to_owned(),
                GraphValue::String(payload.dedupe_key.clone()),
            );
            notification_properties.insert(
                "sent_at".to_owned(),
                GraphValue::String(payload.sent_at.to_rfc3339()),
            );
            graph.upsert_node(
                NodeKind::Notification,
                &notification_id,
                &notification_properties,
            )?;

            ensure_node_reference(
                graph,
                NodeKind::Blocker,
                &payload.blocker_id,
                &origin_properties,
            )?;
            graph.upsert_edge(
                RelationKind::NotificationForBlocker,
                &notification_id,
                &payload.blocker_id,
                &origin_properties,
            )?;

            upsert_actor(graph, &payload.recipient_actor_id, &origin_properties)?;
            graph.upsert_edge(
                RelationKind::NotificationRecipient,
                &notification_id,
                &payload.recipient_actor_id,
                &origin_properties,
            )?;
        }
        EventPayload::NotificationAcknowledged(payload) => {
            let mut notification_properties = origin_properties.clone();
            notification_properties.insert(
                "ack_at".to_owned(),
                GraphValue::String(payload.ack_at.to_rfc3339()),
            );
            notification_properties.insert(
                "snooze_until".to_owned(),
                payload
                    .snooze_until
                    .map(|value| GraphValue::String(value.to_rfc3339()))
                    .unwrap_or(GraphValue::Null),
            );
            graph.upsert_node(
                NodeKind::Notification,
                &payload.notification_id,
                &notification_properties,
            )?;
        }
    }

    Ok(())
}

pub fn project_from_ledger(
    ledger: &impl EventLedger,
    graph: &impl GraphView,
    offset: EventId,
) -> Result<()> {
    ledger.replay_from(offset, &mut |event| project_event(graph, event))
}

pub fn project_from_ledger_for_tenant(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    graph: &impl GraphView,
    offset: EventId,
) -> Result<()> {
    ledger.replay_from_for_tenant(tenant_id, offset, &mut |event| project_event(graph, event))
}

pub fn rebuild_graph(ledger: &impl EventLedger, graph: &impl GraphView) -> Result<()> {
    graph.wipe()?;
    project_from_ledger(ledger, graph, 0)
}

pub fn rebuild_graph_for_tenant(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    graph: &impl GraphView,
) -> Result<()> {
    graph.wipe()?;
    project_from_ledger_for_tenant(ledger, tenant_id, graph, 0)
}

fn upsert_actor(
    graph: &impl GraphView,
    actor_id: &str,
    properties: &GraphProperties,
) -> Result<()> {
    graph.upsert_node(NodeKind::Actor, actor_id, properties)
}

fn ensure_node_reference(
    graph: &impl GraphView,
    kind: NodeKind,
    id: &str,
    properties: &GraphProperties,
) -> Result<()> {
    let table = kind.table_name();
    let rows = graph.query(
        &format!("MATCH (node:`{table}` {{id: $id}}) RETURN node.id AS id LIMIT 1;"),
        &GraphParams::from([("id".to_owned(), GraphValue::String(id.to_owned()))]),
    )?;
    if rows.is_empty() {
        graph.upsert_node(kind, id, properties)?;
    }
    Ok(())
}

fn optional_string_value(value: Option<&str>) -> GraphValue {
    value.map_or(GraphValue::Null, |value| {
        GraphValue::String(value.to_owned())
    })
}

fn origin_properties(event: &Event, event_origin: i64) -> GraphProperties {
    let mut properties = GraphProperties::from([
        (
            "tenant_id".to_owned(),
            GraphValue::String(event.tenant_id.as_str().to_owned()),
        ),
        ("event_origin".to_owned(), GraphValue::Int(event_origin)),
        (
            "source".to_owned(),
            GraphValue::String(event.source.as_str().to_owned()),
        ),
    ]);
    properties.insert(
        "source_ref".to_owned(),
        event
            .source_ref
            .as_ref()
            .map_or(GraphValue::Null, |source_ref| {
                GraphValue::String(source_ref.clone())
            }),
    );
    properties
}

fn event_origin(event: &Event) -> Result<i64> {
    let event_id = event
        .event_id
        .ok_or_else(|| projector_error("event_id is required before projection"))?;
    i64::try_from(event_id)
        .map_err(|error| projector_error(format!("event_id out of range: {error}")).into())
}

fn event_timestamp(event: &Event) -> GraphValue {
    event
        .ts
        .map(|ts| GraphValue::String(ts.to_rfc3339()))
        .unwrap_or(GraphValue::Null)
}

fn relation_kind(kind: EventRelationKind) -> RelationKind {
    match kind {
        EventRelationKind::BasedOn => RelationKind::BasedOn,
        EventRelationKind::HasOption => RelationKind::HasOption,
        EventRelationKind::Chose => RelationKind::Chose,
        EventRelationKind::Assumes => RelationKind::Assumes,
        EventRelationKind::Supports => RelationKind::Supports,
        EventRelationKind::Refutes => RelationKind::Refutes,
    }
}

fn projector_error(error: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(error.to_string())
}

#[cfg(test)]
mod tests;
