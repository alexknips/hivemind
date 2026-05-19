use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::ProjectorError;
use crate::events::{self, Event, EventId, EventPayload, RelationKind as EventRelationKind};
use crate::ledger::EventLedger;
use crate::Result;

#[cfg(feature = "graph-kuzu")]
pub mod kuzu;
pub mod memory;

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

pub fn rebuild_graph(ledger: &impl EventLedger, graph: &impl GraphView) -> Result<()> {
    graph.wipe()?;
    project_from_ledger(ledger, graph, 0)
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
mod tests {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Instant;

    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use crate::events::{Event, EventSource, EventType};
    use crate::ledger::InMemoryEventLedger;

    use super::*;

    type NodeKey = (NodeKind, String);
    type EdgeKey = (RelationKind, String, String);

    #[derive(Debug, Default)]
    struct RecordingGraph {
        nodes: Mutex<BTreeMap<NodeKey, GraphProperties>>,
        edges: Mutex<BTreeMap<EdgeKey, GraphProperties>>,
        wipes: Mutex<usize>,
    }

    impl RecordingGraph {
        fn nodes(&self) -> MutexGuard<'_, BTreeMap<NodeKey, GraphProperties>> {
            self.nodes.lock().expect("nodes lock poisoned")
        }

        fn edges(&self) -> MutexGuard<'_, BTreeMap<EdgeKey, GraphProperties>> {
            self.edges.lock().expect("edges lock poisoned")
        }

        fn snapshot(&self) -> GraphSnapshot {
            GraphSnapshot {
                nodes: self.nodes().clone(),
                edges: self.edges().clone(),
            }
        }

        fn wipe_count(&self) -> usize {
            *self.wipes.lock().expect("wipes lock poisoned")
        }
    }

    impl GraphView for RecordingGraph {
        fn upsert_node(
            &self,
            kind: NodeKind,
            id: &str,
            properties: &GraphProperties,
        ) -> Result<()> {
            self.nodes()
                .insert((kind, id.to_owned()), properties.clone());
            Ok(())
        }

        fn upsert_edge(
            &self,
            kind: RelationKind,
            from_id: &str,
            to_id: &str,
            properties: &GraphProperties,
        ) -> Result<()> {
            self.edges().insert(
                (kind, from_id.to_owned(), to_id.to_owned()),
                properties.clone(),
            );
            Ok(())
        }

        fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
            if cypher.contains("RETURN node.id AS id LIMIT 1;") {
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Ok(Vec::new()),
                };
                for kind in NodeKind::ALL {
                    if cypher.contains(&format!("`{}`", kind.table_name()))
                        && self.nodes().contains_key(&(kind, id.clone()))
                    {
                        return Ok(vec![GraphRow::from([(
                            "id".to_owned(),
                            GraphValue::String(id.clone()),
                        )])]);
                    }
                }
            }

            Ok(Vec::new())
        }

        fn wipe(&self) -> Result<()> {
            self.nodes().clear();
            self.edges().clear();
            *self.wipes.lock().expect("wipes lock poisoned") += 1;
            Ok(())
        }
    }

    #[derive(Debug, PartialEq)]
    struct GraphSnapshot {
        nodes: BTreeMap<NodeKey, GraphProperties>,
        edges: BTreeMap<EdgeKey, GraphProperties>,
    }

    #[test]
    fn projects_all_slice_one_events_to_graph_mutations() -> Result<()> {
        let ledger = fixture_ledger()?;
        let graph = RecordingGraph::default();

        project_from_ledger(&ledger, &graph, 0)?;

        let nodes = graph.nodes();
        assert_eq!(
            nodes
                .get(&(NodeKind::Evidence, "evidence:1".to_owned()))
                .and_then(|properties| properties.get("content")),
            Some(&GraphValue::String(
                "Kuzu supports graph projection".to_owned()
            ))
        );
        assert_eq!(
            nodes
                .get(&(NodeKind::Hypothesis, "hypothesis:1".to_owned()))
                .and_then(|properties| properties.get("statement")),
            Some(&GraphValue::String("Graph projection is viable".to_owned()))
        );
        assert_eq!(
            nodes
                .get(&(NodeKind::Decision, "decision:1".to_owned()))
                .and_then(|properties| properties.get("topic_keys")),
            Some(&GraphValue::StringList(vec![
                "architecture".to_owned(),
                "memory".to_owned()
            ]))
        );
        assert!(nodes.contains_key(&(NodeKind::Option, "option:1".to_owned())));
        assert_eq!(
            nodes
                .get(&(NodeKind::Actor, "actor:alice".to_owned()))
                .and_then(|properties| properties.get("source")),
            Some(&GraphValue::String("agent".to_owned()))
        );
        let request_id = nodes
            .iter()
            .find_map(|((kind, id), properties)| {
                (*kind == NodeKind::DecisionRequest
                    && properties.get("client_request_id")
                        == Some(&GraphValue::String("client-request:release-1".to_owned())))
                .then(|| id.clone())
            })
            .expect("decision request node projected");
        assert_eq!(
            nodes
                .get(&(NodeKind::DecisionRequest, request_id.clone()))
                .and_then(|properties| properties.get("priority")),
            Some(&GraphValue::String("P1".to_owned()))
        );
        assert_eq!(
            nodes
                .get(&(NodeKind::Blocker, "blocker:release-owner".to_owned()))
                .and_then(|properties| properties.get("reason")),
            Some(&GraphValue::String(
                "Release migration cannot continue without owner approval".to_owned()
            ))
        );
        let notification_id = nodes
            .iter()
            .find_map(|((kind, id), properties)| {
                (*kind == NodeKind::Notification
                    && properties.get("dedupe_key")
                        == Some(&GraphValue::String(
                            "tenant:release:blocker:release-owner:P1".to_owned(),
                        )))
                .then(|| id.clone())
            })
            .expect("notification node projected");
        assert_eq!(
            nodes
                .get(&(NodeKind::Notification, notification_id.clone()))
                .and_then(|properties| properties.get("source_event_ids")),
            Some(&GraphValue::StringList(vec!["10".to_owned()]))
        );
        drop(nodes);

        let edges = graph.edges();
        assert_eq!(
            edges
                .get(&(
                    RelationKind::ProposedBy,
                    "decision:1".to_owned(),
                    "actor:alice".to_owned()
                ))
                .and_then(|properties| properties.get("source_ref")),
            Some(&GraphValue::String("projection-test".to_owned()))
        );
        for expected in [
            (RelationKind::ProposedBy, "decision:1", "actor:alice"),
            (RelationKind::HasOption, "decision:1", "option:1"),
            (RelationKind::Chose, "decision:1", "option:2"),
            (RelationKind::Assumes, "decision:1", "hypothesis:1"),
            (RelationKind::BasedOn, "decision:1", "evidence:1"),
            (RelationKind::AcceptedBy, "decision:1", "actor:bob"),
            (RelationKind::RejectedBy, "decision:1", "actor:carol"),
            (RelationKind::Supersedes, "decision:2", "decision:1"),
            (RelationKind::Supports, "evidence:1", "hypothesis:1"),
            (
                RelationKind::DecisionRequestedBy,
                request_id.as_str(),
                "agent:release-bot",
            ),
            (
                RelationKind::DecisionRequestForDecision,
                request_id.as_str(),
                "decision:1",
            ),
            (
                RelationKind::DecisionRequestRequiredOwner,
                request_id.as_str(),
                "human:release-owner",
            ),
            (
                RelationKind::BlockedActor,
                "blocker:release-owner",
                "agent:release-bot",
            ),
            (
                RelationKind::BlockerForDecision,
                "blocker:release-owner",
                "decision:1",
            ),
            (
                RelationKind::BlockerRequiredOwner,
                "blocker:release-owner",
                "human:release-owner",
            ),
            (
                RelationKind::NotificationForBlocker,
                notification_id.as_str(),
                "blocker:release-owner",
            ),
            (
                RelationKind::NotificationRecipient,
                notification_id.as_str(),
                "human:release-owner",
            ),
        ] {
            assert!(
                edges.contains_key(&(expected.0, expected.1.to_owned(), expected.2.to_owned())),
                "missing edge {expected:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn rebuild_wipes_and_replays_deterministically() -> Result<()> {
        let ledger = fixture_ledger()?;
        let first_graph = RecordingGraph::default();
        let second_graph = RecordingGraph::default();

        rebuild_graph(&ledger, &first_graph)?;
        rebuild_graph(&ledger, &second_graph)?;

        assert_eq!(first_graph.wipe_count(), 1);
        assert_eq!(second_graph.wipe_count(), 1);
        assert_eq!(first_graph.snapshot(), second_graph.snapshot());

        Ok(())
    }

    #[test]
    fn refuses_to_project_events_without_ledger_origin() {
        let graph = RecordingGraph::default();
        let event = event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({
                "evidence_id": "evidence:missing-origin",
                "content": "not appended"
            }),
        );

        assert!(project_event(&graph, &event).is_err());
    }

    #[test]
    #[ignore = "performance benchmark; run in isolated environment"]
    fn recording_graph_rebuild_of_10k_events_stays_fast() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        for index in 0..10_000 {
            ledger.append(event(
                EventType::EvidenceRecorded,
                "actor:bench",
                json!({
                    "evidence_id": format!("evidence:{index}"),
                    "content": format!("content {index}")
                }),
            ))?;
        }

        let graph = RecordingGraph::default();
        let start = Instant::now();
        rebuild_graph(&ledger, &graph)?;

        assert_eq!(graph.nodes().len(), 10_001);
        assert!(start.elapsed().as_secs_f64() < 1.0);

        Ok(())
    }

    fn fixture_ledger() -> Result<InMemoryEventLedger> {
        let ledger = InMemoryEventLedger::new();
        for event in [
            event(
                EventType::EvidenceRecorded,
                "actor:alice",
                json!({
                    "evidence_id": "evidence:1",
                    "content": "Kuzu supports graph projection",
                    "source": "unit-test"
                }),
            ),
            event(
                EventType::HypothesisRecorded,
                "actor:alice",
                json!({
                    "hypothesis_id": "hypothesis:1",
                    "statement": "Graph projection is viable"
                }),
            ),
            event(
                EventType::DecisionProposed,
                "actor:alice",
                json!({
                    "decision_id": "decision:1",
                    "title": "Use Kuzu for slice 1",
                    "rationale": "It gives us graph queries without extra services",
                    "topic_keys": ["architecture", "memory"],
                    "option_ids": ["option:1", "option:2"],
                    "chosen_option_id": "option:2",
                    "hypothesis_ids": ["hypothesis:1"],
                    "evidence_ids": ["evidence:1"]
                }),
            ),
            event(
                EventType::DecisionAccepted,
                "actor:bob",
                json!({
                    "decision_id": "decision:1"
                }),
            ),
            event(
                EventType::DecisionRejected,
                "actor:carol",
                json!({
                    "decision_id": "decision:1"
                }),
            ),
            event(
                EventType::DecisionProposed,
                "actor:alice",
                json!({
                    "decision_id": "decision:2",
                    "title": "Use Kuzu with conservative Cypher",
                    "rationale": "Keep future backend swap cheap",
                    "topic_keys": ["architecture"],
                    "option_ids": [],
                    "chosen_option_id": null,
                    "hypothesis_ids": [],
                    "evidence_ids": []
                }),
            ),
            event(
                EventType::DecisionSuperseded,
                "actor:alice",
                json!({
                    "old_decision_id": "decision:1",
                    "new_decision_id": "decision:2"
                }),
            ),
            event(
                EventType::RelationAdded,
                "actor:alice",
                json!({
                    "relation": "SUPPORTS",
                    "from_id": "evidence:1",
                    "to_id": "hypothesis:1"
                }),
            ),
            event(
                EventType::DecisionRequested,
                "agent:release-bot",
                json!({
                    "topic_keys": ["release"],
                    "decision_id": "decision:1",
                    "reason": "Release migration needs an owner decision",
                    "priority": "P1",
                    "required_owner_id": "human:release-owner",
                    "authority_class": "human_required",
                    "requested_by": "agent:release-bot",
                    "client_request_id": "client-request:release-1"
                }),
            ),
            event(
                EventType::BlockerReported,
                "agent:release-bot",
                json!({
                    "blocker_id": "blocker:release-owner",
                    "blocked_actor_id": "agent:release-bot",
                    "decision_id": "decision:1",
                    "topic_keys": ["release"],
                    "blocked_ref": "run:release-migration",
                    "blocked_ref_type": "agent_run",
                    "reason": "Release migration cannot continue without owner approval",
                    "priority": "P1",
                    "last_progress_at": "2026-05-19T10:30:00Z",
                    "required_owner_id": "human:release-owner"
                }),
            ),
            event(
                EventType::NotificationSent,
                "agent:notifier",
                json!({
                    "blocker_id": "blocker:release-owner",
                    "recipient_actor_id": "human:release-owner",
                    "channel": "slack",
                    "threshold_rule": "p1_human_required_direct_15m",
                    "source_event_ids": [10],
                    "dedupe_key": "tenant:release:blocker:release-owner:P1",
                    "sent_at": "2026-05-19T10:45:00Z"
                }),
            ),
        ] {
            ledger.append(event)?;
        }
        Ok(ledger)
    }

    fn event(event_type: EventType, actor_id: &str, payload: serde_json::Value) -> Event {
        Event {
            event_id: None,
            event_uuid: Uuid::new_v4(),
            correlation_id: Some("projection-test".to_owned()),
            causation_event_id: None,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Agent,
            source_ref: Some("projection-test".to_owned()),
            payload,
            ts: Some(Utc::now()),
        }
    }
}
