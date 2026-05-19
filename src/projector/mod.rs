use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::ProjectorError;
use crate::events::{self, Event, EventId, EventPayload, RelationKind as EventRelationKind};
use crate::ledger::EventLedger;
use crate::Result;

#[cfg(feature = "graph-kuzu")]
pub mod kuzu;

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
    Actor,
    Evidence,
    Option,
    Hypothesis,
}

impl NodeKind {
    pub const ALL: [Self; 5] = [
        Self::Decision,
        Self::Actor,
        Self::Evidence,
        Self::Option,
        Self::Hypothesis,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::Actor => "Actor",
            Self::Evidence => "Evidence",
            Self::Option => "Option",
            Self::Hypothesis => "Hypothesis",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    ProposedBy,
    AcceptedBy,
    RejectedBy,
    Supersedes,
    BasedOn,
    HasOption,
    Chose,
    Assumes,
    Supports,
    Refutes,
}

impl RelationKind {
    pub const ALL: [Self; 10] = [
        Self::ProposedBy,
        Self::AcceptedBy,
        Self::RejectedBy,
        Self::Supersedes,
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
            Self::AcceptedBy => "ACCEPTED_BY",
            Self::RejectedBy => "REJECTED_BY",
            Self::Supersedes => "SUPERSEDES",
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
            Self::Supersedes => (NodeKind::Decision, NodeKind::Decision),
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

    upsert_actor(graph, &event.actor_id, event_origin)?;

    match payload {
        EventPayload::DecisionProposed(payload) => {
            let decision_properties = GraphProperties::from([
                (
                    "title".to_owned(),
                    GraphValue::String(payload.title.clone()),
                ),
                (
                    "rationale".to_owned(),
                    GraphValue::String(payload.rationale.clone()),
                ),
                (
                    "topic_keys".to_owned(),
                    GraphValue::StringList(payload.topic_keys.clone()),
                ),
                ("event_origin".to_owned(), GraphValue::Int(event_origin)),
            ]);
            graph.upsert_node(
                NodeKind::Decision,
                &payload.decision_id,
                &decision_properties,
            )?;
            graph.upsert_edge(
                RelationKind::ProposedBy,
                &payload.decision_id,
                &event.actor_id,
                &origin_properties(event_origin),
            )?;

            for option_id in &payload.option_ids {
                graph.upsert_node(
                    NodeKind::Option,
                    option_id,
                    &origin_properties(event_origin),
                )?;
                graph.upsert_edge(
                    RelationKind::HasOption,
                    &payload.decision_id,
                    option_id,
                    &origin_properties(event_origin),
                )?;
            }

            if let Some(chosen_option_id) = &payload.chosen_option_id {
                graph.upsert_node(
                    NodeKind::Option,
                    chosen_option_id,
                    &origin_properties(event_origin),
                )?;
                graph.upsert_edge(
                    RelationKind::Chose,
                    &payload.decision_id,
                    chosen_option_id,
                    &origin_properties(event_origin),
                )?;
            }

            for hypothesis_id in &payload.hypothesis_ids {
                graph.upsert_edge(
                    RelationKind::Assumes,
                    &payload.decision_id,
                    hypothesis_id,
                    &origin_properties(event_origin),
                )?;
            }

            for evidence_id in &payload.evidence_ids {
                graph.upsert_edge(
                    RelationKind::BasedOn,
                    &payload.decision_id,
                    evidence_id,
                    &origin_properties(event_origin),
                )?;
            }
        }
        EventPayload::DecisionAccepted(payload) => graph.upsert_edge(
            RelationKind::AcceptedBy,
            &payload.decision_id,
            &event.actor_id,
            &origin_properties(event_origin),
        )?,
        EventPayload::DecisionRejected(payload) => graph.upsert_edge(
            RelationKind::RejectedBy,
            &payload.decision_id,
            &event.actor_id,
            &origin_properties(event_origin),
        )?,
        EventPayload::DecisionSuperseded(payload) => graph.upsert_edge(
            RelationKind::Supersedes,
            &payload.new_decision_id,
            &payload.old_decision_id,
            &origin_properties(event_origin),
        )?,
        EventPayload::EvidenceRecorded(payload) => {
            let evidence_properties = GraphProperties::from([
                (
                    "content".to_owned(),
                    GraphValue::String(payload.content.clone()),
                ),
                ("event_origin".to_owned(), GraphValue::Int(event_origin)),
            ]);
            graph.upsert_node(
                NodeKind::Evidence,
                &payload.evidence_id,
                &evidence_properties,
            )?;
        }
        EventPayload::HypothesisRecorded(payload) => {
            let hypothesis_properties = GraphProperties::from([
                (
                    "statement".to_owned(),
                    GraphValue::String(payload.statement.clone()),
                ),
                ("event_origin".to_owned(), GraphValue::Int(event_origin)),
            ]);
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
            &origin_properties(event_origin),
        )?,
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

fn upsert_actor(graph: &impl GraphView, actor_id: &str, event_origin: i64) -> Result<()> {
    graph.upsert_node(NodeKind::Actor, actor_id, &origin_properties(event_origin))
}

fn origin_properties(event_origin: i64) -> GraphProperties {
    GraphProperties::from([("event_origin".to_owned(), GraphValue::Int(event_origin))])
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

    use crate::events::{Event, EventType};
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

        fn query(&self, _cypher: &str, _params: &GraphParams) -> Result<Vec<GraphRow>> {
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
        assert!(nodes.contains_key(&(NodeKind::Actor, "actor:alice".to_owned())));
        drop(nodes);

        let edges = graph.edges();
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
        ] {
            ledger.append(event)?;
        }
        Ok(ledger)
    }

    fn event(event_type: EventType, actor_id: &str, payload: serde_json::Value) -> Event {
        Event {
            event_id: None,
            event_uuid: Uuid::new_v4(),
            correlation_id: None,
            causation_event_id: None,
            event_type,
            actor_id: actor_id.to_owned(),
            payload,
            ts: Some(Utc::now()),
        }
    }
}
