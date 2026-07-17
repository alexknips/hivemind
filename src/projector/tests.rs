// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
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
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
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
        (RelationKind::PremisedOn, "decision:1", "hypothesis:1"),
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
        tenant_id: Default::default(),
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

#[test]
fn decision_proposed_with_expressed_confidence_stores_it_on_node() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:conf",
            "title": "Use gRPC for internal API",
            "rationale": "Lower overhead, but we're not sure yet",
            "topic_keys": ["api"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": [],
            "expressed_confidence": "low"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        // ubs:ignore
        graph
            .nodes()
            .get(&(NodeKind::Decision, "decision:conf".to_owned()))
            .and_then(|p| p.get("expressed_confidence")),
        Some(&GraphValue::String("low".to_owned())),
        "expressed_confidence must be stored on the Decision node"
    );
    Ok(())
}

#[test]
fn decision_proposed_without_expressed_confidence_stores_null() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:noconf",
            "title": "Use REST",
            "rationale": "Standard practice",
            "topic_keys": ["api"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        // ubs:ignore
        graph
            .nodes()
            .get(&(NodeKind::Decision, "decision:noconf".to_owned()))
            .and_then(|p| p.get("expressed_confidence")),
        Some(&GraphValue::Null),
    );
    Ok(())
}

#[test]
fn classified_batch_decision_projects_node_and_actor_edges() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:1",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "Use Postgres for storage",
                "rationale": "Scales well and team knows it",
                "topic_keys": ["storage", "database"],
                "evidence_ids": [],
                "options": ["postgres", "mysql"],
                "chosen_option": "postgres",
                "extraction_confidence": 0.92,
                "expressed_confidence": "high",
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": "human:alice",
                "accepted_by": "human:bob",
                "rejected_by": null,
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let decision_node = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::Decision)
        .map(|((_, id), props)| (id.clone(), props.clone()))
        .expect("decision node from capture"); // ubs:ignore

    assert_eq!(
        // ubs:ignore
        decision_node.1.get("title"),
        Some(&GraphValue::String("Use Postgres for storage".to_owned()))
    );
    assert_eq!(
        // ubs:ignore
        decision_node.1.get("expressed_confidence"),
        Some(&GraphValue::String("high".to_owned()))
    );
    assert!(
        // ubs:ignore
        nodes.contains_key(&(NodeKind::Actor, "human:alice".to_owned())),
        "proposer actor must be upserted"
    );
    assert!(
        // ubs:ignore
        nodes.contains_key(&(NodeKind::Actor, "human:bob".to_owned())),
        "acceptor actor must be upserted"
    );
    drop(nodes);

    let edges = graph.edges();
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::ProposedBy,
            decision_node.0.clone(),
            "human:alice".to_owned()
        )),
        "ProposedBy edge required"
    );
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::AcceptedBy,
            decision_node.0.clone(),
            "human:bob".to_owned()
        )),
        "AcceptedBy edge required"
    );
    Ok(())
}

#[test]
fn classified_batch_decision_supersedes_edge() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    // Seed the old decision so ensure_node_reference finds it.
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:old",
            "title": "Old approach",
            "rationale": "Original choice",
            "topic_keys": ["arch"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:2",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "New approach supersedes old",
                "rationale": "Better fit",
                "topic_keys": ["arch"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.85,
                "expressed_confidence": null,
                "supersedes_id": "decision:old",
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": null,
                "accepted_by": null,
                "rejected_by": null,
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let edges = graph.edges();
    let supersedes = edges
        .keys()
        .find(|(kind, _, to)| *kind == RelationKind::Supersedes && to == "decision:old");
    assert!(
        // ubs:ignore
        supersedes.is_some(),
        "Supersedes edge to decision:old required"
    );
    Ok(())
}

#[test]
fn classified_batch_evidence_supports_and_refutes() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    // Seed hypothesis nodes.
    ledger.append(event(
        EventType::HypothesisRecorded,
        "actor:alice",
        json!({ "hypothesis_id": "hyp:1", "statement": "Caching helps latency" }),
    ))?;
    ledger.append(event(
        EventType::HypothesisRecorded,
        "actor:alice",
        json!({ "hypothesis_id": "hyp:2", "statement": "Caching hurts write throughput" }),
    ))?;
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:3",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "evidence",
                "title": "Cache hit rate 95% in load test",
                "rationale": "Load test result",
                "topic_keys": ["cache", "latency"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": ["hyp:1"],
                "refutes_ids": ["hyp:2"],
                "actor_id": null,
                "accepted_by": null,
                "rejected_by": null,
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let edges = graph.edges();
    let supports = edges
        .keys()
        .find(|(kind, _, to)| *kind == RelationKind::Supports && to == "hyp:1");
    let refutes = edges
        .keys()
        .find(|(kind, _, to)| *kind == RelationKind::Refutes && to == "hyp:2");
    assert!(supports.is_some(), "Supports edge to hyp:1 required"); // ubs:ignore
    assert!(refutes.is_some(), "Refutes edge to hyp:2 required"); // ubs:ignore
    Ok(())
}

#[test]
fn classified_batch_blocker_actor_and_decision_edges() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:deploy",
            "title": "Deploy now",
            "rationale": "Tests passed",
            "topic_keys": ["deploy"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:4",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "blocker",
                "title": "Waiting for security sign-off",
                "rationale": "Cannot deploy without security approval",
                "topic_keys": ["security", "deploy"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.88,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": null,
                "accepted_by": null,
                "rejected_by": null,
                "blocked_actor_id": "human:priya",
                "decision_id": "decision:deploy"
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let blocker_node = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::Blocker)
        .map(|((_, id), _)| id.clone())
        .expect("blocker node required"); // ubs:ignore
    drop(nodes);

    let edges = graph.edges();
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::BlockedActor,
            blocker_node.clone(),
            "human:priya".to_owned()
        )),
        "BlockedActor edge required"
    );
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::BlockerForDecision,
            blocker_node,
            "decision:deploy".to_owned()
        )),
        "BlockerForDecision edge required"
    );
    Ok(())
}

#[test]
fn classified_batch_hypothesis_projects_node() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:5",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "hypothesis",
                "title": "Batching reduces API cost by 40%",
                "rationale": "Unverified claim from team",
                "topic_keys": ["cost", "batching"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.75,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": null,
                "accepted_by": null,
                "rejected_by": null,
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let hyp = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::Hypothesis)
        .map(|((_, _), props)| props.clone())
        .expect("hypothesis node required"); // ubs:ignore
    assert_eq!(
        // ubs:ignore
        hyp.get("statement"),
        Some(&GraphValue::String(
            "Batching reduces API cost by 40%".to_owned()
        ))
    );
    Ok(())
}

#[test]
fn classified_batch_empty_captures_is_no_op() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:empty",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    // Only the classifier actor node; no decision/evidence/hypothesis nodes.
    let nodes = graph.nodes();
    assert_eq!(
        // ubs:ignore
        nodes.len(),
        1,
        "only the actor node from the event itself; no capture nodes"
    );
    Ok(())
}
