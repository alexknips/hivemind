use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::ProjectorError;
use crate::events::{
    self, CaptureItem, Event, EventId, EventPayload, RelationKind as EventRelationKind, TenantId,
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
    /// Actor that participated in the session that produced this decision.
    ParticipatedBy,
    /// Actor that initiated the session that produced this decision.
    InitiatedBy,
}

impl RelationKind {
    pub const ALL: [Self; 20] = [
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
        Self::ParticipatedBy,
        Self::InitiatedBy,
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
            Self::ParticipatedBy => "PARTICIPATED_BY",
            Self::InitiatedBy => "INITIATED_BY",
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
            Self::ParticipatedBy | Self::InitiatedBy => (NodeKind::Decision, NodeKind::Actor),
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
            decision_properties.insert(
                "expressed_confidence".to_owned(),
                payload
                    .expressed_confidence
                    .as_deref()
                    .map_or(GraphValue::Null, |c| GraphValue::String(c.to_owned())),
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
        EventPayload::IngestBatchReceived(_) => {
            // Raw transcript batches are ledger-only; they do not project to the graph.
        }
        EventPayload::IngestBatchClassified(payload) => {
            for (idx, capture) in payload.captures.iter().enumerate() {
                let node_id = format!("capture:{event_origin}:{idx}");
                project_capture(graph, capture, &node_id, &origin_properties)?;
            }
        }
        EventPayload::DecisionScored(payload) => {
            // Annotate the capture node with per-dimension Quality scores and
            // Importance factors. Upsert merges onto the existing node without
            // overwriting any decision fields.
            let mut props = origin_properties.clone();
            let dims = &payload.quality_dims;
            props.insert(
                "score_framing".to_owned(),
                GraphValue::Float(dims.framing.score),
            );
            props.insert(
                "score_alternatives".to_owned(),
                GraphValue::Float(dims.alternatives.score),
            );
            props.insert(
                "score_information".to_owned(),
                GraphValue::Float(dims.information.score),
            );
            props.insert(
                "score_reasoning".to_owned(),
                GraphValue::Float(dims.reasoning.score),
            );
            props.insert(
                "score_values_tradeoffs".to_owned(),
                GraphValue::Float(dims.values_tradeoffs.score),
            );
            props.insert(
                "score_bias_exposure".to_owned(),
                GraphValue::Float(dims.bias_exposure.score),
            );
            props.insert(
                "score_calibration".to_owned(),
                GraphValue::Float(dims.calibration.score),
            );
            props.insert(
                "score_weight_version".to_owned(),
                GraphValue::String(payload.weight_version.clone()),
            );
            let imp = &payload.importance;
            props.insert(
                "importance_stakes".to_owned(),
                GraphValue::Float(imp.stakes),
            );
            props.insert(
                "importance_irreversibility".to_owned(),
                GraphValue::Float(imp.irreversibility),
            );
            props.insert(
                "importance_actionability".to_owned(),
                GraphValue::Float(imp.actionability),
            );
            graph.upsert_node(NodeKind::Decision, &payload.capture_node_id, &props)?;
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

/// Project a slice of captures in-memory and return the resulting graph as
/// flat vectors for evaluation. Each item in `id_captures` is
/// `(stable_node_id, &CaptureItem)` — the caller assigns IDs. Returns
/// `(nodes, edges)` where nodes are `(NodeKind, node_id, text)` and edges
/// are `(RelationKind, from_id, to_id)`. Notification nodes are excluded.
pub fn project_captures_in_memory(
    id_captures: &[(&str, &CaptureItem)],
) -> Result<(
    Vec<(NodeKind, String, String)>,
    Vec<(RelationKind, String, String)>,
)> {
    let graph = memory::MemoryGraph::default();
    for (node_id, capture) in id_captures {
        project_capture(&graph, capture, node_id, &GraphProperties::default())?;
    }
    let (nodes_map, edges) = graph.nodes_and_edges()?;
    let nodes = nodes_map
        .into_iter()
        .filter_map(|((kind, id), props)| {
            if kind == NodeKind::Notification {
                return None;
            }
            let text = capture_node_text(kind, &props);
            Some((kind, id, text))
        })
        .collect();
    Ok((nodes, edges))
}

fn capture_node_text(kind: NodeKind, props: &GraphProperties) -> String {
    let key = match kind {
        NodeKind::Decision => "title",
        NodeKind::Evidence => "content",
        NodeKind::Hypothesis => "statement",
        NodeKind::Blocker | NodeKind::DecisionRequest => "reason",
        NodeKind::Option => "label",
        NodeKind::Actor | NodeKind::Notification => return String::new(),
    };
    match props.get(key) {
        Some(GraphValue::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn option_node_id(decision_id: &str, label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{decision_id}:opt:{slug}")
}

fn upsert_actor(
    graph: &impl GraphView,
    actor_id: &str,
    properties: &GraphProperties,
) -> Result<()> {
    let mut props = properties.clone();
    props.insert(
        "kind".to_owned(),
        GraphValue::String(actor_kind(actor_id).to_owned()),
    );
    graph.upsert_node(NodeKind::Actor, actor_id, &props)
}

fn actor_kind(actor_id: &str) -> &'static str {
    if actor_id.starts_with("human:") {
        "human"
    } else if actor_id.starts_with("agent:") {
        "agent"
    } else {
        "unknown"
    }
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

fn project_capture(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    match capture.kind.as_str() {
        "decision" => {
            let mut props = origin_properties.clone();
            props.insert(
                "title".to_owned(),
                GraphValue::String(capture.title.clone()),
            );
            props.insert(
                "rationale".to_owned(),
                GraphValue::String(capture.rationale.clone()),
            );
            props.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(capture.topic_keys.clone()),
            );
            props.insert(
                "expressed_confidence".to_owned(),
                capture
                    .expressed_confidence
                    .as_deref()
                    .map_or(GraphValue::Null, |c| GraphValue::String(c.to_owned())),
            );
            graph.upsert_node(NodeKind::Decision, node_id, &props)?;

            if let Some(actor_id) = &capture.actor_id {
                upsert_actor(graph, actor_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::ProposedBy,
                    node_id,
                    actor_id,
                    origin_properties,
                )?;
            }
            if let Some(accepted_by) = &capture.accepted_by {
                upsert_actor(graph, accepted_by, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::AcceptedBy,
                    node_id,
                    accepted_by,
                    origin_properties,
                )?;
            }
            if let Some(rejected_by) = &capture.rejected_by {
                upsert_actor(graph, rejected_by, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::RejectedBy,
                    node_id,
                    rejected_by,
                    origin_properties,
                )?;
            }
            if let Some(supersedes_id) = &capture.supersedes_id {
                ensure_node_reference(graph, NodeKind::Decision, supersedes_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::Supersedes,
                    node_id,
                    supersedes_id,
                    origin_properties,
                )?;
            }
            for hypothesis_id in &capture.assumes_ids {
                ensure_node_reference(
                    graph,
                    NodeKind::Hypothesis,
                    hypothesis_id,
                    origin_properties,
                )?;
                graph.upsert_edge(
                    RelationKind::Assumes,
                    node_id,
                    hypothesis_id,
                    origin_properties,
                )?;
            }
            for evidence_id in &capture.evidence_ids {
                ensure_node_reference(graph, NodeKind::Evidence, evidence_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::BasedOn,
                    node_id,
                    evidence_id,
                    origin_properties,
                )?;
            }
            for participant_id in &capture.participants {
                upsert_actor(graph, participant_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::ParticipatedBy,
                    node_id,
                    participant_id,
                    origin_properties,
                )?;
            }
            if let Some(initiator_id) = &capture.session_initiator {
                upsert_actor(graph, initiator_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::InitiatedBy,
                    node_id,
                    initiator_id,
                    origin_properties,
                )?;
            }
            // Options: project Option nodes + HAS_OPTION/CHOSE edges.
            if let Some(options) = &capture.options {
                for option_label in options {
                    let opt_id = option_node_id(node_id, option_label);
                    let mut opt_props = origin_properties.clone();
                    opt_props.insert("label".to_owned(), GraphValue::String(option_label.clone()));
                    graph.upsert_node(NodeKind::Option, &opt_id, &opt_props)?;
                    graph.upsert_edge(
                        RelationKind::HasOption,
                        node_id,
                        &opt_id,
                        origin_properties,
                    )?;
                }
            }
            if let Some(chosen) = &capture.chosen_option {
                let opt_id = option_node_id(node_id, chosen);
                graph.upsert_edge(RelationKind::Chose, node_id, &opt_id, origin_properties)?;
            }
        }
        "evidence" => {
            let mut props = origin_properties.clone();
            props.insert(
                "content".to_owned(),
                GraphValue::String(capture.title.clone()),
            );
            props.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(capture.topic_keys.clone()),
            );
            graph.upsert_node(NodeKind::Evidence, node_id, &props)?;

            for hypothesis_id in &capture.supports_ids {
                ensure_node_reference(
                    graph,
                    NodeKind::Hypothesis,
                    hypothesis_id,
                    origin_properties,
                )?;
                graph.upsert_edge(
                    RelationKind::Supports,
                    node_id,
                    hypothesis_id,
                    origin_properties,
                )?;
            }
            for hypothesis_id in &capture.refutes_ids {
                ensure_node_reference(
                    graph,
                    NodeKind::Hypothesis,
                    hypothesis_id,
                    origin_properties,
                )?;
                graph.upsert_edge(
                    RelationKind::Refutes,
                    node_id,
                    hypothesis_id,
                    origin_properties,
                )?;
            }
        }
        "hypothesis" => {
            let mut props = origin_properties.clone();
            props.insert(
                "statement".to_owned(),
                GraphValue::String(capture.title.clone()),
            );
            graph.upsert_node(NodeKind::Hypothesis, node_id, &props)?;
        }
        "blocker" => {
            let mut props = origin_properties.clone();
            props.insert(
                "reason".to_owned(),
                GraphValue::String(capture.title.clone()),
            );
            props.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(capture.topic_keys.clone()),
            );
            graph.upsert_node(NodeKind::Blocker, node_id, &props)?;

            if let Some(blocked_actor_id) = &capture.blocked_actor_id {
                upsert_actor(graph, blocked_actor_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::BlockedActor,
                    node_id,
                    blocked_actor_id,
                    origin_properties,
                )?;
            }
            if let Some(decision_id) = &capture.decision_id {
                ensure_node_reference(graph, NodeKind::Decision, decision_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::BlockerForDecision,
                    node_id,
                    decision_id,
                    origin_properties,
                )?;
            }
        }
        "decision-request" => {
            let mut props = origin_properties.clone();
            props.insert(
                "reason".to_owned(),
                GraphValue::String(capture.title.clone()),
            );
            props.insert(
                "topic_keys".to_owned(),
                GraphValue::StringList(capture.topic_keys.clone()),
            );
            graph.upsert_node(NodeKind::DecisionRequest, node_id, &props)?;

            if let Some(actor_id) = &capture.actor_id {
                upsert_actor(graph, actor_id, origin_properties)?;
                graph.upsert_edge(
                    RelationKind::DecisionRequestedBy,
                    node_id,
                    actor_id,
                    origin_properties,
                )?;
            }
        }
        "notification" => {
            let mut props = origin_properties.clone();
            props.insert(
                "channel".to_owned(),
                GraphValue::String(capture.title.clone()),
            );
            graph.upsert_node(NodeKind::Notification, node_id, &props)?;
        }
        _ => {
            // Unknown kind — silently skip; classifier may produce unknown kinds in future schemas.
        }
    }
    Ok(())
}

fn projector_error(error: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(error.to_string())
}

#[cfg(test)]
mod tests;
