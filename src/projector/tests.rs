// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::sync::{Arc, Mutex, MutexGuard};
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
        (RelationKind::PremisedOn, "option:2", "hypothesis:1"),
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
fn projects_registered_links_and_anchors_projects() -> Result<()> {
    use super::memory::MemoryGraph;

    let ledger = InMemoryEventLedger::new();
    for event in [
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "platform", "display_name": "Platform"}),
        ),
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "billing", "display_name": "Billing"}),
        ),
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "auth", "display_name": "Auth"}),
        ),
        event(
            EventType::ProjectLinked,
            "actor:alice",
            json!({"from": "billing", "to": "platform", "kind": "part_of"}),
        ),
        event(
            EventType::ProjectLinked,
            "actor:alice",
            json!({"from": "billing", "to": "auth", "kind": "depends_on"}),
        ),
        event(
            EventType::ProjectAnchored,
            "actor:alice",
            json!({"handle": "billing", "anchor_kind": "folder", "value": "services/billing"}),
        ),
        event(
            EventType::ProjectAnchored,
            "actor:alice",
            json!({"handle": "billing", "anchor_kind": "rig", "value": "hivemind"}),
        ),
    ] {
        ledger.append(event)?;
    }

    let graph = MemoryGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let project_rows = graph.query(
        "MATCH (node:`Project`) RETURN node.id AS id, node.handle AS handle, node.display_name AS display_name, node.anchors AS anchors ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    assert_eq!(project_rows.len(), 3);

    let billing = project_rows
        .iter()
        .find(|row| row.get("id") == Some(&GraphValue::String("billing".to_owned())))
        .expect("billing project node exists");
    assert_eq!(
        billing.get("display_name"),
        Some(&GraphValue::String("Billing".to_owned()))
    );
    match billing.get("anchors") {
        Some(GraphValue::StringList(values)) => {
            let mut sorted = values.clone();
            sorted.sort();
            assert_eq!(
                sorted,
                vec![
                    "folder:services/billing".to_owned(),
                    "rig:hivemind".to_owned(),
                ]
            );
        }
        other => panic!("expected anchors StringList, got {other:?}"),
    }

    let part_of_rows = graph.query(
        "MATCH (from:`Project`)-[:`PART_OF`]->(to:`Project`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        &GraphParams::new(),
    )?;
    assert_eq!(
        part_of_rows,
        vec![GraphRow::from([
            (
                "from_id".to_owned(),
                GraphValue::String("billing".to_owned())
            ),
            (
                "to_id".to_owned(),
                GraphValue::String("platform".to_owned())
            ),
        ])]
    );

    let depends_on_rows = graph.query(
        "MATCH (from:`Project`)-[:`DEPENDS_ON`]->(to:`Project`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        &GraphParams::new(),
    )?;
    assert_eq!(
        depends_on_rows,
        vec![GraphRow::from([
            (
                "from_id".to_owned(),
                GraphValue::String("billing".to_owned())
            ),
            ("to_id".to_owned(), GraphValue::String("auth".to_owned())),
        ])]
    );

    // Unanchoring the folder marker removes only that anchor from the list.
    ledger.append(event(
        EventType::ProjectUnanchored,
        "actor:alice",
        json!({"handle": "billing", "anchor_kind": "folder", "value": "services/billing"}),
    ))?;
    project_from_ledger(&ledger, &graph, 7)?;

    let project_rows = graph.query(
        "MATCH (node:`Project`) RETURN node.id AS id, node.anchors AS anchors ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    let billing = project_rows
        .iter()
        .find(|row| row.get("id") == Some(&GraphValue::String("billing".to_owned())))
        .expect("billing project node exists");
    assert_eq!(
        billing.get("anchors"),
        Some(&GraphValue::StringList(vec!["rig:hivemind".to_owned()]))
    );

    Ok(())
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
fn decision_proposed_with_quote_and_question_stores_both_on_node() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:quoted",
            "title": "Personal projects are visible tenant-wide",
            "rationale": "Spelled out: a personal project is visible to the whole tenant.",
            "topic_keys": ["projects"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": [],
            "quote": "1a",
            "question": "Should a personal project be visible to the whole tenant?"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let properties = graph
        .nodes()
        .get(&(NodeKind::Decision, "decision:quoted".to_owned()))
        .cloned()
        .expect("decision node projected");
    assert_eq!(
        properties.get("quote"),
        Some(&GraphValue::String("1a".to_owned())),
        "quote must be stored on the Decision node"
    );
    assert_eq!(
        properties.get("question"),
        Some(&GraphValue::String(
            "Should a personal project be visible to the whole tenant?".to_owned()
        )),
        "question must be stored on the Decision node"
    );
    Ok(())
}

#[test]
fn decision_proposed_without_quote_stores_null() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:unquoted",
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

    let properties = graph
        .nodes()
        .get(&(NodeKind::Decision, "decision:unquoted".to_owned()))
        .cloned()
        .expect("decision node projected");
    assert_eq!(properties.get("quote"), Some(&GraphValue::Null));
    assert_eq!(properties.get("question"), Some(&GraphValue::Null));
    Ok(())
}

#[test]
fn decision_proposed_with_stated_project_stores_handle_and_source() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:billing-1",
            "title": "Use per-seat pricing",
            "rationale": "Simpler to reason about at our scale",
            "topic_keys": ["pricing"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": [],
            "project": "billing",
            "project_source": "stated"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let properties = graph
        .nodes()
        .get(&(NodeKind::Decision, "decision:billing-1".to_owned()))
        .cloned()
        .expect("decision node projected");
    assert_eq!(
        properties.get("project"),
        Some(&GraphValue::String("billing".to_owned())),
        "a stated project handle must be stored verbatim"
    );
    assert_eq!(
        properties.get("project_source"),
        Some(&GraphValue::String("stated".to_owned()))
    );
    Ok(())
}

#[test]
fn decision_proposed_with_inherited_source_stores_it_as_recorded() -> Result<()> {
    // hivemind-9lzi: a replacement that took its project from the decision it replaces reads
    // `inherited` on the node -- not `moved`, which only a `decision.moved` stamps.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:billing-2",
            "title": "Use per-seat pricing from the first invoice",
            "rationale": "The first invoice is where customers see the price",
            "topic_keys": ["pricing"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": [],
            "project": "billing",
            "project_source": "inherited"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let properties = graph
        .nodes()
        .get(&(NodeKind::Decision, "decision:billing-2".to_owned()))
        .cloned()
        .expect("decision node projected");
    assert_eq!(
        properties.get("project"),
        Some(&GraphValue::String("billing".to_owned()))
    );
    assert_eq!(
        properties.get("project_source"),
        Some(&GraphValue::String("inherited".to_owned()))
    );
    Ok(())
}

/// A Decision node's `slug`, or `None` if it has none (not projected, or from an event
/// predating this field).
fn decision_slug(graph: &impl GraphView, decision_id: &str) -> Result<Option<GraphValue>> {
    let rows = graph.query(
        "MATCH (node:`Decision` {id: $id}) RETURN node.id AS id, node.slug AS slug ORDER BY node.id;",
        &GraphParams::from([(
            "id".to_owned(),
            GraphValue::String(decision_id.to_owned()),
        )]),
    )?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|row| row.get("slug").cloned()))
}

#[test]
fn decision_proposed_stores_a_slug_kebab_cased_from_the_title() -> Result<()> {
    use super::memory::MemoryGraph;

    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:slug-solo",
            "title": "Use per-seat pricing",
            "rationale": "Simpler to reason about at our scale",
            "topic_keys": ["pricing"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = MemoryGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        decision_slug(&graph, "decision:slug-solo")?,
        Some(GraphValue::String("use-per-seat-pricing".to_owned())),
        "the only decision with this title claims the bare slug"
    );
    Ok(())
}

#[test]
fn decision_proposed_with_a_colliding_title_gets_a_suffixed_slug() -> Result<()> {
    use super::memory::MemoryGraph;

    let ledger = InMemoryEventLedger::new();
    let propose = |decision_id: &str| {
        event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": decision_id,
                "title": "Duplicate Title",
                "rationale": "Two decisions that happen to share a title",
                "topic_keys": ["pricing"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        )
    };
    // Ledger order decides who claims the bare slug -- oldest wins.
    ledger.append(propose("decision:collision-first"))?;
    ledger.append(propose("decision:collision-second"))?;

    let graph = MemoryGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        decision_slug(&graph, "decision:collision-first")?,
        Some(GraphValue::String("duplicate-title".to_owned())),
        "the first decision with this title claims the bare slug"
    );
    assert_eq!(
        decision_slug(&graph, "decision:collision-second")?,
        // decision_id_tail("decision:collision-second") = "decisioncollisionsecond"; the
        // minimum 4-character suffix ("deci") is not itself taken, so it's used immediately.
        Some(GraphValue::String("duplicate-title-deci".to_owned())),
        "a later decision with the same title gets an id-tail suffix, not the same link"
    );
    Ok(())
}

/// The slug rule before hivemind-q9l1 (cut at the cap wherever it falls; an apostrophe is a word
/// break), kept so the tests below can show which titles the new rule leaves alone.
fn slug_before_q9l1(title: &str) -> String {
    let mut slug = normalize_topic_key(title);
    if slug.len() > MAX_DECISION_SLUG_LEN {
        slug.truncate(MAX_DECISION_SLUG_LEN);
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    if slug.is_empty() {
        slug.push_str("decision");
    }
    slug
}

/// Exactly `MAX_DECISION_SLUG_LEN` characters, all of them words and single spaces.
const SIXTY_CHARACTER_TITLE: &str = "Keep each ledger entry append only so nothing is overwritten";

#[test]
fn a_long_title_slug_ends_at_a_whole_word() {
    let title = "Keep the ledger append-only so none of it is ever overwritten by a replay";
    assert_eq!(
        slug_before_q9l1(title),
        "keep-the-ledger-append-only-so-none-of-it-is-ever-overwritte",
        "the old rule cut inside `overwritten`"
    );
    assert_eq!(
        capped_decision_slug(title),
        "keep-the-ledger-append-only-so-none-of-it-is-ever",
        "the slug stops at the last word that fits in 60 characters"
    );
}

#[test]
fn the_slug_cap_sits_below_the_topic_key_cut() {
    const { assert!(MAX_DECISION_SLUG_LEN < crate::commands::MAX_TOPIC_KEY_LEN) };
}

#[test]
fn a_title_of_exactly_the_cap_keeps_every_word() {
    assert_eq!(SIXTY_CHARACTER_TITLE.len(), MAX_DECISION_SLUG_LEN);
    assert_eq!(
        capped_decision_slug(SIXTY_CHARACTER_TITLE),
        "keep-each-ledger-entry-append-only-so-nothing-is-overwritten"
    );
}

#[test]
fn a_word_that_ends_exactly_at_the_cap_is_kept_when_more_words_follow() {
    // The slug is 66 characters, so `normalize_topic_key` itself cuts it at 64 inside `today`;
    // the cap still falls right after `overwritten`, a whole word.
    let title = format!("{SIXTY_CHARACTER_TITLE} today");
    assert_eq!(
        capped_decision_slug(&title),
        "keep-each-ledger-entry-append-only-so-nothing-is-overwritten"
    );
}

#[test]
fn a_word_that_runs_one_past_the_cap_is_left_out_whole() {
    let title = format!("{SIXTY_CHARACTER_TITLE}x");
    assert_eq!(
        capped_decision_slug(&title),
        "keep-each-ledger-entry-append-only-so-nothing-is",
        "a cut at the cap would end the slug in half of `overwrittenx`"
    );
}

#[test]
fn a_single_word_longer_than_the_cap_is_cut_at_the_cap() {
    let title = "a".repeat(MAX_DECISION_SLUG_LEN + 10);
    assert_eq!(
        capped_decision_slug(&title),
        "a".repeat(MAX_DECISION_SLUG_LEN),
        "with no whole word that fits, the slug still says something"
    );
}

#[test]
fn an_apostrophe_is_dropped_not_read_as_a_word_break() {
    for (title, slug) in [
        ("Show the reader's name", "show-the-readers-name"),
        ("Show the readers' names", "show-the-readers-names"),
        (
            "Don\u{2019}t drop the reader\u{2019}s name",
            "dont-drop-the-readers-name",
        ),
        ("It\u{2018}s \u{02BC}quoted\u{02BC}", "its-quoted"),
    ] {
        assert_eq!(capped_decision_slug(title), slug, "slug of {title:?}");
    }
    assert_eq!(
        slug_before_q9l1("Show the reader's name"),
        "show-the-reader-s-name"
    );
}

#[test]
fn a_title_of_only_apostrophes_slugs_to_decision() {
    assert_eq!(capped_decision_slug("''\u{2019}"), "decision");
}

#[test]
fn a_slug_never_exceeds_the_cap_so_a_collision_suffix_has_its_room() {
    for title in [
        SIXTY_CHARACTER_TITLE.to_owned(),
        format!("{SIXTY_CHARACTER_TITLE} and then some more words after it"),
        "a".repeat(200),
        "word ".repeat(40),
        "Keep the ledger append-only so none of it is ever overwritten by a replay".to_owned(),
    ] {
        let slug = capped_decision_slug(&title);
        assert!(
            slug.len() <= MAX_DECISION_SLUG_LEN,
            "{slug:?} is over the cap"
        );
        assert!(!slug.ends_with('-'), "{slug:?} ends in a separator");
    }
}

/// A title the two rules never disagree on slugs exactly as it did, so nothing already linked
/// to moves unless its title is cut inside a word or has an apostrophe.
#[test]
fn titles_without_either_problem_slug_as_they_did() {
    for title in [
        "Use per-seat pricing",
        "Duplicate Title",
        "Cell smoke test (HTTP API) \u{2014} hivemind-zdsh.5",
        "Na\u{ef}ve r\u{e9}sum\u{e9} parsing, v2!",
        SIXTY_CHARACTER_TITLE,
        "Marker files land by fast-forwarding local main",
        "!!!",
        "\u{1f680}\u{1f680}",
        "",
    ] {
        assert_eq!(
            capped_decision_slug(title),
            slug_before_q9l1(title),
            "slug of {title:?}"
        );
    }
}

#[test]
fn two_decisions_with_the_same_long_title_keep_distinct_links() -> Result<()> {
    use super::memory::MemoryGraph;

    let title = "Keep the ledger append-only so none of it is ever overwritten by a replay";
    let ledger = InMemoryEventLedger::new();
    for decision_id in ["decision:long-first", "decision:long-second"] {
        ledger.append(event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": decision_id,
                "title": title,
                "rationale": "Two decisions that happen to share a long title",
                "topic_keys": ["ledger"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ))?;
    }

    let graph = MemoryGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        decision_slug(&graph, "decision:long-first")?,
        Some(GraphValue::String(
            "keep-the-ledger-append-only-so-none-of-it-is-ever".to_owned()
        ))
    );
    assert_eq!(
        decision_slug(&graph, "decision:long-second")?,
        Some(GraphValue::String(
            "keep-the-ledger-append-only-so-none-of-it-is-ever-deci".to_owned()
        )),
        "the later decision's id-tail suffix goes after the whole-word slug"
    );
    Ok(())
}

/// A `decision.scored` assessment annotates the node (see
/// `assert_assessment_annotates_without_rewriting_origin`) without naming `slug`; replaying
/// through it must not change the slug `decision.proposed` already assigned -- the concrete case
/// `assign_decision_slug`'s doc comment promises: nothing projected after the proposal touches
/// this property, so neither would a future retitle (hivemind-nidp).
#[test]
fn a_model_assessment_does_not_change_the_decisions_slug() -> Result<()> {
    use super::memory::MemoryGraph;

    let slug_after = |len: usize| -> Result<Option<GraphValue>> {
        let graph = MemoryGraph::default();
        project_from_ledger(&decision_assessed_fixture_ledger(len)?, &graph, 0)?;
        decision_slug(&graph, "decision:first")
    };

    let before = slug_after(1)?.expect("slug assigned at proposal");
    assert_eq!(
        before,
        GraphValue::String("use-per-seat-pricing".to_owned())
    );
    assert_eq!(
        slug_after(2)?,
        Some(before),
        "a decision.scored annotation must not change the slug decision.proposed assigned"
    );
    Ok(())
}

/// Two decisions proposed by an agent, then the first moved billing -> pricing by a *different*
/// actor over a *different* source, then moved back. The mover's source/source_ref differ from
/// the proposal's on purpose: a projector that spread the move event's origin properties onto
/// the node would visibly rewrite the capture origin. Shared with `projector/postgres/tests.rs`.
fn decision_moved_fixture_events() -> Vec<Event> {
    let proposal = |decision_id: &str| {
        event(
            EventType::DecisionProposed,
            "agent:claude:builder",
            json!({
                "decision_id": decision_id,
                "title": "Use per-seat pricing",
                "rationale": "Simpler to reason about at our scale",
                "topic_keys": ["pricing"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": [],
                "project": "billing",
                "project_source": "stated"
            }),
        )
    };
    let mover_move = |from: &str, to: &str| {
        let mut moved = event(
            EventType::DecisionMoved,
            "human:bob",
            json!({ "decision_id": "decision:first", "from": from, "to": to }),
        );
        moved.source = EventSource::Human;
        moved.source_ref = Some("mover-session".to_owned());
        moved
    };
    vec![
        proposal("decision:first"),
        proposal("decision:second"),
        mover_move("billing", "pricing"),
        mover_move("pricing", "billing"),
    ]
}

/// The first `len` events of [`decision_moved_fixture_events`]: 2 = both proposals, 3 = plus the
/// move, 4 = plus the move back.
pub(super) fn decision_moved_fixture_ledger(len: usize) -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in decision_moved_fixture_events().into_iter().take(len) {
        ledger.append(event)?;
    }
    Ok(ledger)
}

/// One Decision node as the backend stores it (`MemoryGraph` and Postgres both answer a
/// `node.id AS id` lookup with every stored property).
fn decision_row(graph: &impl GraphView, decision_id: &str) -> Result<GraphRow> {
    let rows = graph.query(
        "MATCH (node:`Decision` {id: $id}) RETURN node.id AS id, node.project AS project, node.project_source AS project_source, node.event_origin AS event_origin, node.tenant_id AS tenant_id, node.title AS title, node.former_title AS former_title, node.rationale AS rationale ORDER BY node.id;",
        &GraphParams::from([(
            "id".to_owned(),
            GraphValue::String(decision_id.to_owned()),
        )]),
    )?;
    assert_eq!(rows.len(), 1, "{decision_id} projected exactly once");
    Ok(rows.into_iter().next().expect("one row"))
}

/// Mayor's conformance ruling on hivemind-s15q.10: "nothing is deleted or rewritten". A move
/// changes where a decision is found, not who captured it, so once `decision:first` has been
/// moved (and moved back) its `project`/`project_source` are the move's while its title,
/// rationale, source, source_ref, event_origin and tenant are still the proposal's, and it keeps
/// its place in an event_origin-ordered listing. `graph` holds [`decision_moved_fixture_ledger`]
/// with 3 or 4 events; runs on any backend (`MemoryGraph` here, Postgres in its own tests).
pub(super) fn assert_move_keeps_capture_origin(
    graph: &impl GraphView,
    expected_project: &str,
) -> Result<()> {
    use crate::queries::{
        get_decision_context, get_decision_context_candidates, DecisionContextRequest,
    };

    let proposals = decision_moved_fixture_ledger(2)?.read(0, 10)?;
    let offset_of = |index: usize| {
        i64::try_from(proposals[index].event_id.expect("ledger assigns ids")).expect("fits i64")
    };
    let (first_offset, second_offset) = (offset_of(0), offset_of(1));
    let string = |value: &str| Some(GraphValue::String(value.to_owned()));

    let row = decision_row(graph, "decision:first")?;
    assert_eq!(row.get("project").cloned(), string(expected_project));
    assert_eq!(
        row.get("project_source").cloned(),
        string("moved"),
        "a move stamps project_source = moved, even a move back"
    );
    assert_eq!(row.get("title").cloned(), string("Use per-seat pricing"));
    assert_eq!(
        row.get("rationale").cloned(),
        string("Simpler to reason about at our scale"),
        "a move must not clobber properties it doesn't name"
    );
    assert_eq!(
        row.get("event_origin").cloned(),
        Some(GraphValue::Int(first_offset)),
        "event_origin stays the proposal's ledger offset, not the move's"
    );
    assert_eq!(
        row.get("tenant_id").cloned(),
        string(decision_moved_fixture_events()[0].tenant_id.as_str()),
        "a move must not re-stamp the capture's tenant"
    );

    let context = get_decision_context(graph, "decision:first")?
        .data
        .expect("decision context");
    assert_eq!(
        context.source, "agent",
        "a move must not credit the capture to the mover's source"
    );
    assert_eq!(
        context.source_ref.as_deref(),
        Some("projection-test"),
        "a move must not replace the proposal's source_ref with the mover's"
    );

    // The moved decision keeps its place in an event_origin-ordered listing, and an old moved
    // decision is not "new" to a since-filter that starts at the second proposal.
    let listed = |since_event_origin| -> Result<Vec<String>> {
        Ok(get_decision_context_candidates(
            graph,
            &DecisionContextRequest {
                since_event_origin,
                limit: 10,
                ..Default::default()
            },
        )?
        .data
        .into_iter()
        .map(|context| context.decision_id)
        .collect())
    };
    assert_eq!(listed(None)?, ["decision:first", "decision:second"]);
    assert_eq!(listed(Some(second_offset))?, ["decision:second"]);
    Ok(())
}

#[test]
fn decision_moved_and_back_restores_project_and_keeps_its_capture_origin() -> Result<()> {
    // Acceptance 2 of hivemind-s15q.10 (the history half is in queries/history/tests.rs): move,
    // then move back, on the real merging `MemoryGraph`. The Postgres twin lives in
    // `projector/postgres/tests.rs`.
    use super::memory::MemoryGraph;

    let project_prefix = |len: usize| -> Result<MemoryGraph> {
        let graph = MemoryGraph::default();
        project_from_ledger(&decision_moved_fixture_ledger(len)?, &graph, 0)?;
        Ok(graph)
    };
    let string = |value: &str| Some(GraphValue::String(value.to_owned()));

    // Before any move: the stated project.
    let before_move = decision_row(&project_prefix(2)?, "decision:first")?;
    assert_eq!(before_move.get("project").cloned(), string("billing"));
    assert_eq!(before_move.get("project_source").cloned(), string("stated"));

    // After the move: the new project.
    assert_move_keeps_capture_origin(&project_prefix(3)?, "pricing")?;

    // After the move back: the project is restored (the second move is a fact, not an undo).
    assert_move_keeps_capture_origin(&project_prefix(4)?, "billing")?;

    // Moving one decision leaves the other alone.
    let second = decision_row(&project_prefix(4)?, "decision:second")?;
    assert_eq!(second.get("project").cloned(), string("billing"));
    assert_eq!(second.get("project_source").cloned(), string("stated"));
    Ok(())
}

/// Two decisions proposed by an agent, then the first retitled by a *different* actor over a
/// *different* source, then retitled back (hivemind-ydmp). The retitler's source/source_ref
/// differ from the proposal's on purpose: a projector that spread the retitle event's origin
/// properties onto the node would visibly rewrite the capture origin. Mirrors
/// `decision_moved_fixture_events` exactly. Shared with `projector/postgres/tests.rs`.
fn decision_retitled_fixture_events() -> Vec<Event> {
    let proposal = |decision_id: &str| {
        event(
            EventType::DecisionProposed,
            "agent:claude:builder",
            json!({
                "decision_id": decision_id,
                "title": "A decision with a very long paragraph title that predates the cap",
                "rationale": "Simpler to reason about at our scale",
                "topic_keys": ["pricing"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": [],
                "project": "billing",
                "project_source": "stated"
            }),
        )
    };
    let retitler_retitle = |from: &str, to: &str| {
        let mut retitled = event(
            EventType::DecisionRetitled,
            "human:bob",
            json!({ "decision_id": "decision:first", "from": from, "to": to }),
        );
        retitled.source = EventSource::Human;
        retitled.source_ref = Some("retitler-session".to_owned());
        retitled
    };
    vec![
        proposal("decision:first"),
        proposal("decision:second"),
        retitler_retitle(
            "A decision with a very long paragraph title that predates the cap",
            "A short name",
        ),
        retitler_retitle(
            "A short name",
            "A decision with a very long paragraph title that predates the cap",
        ),
    ]
}

/// The first `len` events of [`decision_retitled_fixture_events`]: 2 = both proposals, 3 = plus
/// the retitle, 4 = plus the retitle back.
pub(super) fn decision_retitled_fixture_ledger(len: usize) -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in decision_retitled_fixture_events().into_iter().take(len) {
        ledger.append(event)?;
    }
    Ok(ledger)
}

/// Mayor's "nothing is deleted or rewritten" conformance ruling (hivemind-s15q.10) applies to
/// retitle too: it changes the decision's current name, not who captured it, so once
/// `decision:first` has been retitled (and retitled back) its `title`/`former_title` are the
/// retitle's while its rationale, source, source_ref, event_origin and tenant are still the
/// proposal's, and it keeps its place in an event_origin-ordered listing. `graph` holds
/// [`decision_retitled_fixture_ledger`] with 3 or 4 events; runs on any backend (`MemoryGraph`
/// here, Postgres in its own tests).
pub(super) fn assert_retitle_keeps_capture_origin(
    graph: &impl GraphView,
    expected_title: &str,
    expected_former_title: &str,
) -> Result<()> {
    use crate::queries::{
        get_decision_context, get_decision_context_candidates, DecisionContextRequest,
    };

    let proposals = decision_retitled_fixture_ledger(2)?.read(0, 10)?;
    let offset_of = |index: usize| {
        i64::try_from(proposals[index].event_id.expect("ledger assigns ids")).expect("fits i64")
    };
    let (first_offset, second_offset) = (offset_of(0), offset_of(1));
    let string = |value: &str| Some(GraphValue::String(value.to_owned()));

    let row = decision_row(graph, "decision:first")?;
    assert_eq!(row.get("title").cloned(), string(expected_title));
    assert_eq!(
        row.get("former_title").cloned(),
        string(expected_former_title),
        "a retitle stamps former_title with the title it replaced, even a retitle back"
    );
    assert_eq!(
        row.get("rationale").cloned(),
        string("Simpler to reason about at our scale"),
        "a retitle must not clobber properties it doesn't name"
    );
    assert_eq!(
        row.get("event_origin").cloned(),
        Some(GraphValue::Int(first_offset)),
        "event_origin stays the proposal's ledger offset, not the retitle's"
    );
    assert_eq!(
        row.get("tenant_id").cloned(),
        string(decision_retitled_fixture_events()[0].tenant_id.as_str()),
        "a retitle must not re-stamp the capture's tenant"
    );

    let context = get_decision_context(graph, "decision:first")?
        .data
        .expect("decision context");
    assert_eq!(
        context.source, "agent",
        "a retitle must not credit the capture to the retitler's source"
    );
    assert_eq!(
        context.source_ref.as_deref(),
        Some("projection-test"),
        "a retitle must not replace the proposal's source_ref with the retitler's"
    );

    // The retitled decision keeps its place in an event_origin-ordered listing, and an old
    // retitled decision is not "new" to a since-filter that starts at the second proposal.
    let listed = |since_event_origin| -> Result<Vec<String>> {
        Ok(get_decision_context_candidates(
            graph,
            &DecisionContextRequest {
                since_event_origin,
                limit: 10,
                ..Default::default()
            },
        )?
        .data
        .into_iter()
        .map(|context| context.decision_id)
        .collect())
    };
    assert_eq!(listed(None)?, ["decision:first", "decision:second"]);
    assert_eq!(listed(Some(second_offset))?, ["decision:second"]);
    Ok(())
}

#[test]
fn decision_retitled_and_back_restores_title_and_keeps_its_capture_origin() -> Result<()> {
    // hivemind-ydmp, mirroring decision_moved_and_back_restores_project_and_keeps_its_capture_origin.
    use super::memory::MemoryGraph;

    let long_title = "A decision with a very long paragraph title that predates the cap";
    let short_title = "A short name";

    let project_prefix = |len: usize| -> Result<MemoryGraph> {
        let graph = MemoryGraph::default();
        project_from_ledger(&decision_retitled_fixture_ledger(len)?, &graph, 0)?;
        Ok(graph)
    };
    let string = |value: &str| Some(GraphValue::String(value.to_owned()));

    // Before any retitle: the proposal's own title, no former_title yet.
    let before_retitle = decision_row(&project_prefix(2)?, "decision:first")?;
    assert_eq!(before_retitle.get("title").cloned(), string(long_title));
    assert_eq!(before_retitle.get("former_title").cloned(), None);

    // After the retitle: the new (short) title, former_title records what it replaced.
    assert_retitle_keeps_capture_origin(&project_prefix(3)?, short_title, long_title)?;

    // After the retitle back: the original is restored (a fact, not an undo).
    assert_retitle_keeps_capture_origin(&project_prefix(4)?, long_title, short_title)?;

    // Retitling one decision leaves the other alone.
    let second = decision_row(&project_prefix(4)?, "decision:second")?;
    assert_eq!(second.get("title").cloned(), string(long_title));
    assert_eq!(second.get("former_title").cloned(), None);
    Ok(())
}

/// One decision proposed by an agent, then assessed twice by a model: a different actor over a
/// different source, and (in the second event) with importance factors. The assessor's
/// `source`, `source_ref` and actor differ from the proposal's on purpose: a projector that spread
/// the scoring event's origin properties onto the node would visibly rewrite the capture origin.
/// Shared with `projector/postgres/tests.rs` and `projector/kuzu/tests.rs`.
fn decision_assessed_fixture_events() -> Vec<Event> {
    let proposal = event(
        EventType::DecisionProposed,
        "agent:claude:builder",
        json!({
            "decision_id": "decision:first",
            "title": "Use per-seat pricing",
            "rationale": "Simpler to reason about at our scale",
            "topic_keys": ["pricing"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": [],
            "project": "billing",
            "project_source": "stated"
        }),
    );
    let assessment = |model: &str, prompt_version: &str, with_importance: bool| {
        let answer = |level: &str| {
            json!({"status": "assessed", "level": level, "explanation": format!("by {model}"),
                   "quote": "Use per-seat pricing"})
        };
        let mut payload = json!({
            "schema_version": 2,
            "decision_id": "decision:first",
            "model": model,
            "prompt_version": prompt_version,
            "dimensions": {
                "framing": answer("partial"),
                "alternatives": {"status": "not_assessed", "reason": "no option is recorded"},
                "information": {"status": "not_assessed", "reason": "nothing cited"},
                "reasoning": answer("solid"),
                "values_tradeoffs": {"status": "not_assessed", "reason": "no tradeoff stated"},
                "bias_exposure": {"status": "not_assessed", "reason": "nothing bears on it"},
                "calibration": {"status": "not_assessed", "reason": "no confidence declared"}
            }
        });
        if with_importance {
            payload["importance"] = json!({
                "stakes": 10.0, "stakes_explanation": "company-wide",
                "irreversibility": 0.25, "irreversibility_explanation": "prices can change",
                "actionability": 1.0, "actionability_explanation": "clear owner"
            });
        }
        let mut scored = event(EventType::DecisionScored, "agent:hivemind:scorer", payload);
        scored.source = EventSource::Api;
        scored.source_ref = Some("scorer-session".to_owned());
        scored
    };
    vec![
        proposal,
        assessment("model-a", "prompt-1", false),
        assessment("model-b", "prompt-2", true),
    ]
}

/// The first `len` events of [`decision_assessed_fixture_events`]: 1 = the proposal, 2 = plus the
/// first assessment, 3 = plus the second.
pub(super) fn decision_assessed_fixture_ledger(len: usize) -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in decision_assessed_fixture_events().into_iter().take(len) {
        ledger.append(event)?;
    }
    Ok(ledger)
}

/// A model's assessment annotates the decision it names and rewrites nothing of the capture: the
/// node keeps the proposal's `event_origin`, `source`, `source_ref`, `tenant_id` and text, and
/// gains `model_assessment` (the newest one) and `model_assessment_origin` (the offset of the
/// event that recorded it). `graph` holds [`decision_assessed_fixture_ledger`] with 2 or 3
/// events; runs on any backend (`MemoryGraph` here, Postgres and Kuzu in their own tests).
pub(super) fn assert_assessment_annotates_without_rewriting_origin(
    graph: &impl GraphView,
    assessments: usize,
) -> Result<()> {
    let events = decision_assessed_fixture_ledger(3)?.read(0, 10)?;
    let offset_of = |index: usize| {
        i64::try_from(events[index].event_id.expect("ledger assigns ids")).expect("fits i64")
    };
    let string = |value: &str| Some(GraphValue::String(value.to_owned()));
    let (model, prompt_version, origin_index) = match assessments {
        1 => ("model-a", "prompt-1", 1),
        _ => ("model-b", "prompt-2", 2),
    };

    let rows = graph.query(
        "MATCH (node:`Decision` {id: $id}) RETURN node.id AS id, node.event_origin AS event_origin, node.source AS source, node.source_ref AS source_ref, node.tenant_id AS tenant_id, node.title AS title, node.rationale AS rationale, node.model_assessment AS model_assessment, node.model_assessment_origin AS model_assessment_origin, node.importance_stakes AS importance_stakes ORDER BY node.id;",
        &GraphParams::from([(
            "id".to_owned(),
            GraphValue::String("decision:first".to_owned()),
        )]),
    )?;
    assert_eq!(rows.len(), 1, "decision:first projected exactly once");
    let row = &rows[0];

    assert_eq!(
        row.get("event_origin").cloned(),
        Some(GraphValue::Int(offset_of(0))),
        "event_origin stays the proposal's ledger offset, not the assessment's"
    );
    assert_eq!(
        row.get("source").cloned(),
        string("agent"),
        "an assessment must not credit the capture to the assessor's source"
    );
    assert_eq!(
        row.get("source_ref").cloned(),
        string("projection-test"),
        "an assessment must not replace the proposal's source_ref"
    );
    assert_eq!(
        row.get("tenant_id").cloned(),
        string(events[0].tenant_id.as_str()),
        "an assessment must not re-stamp the capture's tenant"
    );
    assert_eq!(row.get("title").cloned(), string("Use per-seat pricing"));
    assert_eq!(
        row.get("rationale").cloned(),
        string("Simpler to reason about at our scale"),
        "an assessment must not clobber properties it does not name"
    );

    let Some(GraphValue::String(stored)) = row.get("model_assessment") else {
        return Err(crate::CommandError::Invariant(
            "the assessed decision carries no model_assessment".to_owned(),
        )
        .into());
    };
    let stored: events::ModelAssessment = serde_json::from_str(stored).map_err(projector_error)?;
    assert_eq!(stored.model, model, "the newest assessment is the one kept");
    assert_eq!(stored.prompt_version, prompt_version);
    assert_eq!(
        stored.dimensions.reasoning,
        events::ModelDimension::Assessed {
            level: events::QualityLevel::Solid,
            explanation: format!("by {model}"),
            quote: Some("Use per-seat pricing".to_owned()),
        }
    );
    assert_eq!(
        row.get("model_assessment_origin").cloned(),
        Some(GraphValue::Int(offset_of(origin_index))),
        "the offset of the event that recorded the assessment"
    );
    // Only the second assessment names importance factors.
    assert_eq!(
        row.get("importance_stakes")
            .cloned()
            .filter(|value| !matches!(value, GraphValue::Null)),
        (assessments > 1).then_some(GraphValue::Float(10.0))
    );

    let context = crate::queries::get_decision_context(graph, "decision:first")?
        .data
        .expect("decision context");
    assert_eq!(context.source, "agent");
    assert_eq!(context.source_ref.as_deref(), Some("projection-test"));
    Ok(())
}

#[test]
fn a_model_assessment_annotates_the_decision_without_rewriting_its_capture_origin() -> Result<()> {
    use super::memory::MemoryGraph;

    let project_prefix = |len: usize| -> Result<MemoryGraph> {
        let graph = MemoryGraph::default();
        project_from_ledger(&decision_assessed_fixture_ledger(len)?, &graph, 0)?;
        Ok(graph)
    };

    // Before any assessment the node carries none.
    let before = project_prefix(1)?;
    assert!(crate::queries::get_record_facts(&before, "decision:first")?
        .expect("record facts")
        .model_assessment
        .is_none());

    assert_assessment_annotates_without_rewriting_origin(&project_prefix(2)?, 1)?;
    // The second assessment replaces the first on the node; the first stays in the ledger.
    assert_assessment_annotates_without_rewriting_origin(&project_prefix(3)?, 2)?;
    Ok(())
}

#[test]
fn a_model_assessment_of_a_decision_no_event_created_gets_a_placeholder_origin() -> Result<()> {
    // The write path never records this, but a replayed ledger may hold it: the node named is
    // created as a placeholder carrying the assessment's own origin, exactly as any annotation
    // of a not-yet-recorded node is, and the assessment lands on it.
    use super::memory::MemoryGraph;

    let ledger = InMemoryEventLedger::new();
    let mut events = decision_assessed_fixture_events();
    let assessment = events.remove(1);
    ledger.append(assessment)?;
    let graph = MemoryGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let facts = crate::queries::get_record_facts(&graph, "decision:first")?
        .expect("the named decision exists as a placeholder");
    assert_eq!(facts.event_origin, Some(1));
    assert_eq!(
        facts.model_assessment.map(|assessment| assessment.model),
        Some("model-a".to_owned())
    );
    Ok(())
}

/// An assessment row nobody can parse, as a buggy producer or a hand edit could leave in a shared
/// ledger. The write path refuses it; a reader has to survive it.
pub(super) fn malformed_assessment_event() -> Event {
    event(
        EventType::DecisionScored,
        "agent:hivemind:scorer",
        json!({
            "schema_version": 2,
            "decision_id": "decision:first",
            "model": "model-x",
            "prompt_version": "prompt-x",
            "dimensions": {"framing": "not an answer"}
        }),
    )
}

#[test]
fn a_malformed_assessment_row_is_skipped_and_reported_never_a_failed_replay() -> Result<()> {
    use super::memory::MemoryGraph;

    // The proposal, then the bad row (ledger offset 2), then a good assessment (offset 3).
    let mut events = decision_assessed_fixture_events();
    let good = events.remove(1);
    let proposal = events.remove(0);
    let ledger = InMemoryEventLedger::new();
    ledger.append(proposal)?;
    ledger.append(malformed_assessment_event())?;

    // Alone, the bad row leaves the decision with what its own events say: no assessment.
    let graph = MemoryGraph::default();
    let report = project_from_ledger_for_tenant_reporting(&ledger, &TenantId::local(), &graph, 0)?;
    let facts = crate::queries::get_record_facts(&graph, "decision:first")?
        .expect("the decision is projected");
    assert_eq!(facts.event_origin, Some(1));
    assert!(facts.model_assessment.is_none());
    assert_eq!(report.unreadable_annotations.len(), 1);
    let row = &report.unreadable_annotations[0];
    assert_eq!(row.event_id, Some(2));
    assert_eq!(row.event_type, EventType::DecisionScored);
    assert!(row.reason.contains("does not match event type"));
    // The bad row created nothing: not its decision, not the actor that wrote it.
    assert!(!graph
        .nodes_and_edges()?
        .0
        .contains_key(&(NodeKind::Actor, "agent:hivemind:scorer".to_owned())));

    // A good assessment recorded after it still lands, and the bad row is still reported.
    ledger.append(good)?;
    let graph = MemoryGraph::default();
    let report = project_from_ledger_for_tenant_reporting(&ledger, &TenantId::local(), &graph, 0)?;
    let facts = crate::queries::get_record_facts(&graph, "decision:first")?
        .expect("the decision is projected");
    let assessment = facts
        .model_assessment
        .expect("the readable assessment lands");
    assert_eq!(assessment.model, "model-a");
    assert_eq!(assessment.event_origin, Some(3));
    assert_eq!(report.unreadable_annotations.len(), 1);
    assert_eq!(report.unreadable_annotations[0].event_id, Some(2));

    // The plain entry points succeed too: every existing caller of a rebuild is unchanged.
    let rebuilt = MemoryGraph::default();
    rebuild_graph(&ledger, &rebuilt)?;
    assert!(crate::queries::get_record_facts(&rebuilt, "decision:first")?.is_some());
    Ok(())
}

#[test]
fn a_replay_logs_the_unreadable_rows_once_and_a_clean_replay_logs_nothing() -> Result<()> {
    use super::memory::MemoryGraph;

    let ledger = InMemoryEventLedger::new();
    ledger.append(decision_assessed_fixture_events().remove(0))?;
    for _ in 0..3 {
        ledger.append(malformed_assessment_event())?;
    }

    let subscriber = CapturingSubscriber::default();
    let sink = subscriber.messages.clone();
    tracing::subscriber::with_default(subscriber, || {
        project_from_ledger(&ledger, &MemoryGraph::default(), 0)
    })?;
    let messages = sink.lock().expect("messages lock poisoned").clone();
    assert_eq!(
        messages.len(),
        1,
        "one line for the replay, not one per row: {messages:?}"
    );
    assert!(
        messages[0].contains("3 assessment rows could not be read")
            && messages[0].contains("ledger events 2, 3, 4"),
        "the line names the count and the offsets: {messages:?}"
    );

    let clean = InMemoryEventLedger::new();
    clean.append(decision_assessed_fixture_events().remove(0))?;
    let subscriber = CapturingSubscriber::default();
    let sink = subscriber.messages.clone();
    tracing::subscriber::with_default(subscriber, || {
        project_from_ledger(&clean, &MemoryGraph::default(), 0)
    })?;
    assert!(sink.lock().expect("messages lock poisoned").is_empty());
    Ok(())
}

#[test]
fn a_malformed_event_that_builds_the_graph_still_fails_the_replay() {
    // Only annotations are skippable. A decision whose own event is malformed is what the graph
    // is built from; skipping it would be a silent hole, so the replay still says no.
    let ledger = InMemoryEventLedger::new();
    let mut proposal = decision_assessed_fixture_events().remove(0);
    proposal.payload["title"] = json!("");
    ledger.append(proposal).expect("append does not validate");

    assert!(project_from_ledger(&ledger, &memory::MemoryGraph::default(), 0).is_err());
}

#[test]
fn a_graph_keeps_the_unreadable_rows_it_was_built_past_through_a_clone() {
    use super::memory::MemoryGraph;

    let graph = MemoryGraph::default();
    assert!(graph.unreadable_annotations().is_empty());
    graph.note_unreadable_annotations(Vec::new());
    assert!(graph.unreadable_annotations().is_empty());

    let row = UnreadableAnnotation {
        event_id: Some(7),
        event_type: EventType::DecisionScored,
        reason: "bad".to_owned(),
    };
    graph.note_unreadable_annotations(vec![row.clone()]);
    // A cached graph is cloned and replayed forward; what it skipped comes along.
    let clone = graph.clone();
    assert_eq!(clone.unreadable_annotations(), vec![row.clone()]);
    clone.note_unreadable_annotations(vec![row.clone()]);
    assert_eq!(clone.unreadable_annotations().len(), 2);
    assert_eq!(graph.unreadable_annotations().len(), 1);
}

#[test]
fn decision_proposed_without_project_falls_back_to_personal_address_for_agent_actor() -> Result<()>
{
    // No "project"/"project_source" in the payload at all -- the shape every event
    // predating hivemind-s15q.3 has. Approved record shape item 8: "no migration, no
    // rewrite" -- it must still project, deriving the personal address from actor_id
    // rather than leaving the field empty.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "agent:claude:scribe-42",
        json!({
            "decision_id": "decision:preexisting",
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

    let properties = graph
        .nodes()
        .get(&(NodeKind::Decision, "decision:preexisting".to_owned()))
        .cloned()
        .expect("decision node projected");
    assert_eq!(
        properties.get("project"),
        Some(&GraphValue::String("personal:agent:claude".to_owned())),
        "an event with no project field must project to the recorder's personal project"
    );
    assert_eq!(
        properties.get("project_source"),
        Some(&GraphValue::String("personal_fallback".to_owned()))
    );
    Ok(())
}

#[test]
fn decision_proposed_without_project_falls_back_to_personal_address_for_human_actor() -> Result<()>
{
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "human:alice",
        json!({
            "decision_id": "decision:human-fallback",
            "title": "Use REST",
            "rationale": "Standard practice",
            "topic_keys": ["api"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": [],
            "project_source": "personal_fallback"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let properties = graph
        .nodes()
        .get(&(NodeKind::Decision, "decision:human-fallback".to_owned()))
        .cloned()
        .expect("decision node projected");
    assert_eq!(
        properties.get("project"),
        // human:<id> has no session component to strip.
        Some(&GraphValue::String("personal:human:alice".to_owned()))
    );
    assert_eq!(
        properties.get("project_source"),
        Some(&GraphValue::String("personal_fallback".to_owned()))
    );
    Ok(())
}

#[test]
fn decision_proposed_with_option_labels_stores_label_per_option() -> Result<()> {
    // hivemind-zdsh.3: option_labels must land on each Option node's `label` property so
    // digests/summaries can render titles instead of raw generated option ids.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:labeled",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option:sqs", "option:kafka"],
            "option_labels": ["Amazon SQS", "Kafka"],
            "chosen_option_id": "option:kafka",
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    assert_eq!(
        nodes
            .get(&(NodeKind::Option, "option:sqs".to_owned()))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::String("Amazon SQS".to_owned()))
    );
    assert_eq!(
        nodes
            .get(&(NodeKind::Option, "option:kafka".to_owned()))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::String("Kafka".to_owned()))
    );
    Ok(())
}

#[test]
fn decision_proposed_without_option_labels_stores_no_label() -> Result<()> {
    // Backward compatibility: events written before this field existed (or by a caller that
    // never learned the label) must still project. The "label" property is stored as Null
    // rather than falling back to the id — every reader of it (brief.rs, render.rs,
    // summarize.rs, and search field indexing) already falls back to the id when the
    // property is absent, so a real label and "no label" stay distinguishable in storage.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:unlabeled",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option:legacy"],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        graph
            .nodes()
            .get(&(NodeKind::Option, "option:legacy".to_owned()))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::Null)
    );
    Ok(())
}

#[test]
fn decision_proposed_with_option_descriptions_stores_description_per_option() -> Result<()> {
    // hivemind-zdsh.10: a description captured alongside a label used to be discarded before
    // it ever reached the ledger. It must land on each Option node's `description` property,
    // index-aligned with option_ids the same way option_labels is.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:described",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option:sqs", "option:kafka"],
            "option_labels": ["Amazon SQS", "Kafka"],
            "option_descriptions": ["Fully managed", "More control"],
            "chosen_option_id": "option:kafka",
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    assert_eq!(
        nodes
            .get(&(NodeKind::Option, "option:sqs".to_owned()))
            .and_then(|p| p.get("description")),
        Some(&GraphValue::String("Fully managed".to_owned()))
    );
    assert_eq!(
        nodes
            .get(&(NodeKind::Option, "option:kafka".to_owned()))
            .and_then(|p| p.get("description")),
        Some(&GraphValue::String("More control".to_owned()))
    );
    Ok(())
}

#[test]
fn decision_proposed_without_option_descriptions_stores_no_description() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:undescribed",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option:legacy"],
            "option_labels": ["Amazon SQS"],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        graph
            .nodes()
            .get(&(NodeKind::Option, "option:legacy".to_owned()))
            .and_then(|p| p.get("description")),
        Some(&GraphValue::Null)
    );
    Ok(())
}

#[test]
fn decision_proposed_legacy_slug_option_id_derives_readable_label() -> Result<()> {
    // hivemind-zdsh.10 migration: events from before option_labels existed carry option ids in
    // the old `option-<slugified-label>-<uuid>` shape. Re-projection must derive a readable
    // label from that slug once, rather than showing the raw id or leaving it Null.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:legacy-slug",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option-other-rejected-options-a1b2c3d4-e5f6-47f8-9abc-1234567890ab"],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        graph
            .nodes()
            .get(&(
                NodeKind::Option,
                "option-other-rejected-options-a1b2c3d4-e5f6-47f8-9abc-1234567890ab".to_owned()
            ))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::String("Other rejected options".to_owned()))
    );
    Ok(())
}

/// Projects one `decision.proposed` and returns each option's `label` property in `option_ids`
/// order. `labels` is the event's `option_labels` (`None` = an event from before the field).
fn projected_option_labels(
    option_ids: &[&str],
    labels: Option<&[&str]>,
) -> Result<Vec<GraphValue>> {
    projected_option_property(option_ids, labels, "label")
}

/// `projected_option_labels` for any Option-node property (`label`, `recorded_label`).
fn projected_option_property(
    option_ids: &[&str],
    labels: Option<&[&str]>,
    property: &str,
) -> Result<Vec<GraphValue>> {
    let ledger = InMemoryEventLedger::new();
    let mut payload = json!({
        "decision_id": "decision:labels",
        "title": "Pick one",
        "rationale": "Need a readable record",
        "topic_keys": ["infra"],
        "option_ids": option_ids,
        "chosen_option_id": null,
        "hypothesis_ids": [],
        "evidence_ids": []
    });
    if let Some(labels) = labels {
        payload["option_labels"] = json!(labels);
    }
    ledger.append(event(EventType::DecisionProposed, "actor:alice", payload))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    Ok(option_ids
        .iter()
        .map(|id| {
            nodes
                .get(&(NodeKind::Option, (*id).to_owned()))
                .and_then(|properties| properties.get(property))
                .cloned()
                .unwrap_or(GraphValue::Null)
        })
        .collect())
}

fn strings(values: &[&str]) -> Vec<GraphValue> {
    values
        .iter()
        .map(|value| GraphValue::String((*value).to_owned()))
        .collect()
}

#[test]
fn slug_labels_with_answer_letters_project_as_the_words_after_the_letter() -> Result<()> {
    // hivemind-hk5z: the product-name decision recorded `name-a-upheld` and friends. The stem
    // shared by every option (`name`) and the answer letter are not part of the option.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2", "option:3"],
            Some(&["name-a-upheld", "name-b-standing", "name-c-decisis"]),
        )?,
        strings(&["Upheld", "Standing", "Decisis"])
    );
    // A question number is part of the shared stem, and the letters may arrive out of order.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2", "option:3"],
            Some(&[
                "q1-c-decide-when-first-company-asks",
                "q1-a-drop-the-commercial-licence-line",
                "q1-b-keep-the-line",
            ]),
        )?,
        strings(&[
            "Decide when first company asks",
            "Drop the commercial licence line",
            "Keep the line"
        ])
    );
    Ok(())
}

#[test]
fn slug_labels_without_letters_project_as_words() -> Result<()> {
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2"],
            Some(&["verify-brief-plus-client-parse", "widen-v1-graph"]),
        )?,
        strings(&["Verify brief plus client parse", "Widen v1 graph"])
    );
    Ok(())
}

#[test]
fn single_letter_words_that_are_not_answer_letters_stay() -> Result<()> {
    // `x` and `y` are words here, not the a, b, c... codes of a lettered list.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2"],
            Some(&["use-x-ray-tool", "use-y-axis-scale"]),
        )?,
        strings(&["Use x ray tool", "Use y axis scale"])
    );
    // Letters that repeat are not a list of answers either.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2"],
            Some(&["plan-a-fast", "plan-a-slow"])
        )?,
        strings(&["Plan a fast", "Plan a slow"])
    );
    Ok(())
}

#[test]
fn a_label_that_is_already_words_is_left_as_recorded() -> Result<()> {
    // Capitalised words, a lone lowercase word, and words that only look coded are the
    // caller's own labels: no capitalising, no letter stripping.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2", "option:3"],
            Some(&["Direct CLI", "sqlite", "a new table"]),
        )?,
        strings(&["Direct CLI", "sqlite", "a new table"])
    );
    // A slug next to human labels is turned into words on its own, with nothing stripped.
    assert_eq!(
        projected_option_labels(&["option:1", "option:2"], Some(&["Kafka", "sqs-queue"]))?,
        strings(&["Kafka", "Sqs queue"])
    );
    Ok(())
}

#[test]
fn legacy_ids_with_answer_letters_project_as_the_words_after_the_letter() -> Result<()> {
    // hivemind-hk5z: events from before option_labels carry `option-<slug>-<uuid>`; the slug
    // of a lettered list gets the same treatment as a recorded slug label.
    assert_eq!(
        projected_option_labels(
            &[
                "option-channel-a-claude-code-plugin-first-cd4a4ee9-d211-41a8-8a99-df2009d1a2b3",
                "option-channel-b-mcp-registry-first-18637b40-c454-4123-a0b3-758566d7f3c4",
                "option-channel-c-community-lists-first-dd7c5711-55c0-4345-a47c-d4b907f2e1a9",
            ],
            None,
        )?,
        strings(&[
            "Claude code plugin first",
            "MCP registry first",
            "Community lists first"
        ])
    );
    Ok(())
}

#[test]
fn legacy_ids_with_a_uuid_cut_off_in_the_label_drop_the_fragments() -> Result<()> {
    // The label itself ended in a UUID that the old id generator then cut at a fixed length:
    // `18643473 3836 4f34 8006 d2ae` is not part of what anyone named the option.
    assert_eq!(
        projected_option_labels(
            &[
                "option-option-shelf-a-memory-for-decisions-18643473-3836-4f34-8006-d2ae-d83b7503-cbd9-4b78-98b0-023f0e5c933a",
                "option-option-shelf-b-decision-governance-7321a3a6-6361-4956-97f4-95d8b-59362fe4-8397-4c0a-b721-c8dd370359b0",
                "option-option-shelf-c-adr-tooling-4c2c0683-dd13-4556-9cb4-0ffcc9dcf175-4db0337c-d12a-4a7c-b18a-9a87fec38c56",
            ],
            None,
        )?,
        strings(&["Memory for decisions", "Decision governance", "ADR tooling"])
    );
    Ok(())
}

#[test]
fn acronyms_in_a_slug_keep_their_capitals() -> Result<()> {
    // hivemind-hk5z: a slug only carries acronyms in lowercase; the words read as people write them.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2", "option:3"],
            Some(&["direct-cli", "mcp-server", "rest-api-over-http"]),
        )?,
        strings(&["Direct CLI", "MCP server", "Rest API over HTTP"])
    );
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2", "option:3", "option:4", "option:5"],
            Some(&["ci-gate", "json-lines", "sql-store", "ui-first", "adr-url"]),
        )?,
        strings(&["CI gate", "JSON lines", "SQL store", "UI first", "ADR URL"])
    );
    // Whole words only: `cliff` and `guided` merely contain letters an acronym uses.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2"],
            Some(&["cliff-edge", "guided-tour"]),
        )?,
        strings(&["Cliff edge", "Guided tour"])
    );
    // A lettered set keeps them too, after the stem and the letter are dropped.
    assert_eq!(
        projected_option_labels(
            &["option:1", "option:2"],
            Some(&["channel-a-cli-first", "channel-b-mcp-registry-first"]),
        )?,
        strings(&["CLI first", "MCP registry first"])
    );
    Ok(())
}

#[test]
fn the_recorded_option_text_stays_in_the_graph_beside_the_readable_label() -> Result<()> {
    // hivemind-hk5z: the label is a reading of the ledger; the ledger's own text is not lost
    // to it. Every reader that shows the reading can also show what was recorded.
    let ids = ["option:1", "option:2", "option:3"];
    let labels = Some(&["name-a-upheld", "name-b-standing", "name-c-decisis"][..]);
    assert_eq!(
        projected_option_labels(&ids, labels)?,
        strings(&["Upheld", "Standing", "Decisis"])
    );
    assert_eq!(
        projected_option_property(&ids, labels, "recorded_label")?,
        strings(&["name-a-upheld", "name-b-standing", "name-c-decisis"])
    );
    // A label that was already words is recorded as itself.
    assert_eq!(
        projected_option_property(
            &["option:1", "option:2"],
            Some(&["Direct CLI", "sqlite"]),
            "recorded_label"
        )?,
        strings(&["Direct CLI", "sqlite"])
    );
    // An event from before `option_labels` recorded no label text, only the option id: the
    // words read from the id are a derivation with nothing recorded beside them.
    assert_eq!(
        projected_option_property(
            &["option-batch-delete-7430e283-3854-44c1-aa6e-2232e2349027"],
            None,
            "recorded_label"
        )?,
        vec![GraphValue::Null]
    );
    Ok(())
}

#[test]
fn an_option_without_any_label_text_stays_null_beside_readable_siblings() -> Result<()> {
    // Nothing is invented for an option whose id carries no label (`option-<uuid>`).
    assert_eq!(
        projected_option_labels(
            &[
                "option-9d4f1c2e-6b7a-4c3d-8e9f-0a1b2c3d4e5f",
                "option-batch-delete-7430e283-3854-44c1-aa6e-2232e2349027",
            ],
            None,
        )?,
        vec![
            GraphValue::Null,
            GraphValue::String("Batch delete".to_owned())
        ]
    );
    Ok(())
}

#[test]
fn decision_proposed_bare_opaque_option_id_without_label_stores_null() -> Result<()> {
    // A new-scheme opaque id ("option-<uuid>", nothing left after stripping the prefix but the
    // uuid itself) must not get word-split into meaningless hex chunks when no label is
    // present — it falls back to Null like any other id with nothing to derive.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:opaque",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option-a1b2c3d4-e5f6-47f8-9abc-1234567890ab"],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        graph
            .nodes()
            .get(&(
                NodeKind::Option,
                "option-a1b2c3d4-e5f6-47f8-9abc-1234567890ab".to_owned()
            ))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::Null)
    );
    Ok(())
}

#[test]
fn decision_proposed_option_id_without_a_real_uuid_suffix_stores_null() -> Result<()> {
    // hivemind-zdsh.10 regression guard: the legacy-label deriver must not fire on ids that
    // merely share the "option-" prefix without a genuine trailing UUID — e.g. a short
    // deterministic test/seed id ("option-001-b") or a differently-namespaced scheme
    // ("org:launch:option:public-now" doesn't even share the prefix). Firing on these would
    // invent a label distinction (swapping a hyphen for a space) that was never captured.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:no-real-uuid",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": ["option-001-b", "org:launch:option:public-now"],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    assert_eq!(
        nodes
            .get(&(NodeKind::Option, "option-001-b".to_owned()))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::Null)
    );
    assert_eq!(
        nodes
            .get(&(NodeKind::Option, "org:launch:option:public-now".to_owned()))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::Null)
    );
    Ok(())
}

#[test]
fn strip_trailing_uuid_does_not_panic_on_non_char_boundary_split() {
    // hivemind-zdsh.10 regression: `split_at` is `s.len() - 37`, a byte offset. For a non-ASCII
    // `s` that offset can land inside a multi-byte UTF-8 character instead of on a char
    // boundary. Slicing there must return None, not panic.
    let s = format!("\u{e9}{}", "a".repeat(36)); // 'é' is 2 bytes, so len - 37 == 1 (mid-char)
    assert_eq!(s.len(), 38);
    assert!(!s.is_char_boundary(1));
    assert_eq!(strip_trailing_uuid(&s), None);
}

#[test]
fn decision_proposed_non_ascii_legacy_looking_option_id_does_not_panic() -> Result<()> {
    // End-to-end regression for the same non-char-boundary case, through full re-projection:
    // a legacy-shaped id whose non-ASCII content shifts the trailing-UUID split point off a
    // char boundary must project to Null, not panic the whole replay.
    let non_ascii_id = format!("option-\u{e9}{}", "a".repeat(36));
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:non-ascii-legacy",
            "title": "Pick a queue",
            "rationale": "Need durable delivery",
            "topic_keys": ["infra"],
            "option_ids": [non_ascii_id.clone()],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert_eq!(
        graph
            .nodes()
            .get(&(NodeKind::Option, non_ascii_id))
            .and_then(|p| p.get("label")),
        Some(&GraphValue::Null)
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

/// One `decision` capture as the classifier writes it, naming `actor_id`, `accepted_by` and
/// `rejected_by` and nothing else about who decided.
fn classified_decision_capture(
    actor_id: Option<&str>,
    accepted_by: &[&str],
    rejected_by: &[&str],
) -> serde_json::Value {
    json!({
        "kind": "decision",
        "title": "Run every seat on one model",
        "rationale": "The budget resets on the first",
        "topic_keys": ["usage"],
        "evidence_ids": [],
        "options": ["one model", "two models"],
        "chosen_option": "one model",
        "extraction_confidence": 0.9,
        "expressed_confidence": null,
        "supersedes_id": null,
        "premised_on_ids": [],
        "supports_ids": [],
        "refutes_ids": [],
        "actor_id": actor_id,
        "accepted_by": accepted_by,
        "rejected_by": rejected_by,
        "blocked_actor_id": null,
        "decision_id": null
    })
}

type DecisionEdges = Vec<(RelationKind, String)>;

/// `edges` in the order the graph lists them, so a test can write them in any order.
fn in_graph_order(mut edges: DecisionEdges) -> DecisionEdges {
    edges.sort();
    edges
}

/// The edges of a decision capture projected from a batch recorded at `2026-09-20T10:30:00Z`,
/// as `(relation, actor)` pairs, and the `occurred_at` its node carries.
fn project_classified_decision(capture: serde_json::Value) -> Result<(DecisionEdges, GraphValue)> {
    let mut batch = event(
        EventType::IngestBatchClassified,
        "agent:claude:classifier",
        json!({
            "batch_id": "batch:1",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [capture]
        }),
    );
    batch.ts = Some(
        chrono::DateTime::parse_from_rfc3339("2026-09-20T10:30:00Z")
            .expect("test timestamp parses")
            .with_timezone(&Utc),
    );
    let ledger = InMemoryEventLedger::new();
    ledger.append(batch)?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let (decision_id, properties) = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::Decision)
        .map(|((_, id), properties)| (id.clone(), properties.clone()))
        .expect("decision node from capture"); // ubs:ignore
    let edges = graph
        .edges()
        .keys()
        .filter(|(kind, from, _)| {
            from == &decision_id
                && matches!(
                    kind,
                    RelationKind::AcceptedBy | RelationKind::RejectedBy | RelationKind::ProposedBy
                )
        })
        .map(|(kind, _, to)| (*kind, to.clone()))
        .collect();
    let occurred_at = properties
        .get("occurred_at")
        .cloned()
        .expect("a captured decision carries occurred_at"); // ubs:ignore
    Ok((edges, occurred_at))
}

fn instant(timestamp: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses") // ubs:ignore
        .with_timezone(&Utc)
}

fn time_value(timestamp: &str) -> GraphValue {
    GraphValue::String(instant(timestamp).to_rfc3339())
}

/// A received batch whose turns carry `turn_times` (`None`: a turn with no time), shipped by
/// `session-1`.
fn received_batch(batch_id: &str, turn_times: &[Option<&str>]) -> Event {
    received_batch_in_session(batch_id, "session-1", turn_times)
}

/// A received batch shipped by capture session `session_id`.
fn received_batch_in_session(
    batch_id: &str,
    session_id: &str,
    turn_times: &[Option<&str>],
) -> Event {
    let turns: Vec<serde_json::Value> = turn_times
        .iter()
        .enumerate()
        .map(|(index, ts)| {
            let mut turn = json!({
                "turn_id": format!("t-{index}"),
                "role": "assistant",
                "text": "We run every seat on one model.",
                "truncated": false
            });
            if let Some(ts) = ts {
                turn["ts"] = json!(ts);
            }
            turn
        })
        .collect();
    event(
        EventType::IngestBatchReceived,
        "agent:claude:hook",
        json!({
            "batch_id": batch_id,
            "agent_tool": "claude",
            "session_id": session_id,
            "turns": turns
        }),
    )
}

/// A classification of `batch_ids` holding one `capture`, written at `written_at`.
fn classification(batch_ids: &[&str], capture: serde_json::Value, written_at: &str) -> Event {
    let mut classified = event(
        EventType::IngestBatchClassified,
        "agent:claude:classifier",
        json!({
            "batch_id": batch_ids.first().copied().unwrap_or_default(),
            "batch_ids": batch_ids,
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [capture]
        }),
    );
    classified.ts = Some(instant(written_at));
    classified
}

/// The `occurred_at` of the one decision `graph` holds.
fn the_decision_time(graph: &RecordingGraph) -> GraphValue {
    graph
        .nodes()
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::Decision)
        .and_then(|(_, properties)| properties.get("occurred_at").cloned())
        .expect("the graph holds a decision that carries occurred_at") // ubs:ignore
}

fn project_events(events: Vec<Event>) -> Result<RecordingGraph> {
    let ledger = InMemoryEventLedger::new();
    for event in events {
        ledger.append(event)?;
    }
    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;
    Ok(graph)
}

const SAID_AT: &str = "2026-09-18T06:02:00Z";
const CLASSIFIED_AT: &str = "2026-10-03T03:00:20Z";

/// A decision whose capture names no turn reads as when its batch was said, not as when the
/// classifier ran days later.
#[test]
fn a_decision_classified_later_reads_at_its_batchs_newest_turn_time() -> Result<()> {
    let graph = project_events(vec![
        received_batch(
            "batch:1",
            &[Some("2026-09-18T06:00:00Z"), None, Some(SAID_AT)],
        ),
        classification(
            &["batch:1"],
            classified_decision_capture(None, &[], &[]),
            CLASSIFIED_AT,
        ),
    ])?;

    assert_eq!(the_decision_time(&graph), time_value(SAID_AT)); // ubs:ignore
    Ok(())
}

/// A capture that names its own turn keeps that turn's time: the batch's newest turn is only
/// the fallback.
#[test]
fn a_capture_that_names_its_turn_keeps_that_turns_time() -> Result<()> {
    let mut capture = classified_decision_capture(None, &[], &[]);
    capture["source_turn_id"] = json!("t-0");
    capture["source_ts"] = json!("2026-09-18T06:00:00Z");
    let graph = project_events(vec![
        received_batch("batch:1", &[Some("2026-09-18T06:00:00Z"), Some(SAID_AT)]),
        classification(&["batch:1"], capture, CLASSIFIED_AT),
    ])?;

    assert_eq!(
        // ubs:ignore
        the_decision_time(&graph),
        time_value("2026-09-18T06:00:00Z")
    );
    Ok(())
}

/// A batch whose turns carry no time (everything shipped before turns did) reads at the
/// classification's own time, as before.
#[test]
fn a_batch_with_no_turn_times_reads_at_the_classification_time() -> Result<()> {
    let graph = project_events(vec![
        received_batch("batch:1", &[None, None]),
        classification(
            &["batch:1"],
            classified_decision_capture(None, &[], &[]),
            CLASSIFIED_AT,
        ),
    ])?;

    assert_eq!(the_decision_time(&graph), time_value(CLASSIFIED_AT)); // ubs:ignore
    Ok(())
}

/// A classification of a batch the ledger never received has no turn time to read: nothing is
/// guessed.
#[test]
fn a_classification_of_an_unreceived_batch_reads_at_the_classification_time() -> Result<()> {
    let graph = project_events(vec![classification(
        &["batch:unseen"],
        classified_decision_capture(None, &[], &[]),
        CLASSIFIED_AT,
    )])?;

    assert_eq!(the_decision_time(&graph), time_value(CLASSIFIED_AT)); // ubs:ignore
    Ok(())
}

/// A classification that covers several batches reads at the newest turn time among them.
#[test]
fn a_classification_of_several_batches_reads_at_the_newest_turn_time_among_them() -> Result<()> {
    let graph = project_events(vec![
        received_batch("batch:1", &[Some(SAID_AT)]),
        received_batch("batch:2", &[Some("2026-09-18T07:30:00Z"), None]),
        received_batch("batch:3", &[None]),
        classification(
            &["batch:1", "batch:2", "batch:3"],
            classified_decision_capture(None, &[], &[]),
            CLASSIFIED_AT,
        ),
    ])?;

    assert_eq!(
        // ubs:ignore
        the_decision_time(&graph),
        time_value("2026-09-18T07:30:00Z")
    );
    Ok(())
}

/// A graph extended from an offset (the server's graph cache) reads the same time as one
/// rebuilt from the start, whether the batch was received before the offset or in the same
/// replay as its classification, and projecting the same ledger again changes nothing.
#[test]
fn an_extended_graph_reads_the_same_time_as_a_rebuilt_one() -> Result<()> {
    let capture = classified_decision_capture(None, &[], &[]);
    let ledger = InMemoryEventLedger::new();
    ledger.append(received_batch("batch:1", &[Some(SAID_AT)]))?;
    let extended = RecordingGraph::default();
    project_from_ledger(&ledger, &extended, 0)?;
    let received_up_to = ledger.latest_offset()?;
    ledger.append(event(
        EventType::HypothesisRecorded,
        "agent:claude:hook",
        json!({"hypothesis_id": "hyp-between", "statement": "Something recorded in between."}),
    ))?;
    ledger.append(classification(&["batch:1"], capture, CLASSIFIED_AT))?;

    project_from_ledger(&ledger, &extended, received_up_to)?;
    let rebuilt = RecordingGraph::default();
    project_from_ledger(&ledger, &rebuilt, 0)?;
    let rebuilt_again = RecordingGraph::default();
    project_from_ledger(&ledger, &rebuilt_again, 0)?;

    assert_eq!(the_decision_time(&extended), time_value(SAID_AT)); // ubs:ignore
    assert_eq!(extended.snapshot(), rebuilt.snapshot()); // ubs:ignore
    assert_eq!(rebuilt.snapshot(), rebuilt_again.snapshot()); // ubs:ignore

    Ok(())
}

/// A classification of a batch the ledger never received (a `hivemind emit` capture names a
/// fresh batch id) found by a replay that starts later reads at the classification time too, and
/// is no error.
#[test]
fn an_extended_graph_classifying_an_unreceived_batch_reads_at_the_classification_time() -> Result<()>
{
    let ledger = InMemoryEventLedger::new();
    ledger.append(received_batch("batch:other", &[Some(SAID_AT)]))?;
    let first = ledger.latest_offset()?;
    ledger.append(classification(
        &["batch:unseen"],
        classified_decision_capture(None, &[], &[]),
        CLASSIFIED_AT,
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, first)?;

    assert_eq!(the_decision_time(&graph), time_value(CLASSIFIED_AT)); // ubs:ignore
    Ok(())
}

/// A replay that starts after the first event and holds both a batch and its classification
/// needs nothing from before its offset, and reads the same time as a rebuild.
#[test]
fn a_replay_holding_the_batch_and_its_classification_reads_the_batchs_turn_time() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::HypothesisRecorded,
        "agent:claude:hook",
        json!({"hypothesis_id": "hyp-before", "statement": "Something recorded first."}),
    ))?;
    let first = ledger.latest_offset()?;
    ledger.append(received_batch("batch:1", &[Some(SAID_AT)]))?;
    ledger.append(classification(
        &["batch:1"],
        classified_decision_capture(None, &[], &[]),
        CLASSIFIED_AT,
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, first)?;

    assert_eq!(the_decision_time(&graph), time_value(SAID_AT)); // ubs:ignore
    Ok(())
}

/// The `session_ids` property of the one node of `kind` that `graph` holds (`None` where it
/// carries none).
fn the_session_ids(graph: &RecordingGraph, kind: NodeKind) -> Option<GraphValue> {
    graph
        .nodes()
        .iter()
        .find(|((node_kind, _), _)| *node_kind == kind)
        .and_then(|(_, properties)| properties.get("session_ids").cloned())
}

fn sessions(ids: &[&str]) -> Option<GraphValue> {
    Some(GraphValue::StringList(
        ids.iter().map(|id| (*id).to_owned()).collect(),
    ))
}

/// hivemind-266t: a classified decision carries the capture session that shipped the batches it
/// was classified from, each once and sorted. The session is what the received batch says, not
/// anything read from its id: these batch ids do not look like their session.
#[test]
fn a_classified_decision_carries_the_sessions_that_shipped_its_batches() -> Result<()> {
    let graph = project_events(vec![
        received_batch_in_session("b-1", "sess-b", &[None]),
        received_batch_in_session("b-2", "sess-a", &[None]),
        received_batch_in_session("b-3", "sess-a", &[None]),
        classification(
            &["b-1", "b-2", "b-3"],
            classified_decision_capture(None, &[], &[]),
            CLASSIFIED_AT,
        ),
    ])?;

    assert_eq!(
        // ubs:ignore
        the_session_ids(&graph, NodeKind::Decision),
        sessions(&["sess-a", "sess-b"])
    );
    Ok(())
}

/// Only a decision carries the session: the other node kinds have no such column.
#[test]
fn a_classified_evidence_node_carries_no_session() -> Result<()> {
    let mut evidence = classified_decision_capture(None, &[], &[]);
    evidence["kind"] = json!("evidence");
    let graph = project_events(vec![
        received_batch_in_session("b-1", "sess-a", &[None]),
        classification(&["b-1"], evidence, CLASSIFIED_AT),
    ])?;

    assert_eq!(the_session_ids(&graph, NodeKind::Evidence), None); // ubs:ignore
    Ok(())
}

/// A classification of batches the ledger never received (a `hivemind emit` capture names a
/// fresh batch id) names no session: nothing is guessed, and it is no error.
#[test]
fn a_decision_classified_from_unreceived_batches_carries_no_session() -> Result<()> {
    let graph = project_events(vec![classification(
        &["batch:sess-a:0-5"],
        classified_decision_capture(None, &[], &[]),
        CLASSIFIED_AT,
    )])?;

    assert_eq!(the_session_ids(&graph, NodeKind::Decision), None); // ubs:ignore
    Ok(())
}

/// A graph extended from an offset (the server's graph cache) carries the same sessions as one
/// rebuilt from the start, though the batch was received before the offset.
#[test]
fn an_extended_graph_carries_the_same_sessions_as_a_rebuilt_one() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(received_batch_in_session("b-1", "sess-a", &[None]))?;
    let extended = RecordingGraph::default();
    project_from_ledger(&ledger, &extended, 0)?;
    let received_up_to = ledger.latest_offset()?;
    ledger.append(classification(
        &["b-1"],
        classified_decision_capture(None, &[], &[]),
        CLASSIFIED_AT,
    ))?;

    project_from_ledger(&ledger, &extended, received_up_to)?;
    let rebuilt = RecordingGraph::default();
    project_from_ledger(&ledger, &rebuilt, 0)?;

    assert_eq!(
        // ubs:ignore
        the_session_ids(&extended, NodeKind::Decision),
        sessions(&["sess-a"])
    );
    assert_eq!(extended.snapshot(), rebuilt.snapshot()); // ubs:ignore
    Ok(())
}

/// A ledger bound to one tenant the way the Postgres ledger is: the `_for_tenant` methods honour
/// the tenant they are given, while the plain methods and `bound_tenant` address the bound one.
struct BoundTenantLedger {
    inner: InMemoryEventLedger,
    tenant_id: TenantId,
}

impl EventLedger for BoundTenantLedger {
    fn append_for_tenant(&self, tenant_id: &TenantId, event: Event) -> Result<EventId> {
        self.inner.append_for_tenant(tenant_id, event)
    }

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.inner.read_for_tenant(tenant_id, offset, limit)
    }

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        self.inner
            .replay_from_for_tenant(tenant_id, offset, callback)
    }

    fn latest_offset_for_tenant(&self, tenant_id: &TenantId) -> Result<EventId> {
        self.inner.latest_offset_for_tenant(tenant_id)
    }

    fn bound_tenant(&self) -> TenantId {
        self.tenant_id.clone()
    }

    fn append(&self, event: Event) -> Result<EventId> {
        self.append_for_tenant(&self.tenant_id, event)
    }

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        self.replay_from_for_tenant(&self.tenant_id, offset, callback)
    }

    fn latest_offset(&self) -> Result<EventId> {
        self.latest_offset_for_tenant(&self.tenant_id)
    }
}

/// `project_from_ledger` projects the tenant its ledger is bound to, not `local`: a rebuild and a
/// graph extended from an offset (whose batch lookup reads before the offset) both find the
/// bound tenant's events and read the batch's turn time. The shared-backend Postgres ledger is
/// bound to one tenant per connection; its recall parity tests build their graph this way.
#[test]
fn a_ledger_bound_to_another_tenant_projects_that_tenants_events() -> Result<()> {
    let ledger = BoundTenantLedger {
        inner: InMemoryEventLedger::new(),
        tenant_id: TenantId::new("acme").expect("a tenant id"), // ubs:ignore
    };
    ledger.append(received_batch("batch:1", &[Some(SAID_AT)]))?;
    let received_up_to = ledger.latest_offset()?;
    ledger.append(classification(
        &["batch:1"],
        classified_decision_capture(None, &[], &[]),
        CLASSIFIED_AT,
    ))?;

    let rebuilt = RecordingGraph::default();
    project_from_ledger(&ledger, &rebuilt, 0)?;
    let extended = RecordingGraph::default();
    project_from_ledger(&ledger, &extended, received_up_to)?;

    assert_eq!(the_decision_time(&rebuilt), time_value(SAID_AT)); // ubs:ignore
    assert_eq!(the_decision_time(&extended), time_value(SAID_AT)); // ubs:ignore
    Ok(())
}

#[test]
fn classified_decision_credited_to_a_human_is_not_accepted_but_says_when() -> Result<()> {
    let (edges, occurred_at) =
        project_classified_decision(classified_decision_capture(Some("human:alex"), &[], &[]))?;

    assert_eq!(
        // ubs:ignore
        edges,
        vec![(RelationKind::ProposedBy, "human:alex".to_owned())],
        "actor_id is who proposed, made or reported the item, never its decider"
    );
    assert_eq!(
        // ubs:ignore
        occurred_at,
        GraphValue::String("2026-09-20T10:30:00+00:00".to_owned())
    );
    Ok(())
}

#[test]
fn classified_decision_credited_to_an_agent_or_nobody_is_not_accepted() -> Result<()> {
    let (agent_edges, agent_time) = project_classified_decision(classified_decision_capture(
        Some("agent:claude:mayor"),
        &[],
        &[],
    ))?;
    assert_eq!(
        // ubs:ignore
        agent_edges,
        vec![(RelationKind::ProposedBy, "agent:claude:mayor".to_owned())],
        "an agent's own capture waits for a person to decide it"
    );
    assert_eq!(
        // ubs:ignore
        agent_time,
        GraphValue::String("2026-09-20T10:30:00+00:00".to_owned()),
        "it still says when it was recorded"
    );

    let (nobody_edges, _) =
        project_classified_decision(classified_decision_capture(None, &[], &[]))?;
    assert_eq!(
        // ubs:ignore
        nobody_edges,
        Vec::new(),
        "a capture that names nobody has no decider and no proposer"
    );
    Ok(())
}

#[test]
fn classified_decision_that_names_an_acceptor_or_a_rejecter_is_not_also_accepted_by_its_proposer(
) -> Result<()> {
    let (accepted, _) = project_classified_decision(classified_decision_capture(
        Some("human:alice"),
        &["human:bob"],
        &[],
    ))?;
    assert_eq!(
        // ubs:ignore
        accepted,
        in_graph_order(vec![
            (RelationKind::AcceptedBy, "human:bob".to_owned()),
            (RelationKind::ProposedBy, "human:alice".to_owned()),
        ]),
        "the named acceptor is the decider; the proposer only recorded it"
    );

    let (rejected, _) = project_classified_decision(classified_decision_capture(
        Some("human:alice"),
        &[],
        &["human:bob"],
    ))?;
    assert_eq!(
        // ubs:ignore
        rejected,
        in_graph_order(vec![
            (RelationKind::RejectedBy, "human:bob".to_owned()),
            (RelationKind::ProposedBy, "human:alice".to_owned()),
        ]),
        "a rejected capture is not turned into a contested one on a guess"
    );
    Ok(())
}

#[test]
fn classified_batch_decision_projects_to_the_recorders_personal_project() -> Result<()> {
    // A captured decision names no project either: like a proposal that names none, it belongs
    // to the personal project of whoever recorded it (the batch's actor), never to the decider
    // the classifier read out of the text.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:project",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "Use Postgres for storage",
                "rationale": "Scales well and team knows it",
                "topic_keys": ["storage"],
                "evidence_ids": [],
                "options": ["postgres", "mysql"],
                "chosen_option": "postgres",
                "extraction_confidence": 0.92,
                "actor_id": "human:alice"
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let properties = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::Decision)
        .map(|(_, props)| props.clone())
        .expect("decision node from capture"); // ubs:ignore
    assert_eq!(
        properties.get("project"),
        Some(&GraphValue::String("personal:agent:hivemind".to_owned())),
        "a captured decision projects to the recording actor's personal project"
    );
    assert_eq!(
        properties.get("project_source"),
        Some(&GraphValue::String("personal_fallback".to_owned()))
    );
    Ok(())
}

#[test]
fn classified_batch_under_agent_token_credits_named_human_not_the_token() -> Result<()> {
    // hivemind-zdsh.19, acceptance clause 2: a batch submitted under an agent token
    // (`agent:<tool>:<name>`) in which a human answers must yield a decision credited
    // to that human. The token's agent identity is provenance of the session
    // (INITIATED_BY / PARTICIPATED_BY), never the decider (PROPOSED_BY). The
    // classifier's named-actor extraction is what sets `actor_id`; this pins that the
    // projection keeps the two roles apart.
    let agent_token = "agent:gastown:crew";
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:agent-token",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "Ship the cell behind a feature flag",
                "rationale": "Alex answered the agent's question: flag first, then default on",
                "topic_keys": ["rollout"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": "human:alex@example.com",
                "accepted_by": null,
                "rejected_by": null,
                "blocked_actor_id": null,
                "decision_id": null,
                "participants": [agent_token],
                "session_initiator": agent_token
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let decision_id = graph
        .nodes()
        .keys()
        .find(|(kind, _)| *kind == NodeKind::Decision)
        .map(|(_, id)| id.clone())
        .expect("decision node from capture"); // ubs:ignore

    let edges = graph.edges();
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::ProposedBy,
            decision_id.clone(),
            "human:alex@example.com".to_owned()
        )),
        "the human who answered must be credited as proposer"
    );
    assert!(
        // ubs:ignore
        !edges.contains_key(&(
            RelationKind::ProposedBy,
            decision_id.clone(),
            agent_token.to_owned()
        )),
        "the agent token must not be credited as proposer"
    );
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::InitiatedBy,
            decision_id.clone(),
            agent_token.to_owned()
        )),
        "the agent token is still recorded as the session initiator"
    );
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::ParticipatedBy,
            decision_id,
            agent_token.to_owned()
        )),
        "the agent token is still recorded as a session participant"
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
fn classified_batch_decision_restating_another_projects_a_same_as_edge_newer_to_older() -> Result<()>
{
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:first",
            "title": "Run every town seat on Fable only",
            "rationale": "Hold the spend",
            "topic_keys": ["cost"],
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
            "batch_id": "batch:restates",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "Fable only for every town seat",
                "rationale": "The same ruling, relayed",
                "topic_keys": ["cost"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.85,
                "restates_id": "decision:first"
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let edges = graph.edges();
    let same_as: Vec<_> = edges
        .keys()
        .filter(|(kind, _, _)| *kind == RelationKind::SameAs)
        .collect();
    assert_eq!(same_as.len(), 1, "{same_as:?}");
    let (_, from, to) = same_as[0];
    assert!(from.starts_with("capture:"), "{from}");
    assert_eq!(to, "decision:first");
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
fn classified_batch_resolves_within_batch_title_references() -> Result<()> {
    // No pre-existing ledger state: every cross-reference target is captured
    // in this SAME batch and named only by its title, matching how the
    // classifier must reference a sibling capture (see resolve_batch_local_references).
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:within",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [
                {
                    "kind": "decision",
                    "title": "Old approach",
                    "rationale": "Original choice",
                    "topic_keys": ["arch"],
                    "evidence_ids": [],
                    "options": null,
                    "chosen_option": null,
                    "extraction_confidence": 0.8,
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
                },
                {
                    "kind": "hypothesis",
                    "title": "Caching helps latency",
                    "rationale": "Stated as a proposition to test",
                    "topic_keys": ["cache"],
                    "evidence_ids": [],
                    "options": null,
                    "chosen_option": null,
                    "extraction_confidence": 0.8,
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
                },
                {
                    "kind": "evidence",
                    "title": "Cache hit rate 95% in load test",
                    "rationale": "Load test result",
                    "topic_keys": ["cache"],
                    "evidence_ids": [],
                    "options": null,
                    "chosen_option": null,
                    "extraction_confidence": 0.9,
                    "expressed_confidence": null,
                    "supersedes_id": null,
                    "premised_on_ids": [],
                    "supports_ids": ["Caching helps latency"],
                    "refutes_ids": [],
                    "actor_id": null,
                    "accepted_by": null,
                    "rejected_by": null,
                    "blocked_actor_id": null,
                    "decision_id": null
                },
                {
                    "kind": "decision",
                    "title": "New approach supersedes old",
                    "rationale": "Better fit, backed by the cache load test",
                    "topic_keys": ["arch"],
                    "evidence_ids": ["Cache hit rate 95% in load test"],
                    "options": null,
                    "chosen_option": null,
                    "extraction_confidence": 0.85,
                    "expressed_confidence": null,
                    "supersedes_id": "Old approach",
                    "premised_on_ids": ["Caching helps latency"],
                    "supports_ids": [],
                    "refutes_ids": [],
                    "actor_id": null,
                    "accepted_by": null,
                    "rejected_by": null,
                    "blocked_actor_id": null,
                    "decision_id": null
                }
            ]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    // Two Decision nodes exist ("Old approach" and "New approach..."); resolve
    // which one is "old" by its stored title so edge assertions below aren't
    // order-dependent.
    let old_decision_id = nodes
        .iter()
        .find(|((kind, _), props)| {
            *kind == NodeKind::Decision
                && matches!(props.get("title"), Some(GraphValue::String(t)) if t == "Old approach")
        })
        .map(|((_, id), _)| id.clone())
        .expect("Old approach decision node");
    let hypothesis_id = nodes
        .keys()
        .find(|(kind, _)| *kind == NodeKind::Hypothesis)
        .map(|(_, id)| id.clone())
        .expect("hypothesis node");
    let evidence_id = nodes
        .keys()
        .find(|(kind, _)| *kind == NodeKind::Evidence)
        .map(|(_, id)| id.clone())
        .expect("evidence node");

    let edges = graph.edges();
    assert!(
        edges
            .keys()
            .any(|(kind, _, to)| *kind == RelationKind::Supersedes && *to == old_decision_id),
        "SUPERSEDES must resolve to the sibling Decision's real node id, not the literal title"
    );
    assert!(
        edges
            .keys()
            .any(|(kind, _, to)| *kind == RelationKind::BasedOn && *to == evidence_id),
        "BASED_ON must resolve to the sibling Evidence's real node id, not the literal title"
    );
    assert!(
        edges.keys().any(
            |(kind, _, to)| *kind == RelationKind::PremisedOnDirect && *to == hypothesis_id
        ),
        "PREMISED_ON_DIRECT must resolve to the sibling Hypothesis's real node id, not the literal title"
    );
    assert!(
        edges
            .keys()
            .any(|(kind, _, to)| *kind == RelationKind::Supports && *to == hypothesis_id),
        "SUPPORTS must resolve to the sibling Hypothesis's real node id, not the literal title"
    );
    // None of the resolved edge targets should be the raw title strings —
    // that would mean resolution silently fell through to stub-node creation.
    assert!(
        !edges.keys().any(|(_, _, to)| to == "Old approach"
            || to == "Caching helps latency"
            || to == "Cache hit rate 95% in load test"),
        "no edge should target a raw title string once same-batch resolution ran"
    );
    Ok(())
}

// --- resolve_batch_local_references: duplicate-title warning + resolve counter (hivemind-11nl) ---

fn capture_with_title(kind: &str, title: &str) -> CaptureItem {
    CaptureItem {
        kind: kind.to_owned(),
        title: title.to_owned(),
        rationale: "test rationale".to_owned(),
        topic_keys: vec![],
        evidence_ids: vec![],
        options: None,
        chosen_option: None,
        extraction_confidence: 0.9,
        expressed_confidence: None,
        supersedes_id: None,
        premised_on_ids: vec![],
        supports_ids: vec![],
        refutes_ids: vec![],
        actor_id: None,
        accepted_by: vec![],
        rejected_by: vec![],
        blocked_actor_id: None,
        decision_id: None,
        participants: vec![],
        restates_id: None,
        source_turn_id: None,
        source_ts: None,
        question: None,
        session_initiator: None,
    }
}

/// Minimal `tracing::Subscriber` that records every event's fields (including
/// the format-string `message` field) as debug-formatted strings, so a test
/// can assert a specific `tracing::warn!`/`tracing::debug!` actually fired
/// without pulling in a test-only tracing crate for one assertion.
#[derive(Default)]
struct CapturingSubscriber {
    messages: Arc<Mutex<Vec<String>>>,
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Formatter(String);
        impl tracing::field::Visit for Formatter {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!(" {}={:?}", field.name(), value));
            }
        }
        let mut formatter = Formatter(String::new());
        event.record(&mut formatter);
        self.messages
            .lock()
            .expect("messages lock poisoned")
            .push(formatter.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[test]
fn duplicate_title_in_batch_warns_and_keeps_first_wins() {
    // "id-a" is captured first, "id-b" second; both titled "Same title". A
    // third capture references that title, which must resolve to "id-a"
    // (first-wins, unchanged) while the collision also logs a warning naming
    // the title and both node ids.
    let first = capture_with_title("decision", "Same title");
    let duplicate = capture_with_title("decision", "Same title");
    let mut referencer = capture_with_title("evidence", "Referencer");
    referencer.supports_ids = vec!["Same title".to_owned()];
    let id_captures: [(&str, &CaptureItem); 3] = [
        ("id-a", &first),
        ("id-b", &duplicate),
        ("id-c", &referencer),
    ];

    let subscriber = CapturingSubscriber::default();
    let sink = subscriber.messages.clone();
    let (resolved, _stats) = tracing::subscriber::with_default(subscriber, || {
        resolve_batch_local_references(&id_captures)
    });

    let referencer_resolved = resolved
        .iter()
        .find(|(node_id, _)| node_id == "id-c")
        .map(|(_, capture)| capture)
        .expect("referencer capture present");
    assert_eq!(
        referencer_resolved.supports_ids,
        vec!["id-a".to_owned()],
        "duplicate title must resolve to the FIRST captured node id, not the second"
    );

    let logged = sink.lock().expect("messages lock poisoned");
    assert!(
        logged.iter().any(|m| m.contains("Same title")
            && m.contains("id-a")
            && m.contains("id-b")),
        "duplicate-title collision must log a warning naming the title and both node ids; got: {logged:?}"
    );
}

#[test]
fn resolving_title_reference_increments_resolve_counter() {
    let sibling = capture_with_title("hypothesis", "Sibling hypothesis");
    let mut referencer = capture_with_title("evidence", "Supporting evidence");
    referencer.supports_ids = vec!["Sibling hypothesis".to_owned()];
    // A non-matching value exercises the unresolved/pass-through counter too.
    referencer.refutes_ids = vec!["no such title".to_owned()];
    let id_captures: [(&str, &CaptureItem); 2] =
        [("id-sibling", &sibling), ("id-referencer", &referencer)];

    let (resolved, stats) = resolve_batch_local_references(&id_captures);

    assert_eq!(
        stats.resolved, 1,
        "exactly one relational-id value matched a sibling title and was resolved"
    );
    assert_eq!(
        stats.unresolved, 1,
        "exactly one relational-id value did not match any sibling title"
    );
    let referencer_resolved = resolved
        .iter()
        .find(|(node_id, _)| node_id == "id-referencer")
        .map(|(_, capture)| capture)
        .expect("referencer capture present");
    assert_eq!(
        referencer_resolved.supports_ids,
        vec!["id-sibling".to_owned()]
    );
    assert_eq!(
        referencer_resolved.refutes_ids,
        vec!["no such title".to_owned()],
        "unmatched reference falls through to verbatim pass-through unchanged"
    );
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

#[test]
fn classified_batch_decision_request_plain_ask_uses_decision_requested_by() -> Result<()> {
    // D1/D3 regression guard: a DecisionRequest with an actor_id but no accepted_by/
    // rejected_by position on record is a plain open ask, not a contested one.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:dr-plain",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision-request",
                "title": "launch the EU beta",
                "rationale": "",
                "topic_keys": [],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": "human:priya",
                "accepted_by": [],
                "rejected_by": [],
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let request_node = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::DecisionRequest)
        .map(|((_, id), _)| id.clone())
        .expect("decision request node from capture"); // ubs:ignore
    drop(nodes);

    let edges = graph.edges();
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::DecisionRequestedBy,
            request_node.clone(),
            "human:priya".to_owned()
        )),
        "plain ask must use DecisionRequestedBy"
    );
    assert!(
        // ubs:ignore
        !edges.contains_key(&(
            RelationKind::RequestProposedBy,
            request_node.clone(),
            "human:priya".to_owned()
        )),
        "plain ask must not use RequestProposedBy"
    );
    Ok(())
}

#[test]
fn classified_batch_decision_request_contested_uses_request_proposed_by() -> Result<()> {
    // G1-style contested ask: actor_id (proposer) + one rejecter -> RequestProposedBy /
    // RequestRejectedBy, and NOT the plain-ask DecisionRequestedBy edge.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:dr-contested",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision-request",
                "title": "separate data tier for analytics",
                "rationale": "",
                "topic_keys": [],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": "human:elena",
                "accepted_by": [],
                "rejected_by": ["human:marco"],
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let request_node = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::DecisionRequest)
        .map(|((_, id), _)| id.clone())
        .expect("decision request node from capture"); // ubs:ignore
    drop(nodes);

    let edges = graph.edges();
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::RequestProposedBy,
            request_node.clone(),
            "human:elena".to_owned()
        )),
        "contested ask must use RequestProposedBy"
    );
    assert!(
        // ubs:ignore
        edges.contains_key(&(
            RelationKind::RequestRejectedBy,
            request_node.clone(),
            "human:marco".to_owned()
        )),
        "RequestRejectedBy edge required"
    );
    assert!(
        // ubs:ignore
        !edges.contains_key(&(
            RelationKind::DecisionRequestedBy,
            request_node.clone(),
            "human:elena".to_owned()
        )),
        "contested ask must not use DecisionRequestedBy"
    );
    Ok(())
}

#[test]
fn classified_batch_decision_request_multi_actor_accept_reject() -> Result<()> {
    // G2-style: multiple rejecters on one DecisionRequest, all preserved as
    // separate RequestRejectedBy edges (cardinality, not last-write-wins).
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:dr-multi",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision-request",
                "title": "pricing model for v2 launch",
                "rationale": "",
                "topic_keys": [],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": "human:pat",
                "accepted_by": [],
                "rejected_by": ["human:sam", "human:jo"],
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let request_node = nodes
        .iter()
        .find(|((kind, _), _)| *kind == NodeKind::DecisionRequest)
        .map(|((_, id), _)| id.clone())
        .expect("decision request node from capture"); // ubs:ignore
    drop(nodes);

    let edges = graph.edges();
    for rejecter in ["human:sam", "human:jo"] {
        assert!(
            edges.contains_key(&(
                RelationKind::RequestRejectedBy,
                request_node.clone(),
                rejecter.to_owned()
            )),
            "RequestRejectedBy edge required for {rejecter}"
        );
    }
    Ok(())
}

#[test]
fn classified_batch_decision_multi_actor_accepted_by() -> Result<()> {
    // G3-style: a Decision accepted by two actors — both AcceptedBy edges must
    // survive; accepted_by is a Vec, not a last-write-wins scalar.
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:d-multi-accept",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "migrate to GraphQL",
                "rationale": "batching cut API calls by 40%",
                "topic_keys": [],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": "high",
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": null,
                "accepted_by": ["human:lena", "human:raj"],
                "rejected_by": [],
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
        .map(|((_, id), _)| id.clone())
        .expect("decision node from capture"); // ubs:ignore
    drop(nodes);

    let edges = graph.edges();
    for acceptor in ["human:lena", "human:raj"] {
        assert!(
            edges.contains_key(&(
                RelationKind::AcceptedBy,
                decision_node.clone(),
                acceptor.to_owned()
            )),
            "AcceptedBy edge required for {acceptor}"
        );
    }
    Ok(())
}

// ── Grounding (hivemind-gwhr.1): FOLLOWS_FROM projection, hypothesis kind/check_by/would_change_if ──

#[test]
fn relation_added_follows_from_projects_decision_to_decision_edge() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::RelationAdded,
        "actor:alice",
        json!({
            "relation": "FOLLOWS_FROM",
            "from_id": "decision:later",
            "to_id": "decision:earlier"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert!(
        graph.edges().contains_key(&(
            RelationKind::FollowsFrom,
            "decision:later".to_owned(),
            "decision:earlier".to_owned()
        )),
        "FOLLOWS_FROM edge must project decision -> premise decision"
    );
    Ok(())
}

// ── Questions (hivemind-zdsh.16): a decision ANSWERS a question node ──

#[test]
fn question_recorded_projects_a_question_node_with_its_normalized_text() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::QuestionRecorded,
        "actor:alice",
        json!({
            "question_id": "question:storage",
            "text": "  Which storage ENGINE should the prototype use? "
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let props = nodes
        .get(&(NodeKind::Question, "question:storage".to_owned()))
        .expect("question node present");
    assert_eq!(
        props.get("text"),
        Some(&GraphValue::String(
            "  Which storage ENGINE should the prototype use? ".to_owned()
        )),
        "the words as first written"
    );
    assert_eq!(
        props.get("normalized_text"),
        Some(&GraphValue::String(
            "which storage engine should the prototype use".to_owned()
        )),
        "the form two spellings of one question are compared in"
    );
    Ok(())
}

#[test]
fn relation_added_answers_projects_decision_to_question_edge() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::RelationAdded,
        "actor:alice",
        json!({
            "relation": "ANSWERS",
            "from_id": "decision:pick",
            "to_id": "question:storage"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    assert!(
        graph.edges().contains_key(&(
            RelationKind::Answers,
            "decision:pick".to_owned(),
            "question:storage".to_owned()
        )),
        "ANSWERS edge must project decision -> question"
    );
    assert_eq!(
        RelationKind::Answers.endpoints(),
        (NodeKind::Decision, NodeKind::Question)
    );
    Ok(())
}

#[test]
fn hypothesis_recorded_with_bet_kind_stores_kind_check_by_would_change_if() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::HypothesisRecorded,
        "actor:alice",
        json!({
            "hypothesis_id": "hypothesis:bet",
            "statement": "Latency stays under 50ms at 10x load",
            "kind": "bet",
            "check_by": "2026-10-01T00:00:00Z",
            "would_change_if": "A 10x load test shows p99 above 50ms"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let props = nodes
        .get(&(NodeKind::Hypothesis, "hypothesis:bet".to_owned()))
        .expect("hypothesis node present");
    assert_eq!(
        props.get("kind"),
        Some(&GraphValue::String("bet".to_owned()))
    );
    assert_eq!(
        props.get("check_by"),
        Some(&GraphValue::String("2026-10-01T00:00:00+00:00".to_owned()))
    );
    assert_eq!(
        props.get("would_change_if"),
        Some(&GraphValue::String(
            "A 10x load test shows p99 above 50ms".to_owned()
        ))
    );
    Ok(())
}

#[test]
fn hypothesis_recorded_without_kind_projects_assumption_default() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::HypothesisRecorded,
        "actor:alice",
        json!({
            "hypothesis_id": "hypothesis:plain",
            "statement": "An embedded database is enough for slice-1 write throughput."
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let props = nodes
        .get(&(NodeKind::Hypothesis, "hypothesis:plain".to_owned()))
        .expect("hypothesis node present");
    assert_eq!(
        props.get("kind"),
        Some(&GraphValue::String("assumption".to_owned()))
    );
    assert_eq!(props.get("check_by"), Some(&GraphValue::Null));
    assert_eq!(props.get("would_change_if"), Some(&GraphValue::Null));
    Ok(())
}

// ── Grounding provenance (hivemind-gwhr.3): who added a grounding edge, when, and what caused it ──

fn event_caused_by(
    event_type: EventType,
    actor_id: &str,
    payload: serde_json::Value,
    causation_event_id: Option<u64>,
) -> Event {
    let mut event = event(event_type, actor_id, payload);
    event.causation_event_id = causation_event_id;
    event
}

fn edge_int(properties: &GraphProperties, key: &str) -> Option<i64> {
    match properties.get(key) {
        Some(GraphValue::Int(value)) => Some(*value),
        _ => None,
    }
}

fn edge_text<'a>(properties: &'a GraphProperties, key: &str) -> Option<&'a str> {
    match properties.get(key) {
        Some(GraphValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

#[test]
fn grounding_edges_carry_who_added_them_and_the_proposal_that_caused_them() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let proposal = ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:later",
            "title": "Follows from a goal",
            "rationale": "It is consistent with the goal we set earlier.",
            "topic_keys": ["grounding"],
            "option_ids": [],
            "hypothesis_ids": ["hypothesis:1"],
            "evidence_ids": ["evidence:1"]
        }),
    ))?;
    // Fan-out at capture: caused by the proposal.
    ledger.append(event_caused_by(
        EventType::RelationAdded,
        "actor:alice",
        json!({"relation": "FOLLOWS_FROM", "from_id": "decision:later", "to_id": "decision:goal"}),
        Some(proposal),
    ))?;
    // Attributed afterwards by someone else: no causing proposal.
    ledger.append(event_caused_by(
        EventType::RelationAdded,
        "actor:bob",
        json!({"relation": "FOLLOWS_FROM", "from_id": "decision:later", "to_id": "decision:other"}),
        None,
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let edges = graph.edges();
    let follows = |to: &str| {
        edges
            .get(&(
                RelationKind::FollowsFrom,
                "decision:later".to_owned(),
                to.to_owned(),
            ))
            .expect("FOLLOWS_FROM edge present")
    };
    let at_capture = follows("decision:goal");
    assert_eq!(
        edge_int(at_capture, "causation_event_id"),
        i64::try_from(proposal).ok()
    );
    assert_eq!(edge_text(at_capture, "added_by"), Some("actor:alice"));
    assert!(edge_text(at_capture, "added_at").is_some());

    let later = follows("decision:other");
    assert_eq!(edge_int(later, "causation_event_id"), None);
    assert_eq!(edge_text(later, "added_by"), Some("actor:bob"));

    // Edges named by the proposal's own payload are at capture by definition: same causation.
    for (kind, to) in [
        (RelationKind::PremisedOnDirect, "hypothesis:1"),
        (RelationKind::BasedOn, "evidence:1"),
    ] {
        let payload_edge = edges
            .get(&(kind, "decision:later".to_owned(), to.to_owned()))
            .expect("payload edge present");
        assert_eq!(
            edge_int(payload_edge, "causation_event_id"),
            i64::try_from(proposal).ok(),
            "{kind:?}"
        );
        assert_eq!(
            edge_text(payload_edge, "added_by"),
            Some("actor:alice"),
            "{kind:?}"
        );
    }
    Ok(())
}

#[test]
fn only_grounding_edges_carry_provenance() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:1",
            "title": "One",
            "rationale": "Enough words to read on its own.",
            "topic_keys": ["grounding"],
            "option_ids": ["option:1"],
            "chosen_option_id": "option:1"
        }),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let edges = graph.edges();
    let has_option = edges
        .get(&(
            RelationKind::HasOption,
            "decision:1".to_owned(),
            "option:1".to_owned(),
        ))
        .expect("HAS_OPTION edge present");
    assert!(!has_option.contains_key("added_by"));
    assert!(!has_option.contains_key("causation_event_id"));
    Ok(())
}

#[test]
fn evidence_recorded_projects_where_it_was_seen_and_when() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(event(
        EventType::EvidenceRecorded,
        "actor:alice",
        json!({
            "evidence_id": "evidence:1",
            "content": "27 of 27 captures carry no premise",
            "source": "mayor audit 2026-09-22"
        }),
    ))?;
    ledger.append(event(
        EventType::EvidenceRecorded,
        "actor:alice",
        json!({"evidence_id": "evidence:2", "content": "No source given"}),
    ))?;

    let graph = RecordingGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let nodes = graph.nodes();
    let with_source = nodes
        .get(&(NodeKind::Evidence, "evidence:1".to_owned()))
        .expect("evidence node present");
    assert_eq!(
        with_source.get("evidence_source"),
        Some(&GraphValue::String("mayor audit 2026-09-22".to_owned()))
    );
    assert!(edge_text(with_source, "recorded_at").is_some());
    // The event's channel stays under `source`; the two are different facts.
    assert_eq!(
        with_source.get("source"),
        Some(&GraphValue::String("agent".to_owned()))
    );
    let without_source = nodes
        .get(&(NodeKind::Evidence, "evidence:2".to_owned()))
        .expect("evidence node present");
    assert_eq!(
        without_source.get("evidence_source"),
        Some(&GraphValue::Null)
    );
    Ok(())
}

#[test]
fn memory_graph_serves_grounding_edges_with_provenance_oldest_first() -> Result<()> {
    let graph = memory::MemoryGraph::default();
    let properties = |origin: i64, by: &str, causation: Option<i64>| {
        let mut properties = GraphProperties::from([
            ("event_origin".to_owned(), GraphValue::Int(origin)),
            ("added_by".to_owned(), GraphValue::String(by.to_owned())),
            (
                "added_at".to_owned(),
                GraphValue::String("2026-01-01T00:00:00+00:00".to_owned()),
            ),
        ]);
        if let Some(causation) = causation {
            properties.insert("causation_event_id".to_owned(), GraphValue::Int(causation));
        }
        properties
    };
    // The same premise asserted twice: at capture, then again later by someone else.
    graph.upsert_edge(
        RelationKind::FollowsFrom,
        "decision:a",
        "decision:goal",
        &properties(5, "actor:alice", Some(4)),
    )?;
    graph.upsert_edge(
        RelationKind::FollowsFrom,
        "decision:a",
        "decision:goal",
        &properties(9, "actor:bob", None),
    )?;

    let rows = graph.query(
        "MATCH (a:`Decision` {id: $id})-[r:`FOLLOWS_FROM`]->(b:`Decision`) RETURN b.id AS id, r.event_origin AS event_origin, r.causation_event_id AS causation_event_id, r.added_by AS added_by, r.added_at AS added_at ORDER BY b.id;",
        &GraphParams::from([("id".to_owned(), GraphValue::String("decision:a".to_owned()))]),
    )?;

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("added_by"),
        Some(&GraphValue::String("actor:alice".to_owned()))
    );
    assert_eq!(rows[0].get("causation_event_id"), Some(&GraphValue::Int(4)));
    assert_eq!(
        rows[1].get("added_by"),
        Some(&GraphValue::String("actor:bob".to_owned()))
    );
    assert_eq!(rows[1].get("causation_event_id"), Some(&GraphValue::Null));
    Ok(())
}

#[test]
fn memory_graph_single_node_lookup_honours_the_id_anchor() -> Result<()> {
    let graph = memory::MemoryGraph::default();
    for id in ["evidence:1", "evidence:2"] {
        graph.upsert_node(
            NodeKind::Evidence,
            id,
            &GraphProperties::from([(
                "content".to_owned(),
                GraphValue::String(format!("content of {id}")),
            )]),
        )?;
    }

    let anchored = graph.query(
        "MATCH (node:`Evidence` {id: $id}) RETURN node.id AS id, node.content AS content LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String("evidence:2".to_owned()))]),
    )?;
    assert_eq!(anchored.len(), 1);
    assert_eq!(
        anchored[0].get("content"),
        Some(&GraphValue::String("content of evidence:2".to_owned()))
    );

    let scan = graph.query(
        "MATCH (node:`Evidence`) RETURN node.id AS id, node.content AS content ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    assert_eq!(scan.len(), 2);
    Ok(())
}

// hivemind-zdsh.6: the delegation marker is a Decision-node property upserted by
// `decision.accepted`. `RecordingGraph` replaces a node's whole property map on upsert, so
// these tests use `MemoryGraph` (and, in `postgres/tests.rs`, the JSONB merge), which merge
// the way every real backend does.
#[test]
fn delegated_acceptance_merges_the_marker_onto_the_decision_node() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    for (decision_id, delegated_by) in [
        ("decision:delegated", Some("human:alex")),
        ("decision:alone", None),
    ] {
        ledger.append(event(
            EventType::DecisionProposed,
            "agent:claude:builder",
            json!({
                "decision_id": decision_id,
                "title": "Agent decides for itself",
                "rationale": "A stated reason the projection does not read",
                "topic_keys": ["governance"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ))?;
        let mut accepted = json!({ "decision_id": decision_id });
        if let Some(delegated_by) = delegated_by {
            accepted["delegated_by"] = json!(delegated_by);
        }
        ledger.append(event(
            EventType::DecisionAccepted,
            "agent:claude:builder",
            accepted,
        ))?;
    }

    let graph = memory::MemoryGraph::default();
    project_from_ledger(&ledger, &graph, 0)?;

    let rows = graph.query(
        "MATCH (node:`Decision`) RETURN node.id AS id ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    let row_for = |decision_id: &str| {
        rows.iter()
            .find(|row| row.get("id") == Some(&GraphValue::String(decision_id.to_owned())))
            .cloned()
    };

    let delegated = row_for("decision:delegated").expect("delegated decision projected");
    assert_eq!(
        delegated.get("delegated_by"),
        Some(&GraphValue::String("human:alex".to_owned()))
    );
    // The marker is merged, not substituted: the proposal's own properties survive, and
    // `event_origin` still points at the proposal, not at the accepting event.
    assert_eq!(
        delegated.get("title"),
        Some(&GraphValue::String("Agent decides for itself".to_owned()))
    );
    let proposal_origin = ledger.read(0, 10)?[0].event_id.expect("ledger assigns ids");
    assert_eq!(
        delegated.get("event_origin"),
        Some(&GraphValue::Int(
            i64::try_from(proposal_origin).expect("fits i64")
        ))
    );

    let alone = row_for("decision:alone").expect("undelegated decision projected");
    assert_eq!(
        alone.get("delegated_by"),
        None,
        "an acceptance without the marker must not create the property, not even as null"
    );

    // The acceptance itself is projected the same way with or without the marker.
    let accepted_edges = graph.query(
        "MATCH (from:`Decision`)-[:`ACCEPTED_BY`]->(to:`Actor`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        &GraphParams::new(),
    )?;
    assert_eq!(accepted_edges.len(), 2);
    Ok(())
}

/// A ledger that writes at least one edge of every `RelationKind` through the projector's
/// real event paths, in the natural order (evidence and hypotheses recorded before the
/// decision that names them, actors first appearing when they act).
fn natural_order_scenario() -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in [
        event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({"evidence_id": "evidence:1", "content": "Benchmarks favour Kuzu"}),
        ),
        event(
            EventType::HypothesisRecorded,
            "actor:alice",
            json!({"hypothesis_id": "hypothesis:1", "statement": "Graph projection is viable"}),
        ),
        event(
            EventType::QuestionRecorded,
            "actor:alice",
            json!({"question_id": "question:1", "text": "Which graph store should slice 1 use?"}),
        ),
        event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": "decision:1",
                "title": "Use Kuzu for slice 1",
                "rationale": "Graph queries without extra services",
                "topic_keys": ["architecture"],
                "option_ids": ["option:1", "option:2"],
                "chosen_option_id": "option:2",
                "hypothesis_ids": ["hypothesis:1"],
                "evidence_ids": ["evidence:1"]
            }),
        ),
        event(
            EventType::DecisionAccepted,
            "actor:bob",
            json!({"decision_id": "decision:1"}),
        ),
        event(
            EventType::DecisionRejected,
            "actor:carol",
            json!({"decision_id": "decision:1"}),
        ),
        event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": "decision:2",
                "title": "Use Kuzu with conservative Cypher",
                "rationale": "Keep a backend swap cheap",
                "topic_keys": ["architecture"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hypothesis:1"],
                "evidence_ids": []
            }),
        ),
        event(
            EventType::DecisionSuperseded,
            "actor:alice",
            json!({"old_decision_id": "decision:1", "new_decision_id": "decision:2"}),
        ),
        event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({"evidence_id": "evidence:2", "content": "Prototype projected cleanly"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "SUPPORTS", "from_id": "evidence:2", "to_id": "hypothesis:1"}),
        ),
        event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({"evidence_id": "evidence:3", "content": "Rebuild took an hour"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "REFUTES", "from_id": "evidence:3", "to_id": "hypothesis:1"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "SAME_AS", "from_id": "decision:2", "to_id": "decision:1"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "FOLLOWS_FROM", "from_id": "decision:2", "to_id": "decision:1"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "ANSWERS", "from_id": "decision:1", "to_id": "question:1"}),
        ),
        // ASK_FOR / ASKED_BY: an ask naming a question already recorded above.
        event(
            EventType::QuestionAsked,
            "human:dana",
            json!({"question_id": "question:1", "text": "Which graph store should slice 1 use?"}),
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
                "source_event_ids": [14],
                "dedupe_key": "tenant:release:blocker:release-owner:P1",
                "sent_at": "2026-05-19T10:45:00Z"
            }),
        ),
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "platform", "display_name": "Platform"}),
        ),
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "billing", "display_name": "Billing"}),
        ),
        event(
            EventType::ProjectLinked,
            "actor:alice",
            json!({"from": "billing", "to": "platform", "kind": "part_of"}),
        ),
        event(
            EventType::ProjectLinked,
            "actor:alice",
            json!({"from": "billing", "to": "platform", "kind": "depends_on"}),
        ),
        event(
            EventType::IngestBatchClassified,
            "agent:hivemind:classifier",
            json!({
                "batch_id": "batch:direction-contract",
                "classifier_model": "claude-haiku-4-5-20251001",
                "schema_version": "2",
                "captures": [
                    {
                        "kind": "decision",
                        "title": "Adopt trunk-based development",
                        "rationale": "Short-lived branches merge faster",
                        "topic_keys": ["workflow"],
                        "evidence_ids": [],
                        "options": null,
                        "chosen_option": null,
                        "extraction_confidence": 0.9,
                        "expressed_confidence": null,
                        "supersedes_id": null,
                        "premised_on_ids": [],
                        "supports_ids": [],
                        "refutes_ids": [],
                        "actor_id": "human:dana",
                        "accepted_by": [],
                        "rejected_by": [],
                        "blocked_actor_id": null,
                        "decision_id": null,
                        "participants": ["human:dana", "human:erin"],
                        "session_initiator": "human:dana"
                    },
                    {
                        "kind": "decision-request",
                        "title": "separate data tier for analytics",
                        "rationale": "",
                        "topic_keys": [],
                        "evidence_ids": [],
                        "options": null,
                        "chosen_option": null,
                        "extraction_confidence": 0.9,
                        "expressed_confidence": null,
                        "supersedes_id": null,
                        "premised_on_ids": [],
                        "supports_ids": [],
                        "refutes_ids": [],
                        "actor_id": "human:elena",
                        "accepted_by": ["human:dana"],
                        "rejected_by": ["human:marco"],
                        "blocked_actor_id": null,
                        "decision_id": null
                    }
                ]
            }),
        ),
    ] {
        ledger.append(event)?;
    }
    Ok(ledger)
}

/// `natural_order_scenario` plus one event of every kind it lacks that writes to the graph: a
/// delegated acceptance, a score, a move, a blocker resolution, an acknowledgement, an anchor and a
/// classified batch of the capture kinds it does not carry. Replayed into a graph backend that
/// declares its schema up front, it fails on any node property or relation the projector writes
/// and the backend does not know.
pub(super) fn every_event_scenario() -> Result<InMemoryEventLedger> {
    let ledger = natural_order_scenario()?;
    let notification_id = ledger
        .read(0, 1000)?
        .into_iter()
        .find(|event| event.event_type == EventType::NotificationSent)
        .map(|event| event.event_uuid.to_string())
        .ok_or_else(|| projector_error("the natural-order scenario sends a notification"))?;
    let capture = |kind: &str, title: &str, extra: serde_json::Value| {
        let mut capture = json!({
            "kind": kind,
            "title": title,
            "rationale": "",
            "topic_keys": ["kuzu"],
            "evidence_ids": [],
            "options": null,
            "chosen_option": null,
            "extraction_confidence": 0.9,
            "expressed_confidence": null,
            "supersedes_id": null,
            "premised_on_ids": [],
            "supports_ids": [],
            "refutes_ids": [],
            "actor_id": "human:dana",
            "accepted_by": [],
            "rejected_by": [],
            "blocked_actor_id": null,
            "decision_id": null
        });
        if let (Some(base), Some(extra)) = (capture.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        capture
    };
    let surfaced = event(
        EventType::SuggestionSurfaced,
        "agent:claude:builder",
        json!({
            "finding_id": "finding-0f1e2d3c4b5a69788796a5b4c3d2e1f0",
            "decision_id": "decision:1",
            "recipient_actor_id": "agent:claude:builder",
            "channel": "mcp",
            "sent_at": "2026-05-19T11:30:00Z"
        }),
    );
    let surfaced_id = surfaced.event_uuid.to_string();
    for event in [
        event(
            EventType::DecisionAccepted,
            "agent:claude:builder",
            json!({"decision_id": "decision:2", "delegated_by": "human:alice"}),
        ),
        event(
            EventType::DecisionScored,
            "agent:hivemind:scorer",
            json!({
                "capture_node_id": "decision:1",
                "scorer_model": "claude-haiku-4-5-20251001",
                "weight_version": "v1",
                "supersedes_score_id": null,
                "quality_dims": {
                    "framing": {"score": 0.8, "explanation": "clear"},
                    "alternatives": {"score": 0.7, "explanation": "two options"},
                    "information": {"score": 0.6, "explanation": "some data"},
                    "reasoning": {"score": 0.9, "explanation": "sound"},
                    "values_tradeoffs": {"score": 0.5, "explanation": "implicit"},
                    "bias_exposure": {"score": 0.8, "explanation": "none seen"},
                    "calibration": {"score": 0.7, "explanation": "reasonable"}
                },
                "importance": {
                    "stakes": 10.0,
                    "stakes_explanation": "wide",
                    "irreversibility": 0.6,
                    "irreversibility_explanation": "some cost",
                    "actionability": 1.0,
                    "actionability_explanation": "clear owner"
                }
            }),
        ),
        event(
            EventType::DecisionMoved,
            "actor:alice",
            json!({"decision_id": "decision:1", "from": "billing", "to": "platform"}),
        ),
        event(
            EventType::BlockerResolved,
            "agent:release-bot",
            json!({
                "blocker_id": "blocker:release-owner",
                "resolution_event_id": null,
                "resolution_reason": "owner approved"
            }),
        ),
        event(
            EventType::NotificationAcknowledged,
            "human:release-owner",
            json!({
                "notification_id": notification_id,
                "ack_at": "2026-05-19T11:00:00Z",
                "snooze_until": "2026-05-19T12:00:00Z"
            }),
        ),
        surfaced,
        event(
            EventType::NotificationAcknowledged,
            "agent:claude:builder",
            json!({
                "notification_id": surfaced_id,
                "ack_at": "2026-05-19T11:31:00Z",
                "snooze_until": null,
                "action": "dismissed"
            }),
        ),
        event(
            EventType::ProjectAnchored,
            "actor:alice",
            json!({"handle": "platform", "anchor_kind": "folder", "value": "services/platform"}),
        ),
        event(
            EventType::IngestBatchClassified,
            "agent:hivemind:classifier",
            json!({
                "batch_id": "batch:every-capture-kind",
                "classifier_model": "claude-haiku-4-5-20251001",
                "schema_version": "2",
                "captures": [
                    capture(
                        "evidence",
                        "Latency measured at 12ms",
                        json!({"supports_ids": ["hypothesis:1"]}),
                    ),
                    capture("hypothesis", "Load stays flat", json!({})),
                    capture(
                        "blocker",
                        "Waiting on review",
                        json!({"blocked_actor_id": "human:erin", "decision_id": "decision:1"}),
                    ),
                    capture("notification", "slack", json!({}))
                ]
            }),
        ),
    ] {
        ledger.append(event)?;
    }
    Ok(ledger)
}

#[test]
fn every_event_scenario_writes_every_node_and_relation_kind() -> Result<()> {
    let graph = RecordingGraph::default();
    project_from_ledger(&every_event_scenario()?, &graph, 0)?;

    let nodes = graph.nodes();
    for kind in NodeKind::ALL {
        assert!(
            nodes.keys().any(|(node_kind, _)| *node_kind == kind),
            "the scenario writes no {kind:?} node"
        );
    }
    let edges = graph.edges();
    for kind in RelationKind::ALL {
        assert!(
            edges.keys().any(|(edge_kind, _, _)| *edge_kind == kind),
            "the scenario writes no {kind:?} edge"
        );
    }
    Ok(())
}

// ── Arrow orientation (hivemind-ku1x): every arrow runs newer -> older ──

use super::arrow::{orient, Arrow};
use super::memory::MemoryGraph;
use crate::queries::oriented_edges;

#[test]
fn arrow_labels_exist_for_every_kind_and_only_actor_targets_never_reverse() {
    for kind in RelationKind::ALL {
        let labels = kind.arrow_labels();
        assert!(
            !labels.forward.is_empty(),
            "{kind:?} needs a label for the arrow as stored"
        );
        let (_, target) = kind.endpoints();
        assert_eq!(
            labels.reversed.is_none(),
            target == NodeKind::Actor,
            "{kind:?} reverses exactly when its target is not an actor"
        );
        if let Some(reversed) = labels.reversed {
            assert!(!reversed.is_empty(), "{kind:?} needs a reversed label");
            if kind != RelationKind::SameAs {
                assert_ne!(
                    reversed, labels.forward,
                    "{kind:?} reads the same both ways, so the label would not say which way it runs"
                );
            }
        }
    }
}

#[test]
fn arrow_orient_reverses_only_when_the_stored_target_is_strictly_newer() {
    // BASED_ON is stored decision -> evidence.
    let based_on = |decision_time, evidence_time| {
        orient(
            RelationKind::BasedOn,
            "decision:1",
            decision_time,
            "evidence:1",
            evidence_time,
        )
    };

    let as_stored = based_on(Some(10), Some(4));
    assert!(!as_stored.reversed);
    assert_eq!(
        (as_stored.from_id.as_str(), as_stored.to_id.as_str()),
        ("decision:1", "evidence:1")
    );
    assert_eq!(as_stored.label, "based on");

    for (decision_time, evidence_time) in [(Some(7), Some(7)), (None, Some(7)), (Some(7), None)] {
        let arrow = based_on(decision_time, evidence_time);
        assert!(
            !arrow.reversed,
            "same-age and unknown times keep the stored direction: {decision_time:?} {evidence_time:?}"
        );
    }

    let late = based_on(Some(4), Some(10));
    assert!(late.reversed);
    assert_eq!(
        (late.from_id.as_str(), late.to_id.as_str()),
        ("evidence:1", "decision:1")
    );
    assert_eq!(
        (late.from_kind, late.to_kind),
        (NodeKind::Evidence, NodeKind::Decision)
    );
    assert_eq!(late.label, "informs");
    assert_eq!(late.relation, RelationKind::BasedOn);
    assert_eq!(late.stored_source(), "decision:1");
    assert_eq!(late.stored_target(), "evidence:1");

    // An actor is older than every record, whatever times the caller passes.
    let accepted = orient(
        RelationKind::AcceptedBy,
        "decision:1",
        Some(4),
        "human:ana",
        Some(10),
    );
    assert!(!accepted.reversed);
    assert_eq!(accepted.label, "accepted by");
}

#[test]
fn arrow_table_in_the_graph_contract_doc_matches_the_code() {
    let doc = include_str!("../../docs/GRAPH_CONTRACT.md");
    // The kinds table is the rows whose first cell is a SCREAMING_SNAKE relation name; the
    // field table above it starts with lower-case field names.
    let table_rows: Vec<&str> = doc
        .lines()
        .filter(|line| {
            line.strip_prefix("| `")
                .and_then(|rest| rest.split('`').next())
                .is_some_and(|name| {
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                })
        })
        .collect();
    assert_eq!(
        table_rows.len(),
        RelationKind::ALL.len(),
        "docs/GRAPH_CONTRACT.md must list exactly one row per RelationKind"
    );
    for kind in RelationKind::ALL {
        let (source, target) = kind.endpoints();
        let labels = kind.arrow_labels();
        let expected = format!(
            "| `{}` | {} → {} | {} | {} |",
            kind.table_name(),
            source.table_name(),
            target.table_name(),
            labels.forward,
            labels.reversed.unwrap_or("never"),
        );
        assert!(
            table_rows.iter().any(|row| row.trim_end() == expected),
            "docs/GRAPH_CONTRACT.md must have the row: {expected}"
        );
    }
}

/// `suggestion.surfaced` is the suggestion-side twin of `notification.sent`: a `Notification` node
/// that says which finding was shown to whom, and no blocker (hivemind-m306.4.2). The
/// acknowledgement annotates that node (`ack_at`, `action`) without moving where it was recorded.
#[test]
fn a_surfaced_suggestion_is_a_notification_with_no_blocker_and_its_acknowledgement_marks_it(
) -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let surfaced = event(
        EventType::SuggestionSurfaced,
        "agent:claude:builder",
        json!({
            "finding_id": "finding-0f1e2d3c4b5a69788796a5b4c3d2e1f0",
            "decision_id": "decision:1",
            "recipient_actor_id": "human:alex",
            "channel": "slack",
            "sent_at": "2026-05-19T11:30:00Z"
        }),
    );
    let notification_id = surfaced.event_uuid.to_string();
    let surfaced_origin = ledger.append(surfaced)?;
    // An acknowledgement of a blocker notification (no action) written the old way.
    ledger.append(event(
        EventType::NotificationAcknowledged,
        "human:alex",
        json!({"notification_id": "notification:old-style", "ack_at": "2026-05-19T11:00:00Z", "snooze_until": null}),
    ))?;

    let unacknowledged = RecordingGraph::default();
    project_from_ledger(&ledger, &unacknowledged, 0)?;
    let nodes = unacknowledged.nodes();
    let node = nodes
        .get(&(NodeKind::Notification, notification_id.clone()))
        .expect("the surfaced suggestion is a Notification node");
    let string = |value: &str| Some(GraphValue::String(value.to_owned()));
    assert_eq!(
        node.get("finding_id").cloned(),
        string("finding-0f1e2d3c4b5a69788796a5b4c3d2e1f0")
    );
    assert_eq!(node.get("decision_id").cloned(), string("decision:1"));
    assert_eq!(
        node.get("recipient_actor_id").cloned(),
        string("human:alex")
    );
    assert_eq!(node.get("channel").cloned(), string("slack"));
    assert_eq!(
        node.get("sent_at").cloned(),
        string("2026-05-19T11:30:00+00:00")
    );
    assert_eq!(
        node.get("event_origin"),
        Some(&GraphValue::Int(
            i64::try_from(surfaced_origin).expect("fits")
        ))
    );
    // Not acknowledged yet, and not a notification about a blocker.
    assert_eq!(node.get("ack_at"), None);
    assert_eq!(node.get("blocker_id"), None);
    assert!(nodes.keys().all(|(kind, _)| *kind != NodeKind::Blocker));
    // Naming a notification no event created leaves a bare node with no finding.
    assert_eq!(
        nodes
            .get(&(NodeKind::Notification, "notification:old-style".to_owned()))
            .and_then(|properties| properties.get("finding_id")),
        None
    );
    drop(nodes);
    let edges = unacknowledged.edges();
    assert!(edges.contains_key(&(
        RelationKind::NotificationRecipient,
        notification_id.clone(),
        "human:alex".to_owned()
    )));
    assert!(
        edges
            .keys()
            .all(|(kind, _, _)| *kind != RelationKind::NotificationForBlocker),
        "a finding is not a blocker: no notification-for-blocker fact"
    );
    drop(edges);

    // Acknowledged: the node gains `ack_at` and `action`, and keeps where it was recorded.
    ledger.append(event(
        EventType::NotificationAcknowledged,
        "human:alex",
        json!({
            "notification_id": notification_id,
            "ack_at": "2026-05-19T11:45:00Z",
            "snooze_until": "2026-05-20T00:00:00Z",
            "action": "acted"
        }),
    ))?;
    // `MemoryGraph` merges what an annotation writes into the node, as every real backend does.
    let graph = project_ledger(&ledger)?;
    let row = |id: &str| -> Result<GraphRow> {
        let rows = graph.query(
            "MATCH (node:`Notification` {id: $id}) RETURN node.id AS id ORDER BY node.id;",
            &GraphParams::from([("id".to_owned(), GraphValue::String(id.to_owned()))]),
        )?;
        assert_eq!(rows.len(), 1, "{id} projected exactly once");
        Ok(rows.into_iter().next().expect("one row"))
    };
    let node = row(&notification_id)?;
    assert_eq!(
        node.get("ack_at").cloned(),
        string("2026-05-19T11:45:00+00:00")
    );
    assert_eq!(
        node.get("snooze_until").cloned(),
        string("2026-05-20T00:00:00+00:00")
    );
    assert_eq!(node.get("action").cloned(), string("acted"));
    assert_eq!(
        node.get("finding_id").cloned(),
        string("finding-0f1e2d3c4b5a69788796a5b4c3d2e1f0")
    );
    assert_eq!(
        node.get("event_origin"),
        Some(&GraphValue::Int(
            i64::try_from(surfaced_origin).expect("fits")
        ))
    );
    // The old-style acknowledgement carries no action.
    assert_eq!(
        row("notification:old-style")?.get("action"),
        Some(&GraphValue::Null)
    );
    Ok(())
}

/// Every non-actor node's `event_origin`, read back from the projected graph.
fn recorded_at(graph: &MemoryGraph) -> Result<BTreeMap<NodeKey, i64>> {
    let mut recorded = BTreeMap::new();
    for kind in NodeKind::ALL {
        if kind == NodeKind::Actor {
            continue;
        }
        let rows = graph.query(
            &format!(
                "MATCH (node:`{}`) RETURN node.id AS id, node.event_origin AS event_origin ORDER BY node.id;",
                kind.table_name()
            ),
            &GraphParams::new(),
        )?;
        for row in rows {
            if let (Some(GraphValue::String(id)), Some(GraphValue::Int(origin))) =
                (row.get("id"), row.get("event_origin"))
            {
                recorded.insert((kind, id.clone()), *origin);
            }
        }
    }
    Ok(recorded)
}

/// The rule itself: for every arrow into a record, the arrow starts at a node recorded at or
/// after the one it ends at.
fn assert_arrows_run_newer_to_older(graph: &MemoryGraph, arrows: &[Arrow]) -> Result<()> {
    let recorded = recorded_at(graph)?;
    for arrow in arrows {
        if arrow.to_kind == NodeKind::Actor {
            continue;
        }
        let from = recorded.get(&(arrow.from_kind, arrow.from_id.clone()));
        let to = recorded.get(&(arrow.to_kind, arrow.to_id.clone()));
        let (Some(from), Some(to)) = (from, to) else {
            panic!("no recorded time for the ends of {arrow:?}");
        };
        assert!(
            from >= to,
            "{:?} arrow {} -> {} runs from a node recorded at {from} to one recorded at {to}",
            arrow.relation,
            arrow.from_id,
            arrow.to_id
        );
    }
    Ok(())
}

fn project_ledger(ledger: &InMemoryEventLedger) -> Result<MemoryGraph> {
    let graph = MemoryGraph::default();
    project_from_ledger(ledger, &graph, 0)?;
    Ok(graph)
}

#[test]
fn arrow_keeps_the_stored_direction_when_every_target_was_recorded_first() -> Result<()> {
    let graph = project_ledger(&natural_order_scenario()?)?;
    let arrows = oriented_edges(&graph)?;

    for kind in RelationKind::ALL {
        assert!(
            arrows.iter().any(|arrow| arrow.relation == kind),
            "the natural-order scenario writes no {kind:?} edge"
        );
    }
    let reversed: Vec<&Arrow> = arrows.iter().filter(|arrow| arrow.reversed).collect();
    assert!(
        reversed.is_empty(),
        "no target was recorded after its source, so nothing reverses: {reversed:?}"
    );
    assert_arrows_run_newer_to_older(&graph, &arrows)
}

/// A ledger in which, for every kind that can reverse, the stored target is recorded after the
/// stored source: an edge attached later to something newer, or a forward reference that a
/// later event fills in.
fn late_attachment_scenario() -> Result<InMemoryEventLedger> {
    let proposal = |decision_id: &str, option_ids: &[&str]| {
        event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": decision_id,
                "title": format!("Decision {decision_id}"),
                "rationale": "Recorded for the arrow-direction scenario",
                "topic_keys": ["arrows"],
                "option_ids": option_ids,
                "chosen_option_id": option_ids.first(),
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        )
    };
    let ledger = InMemoryEventLedger::new();
    for event in [
        // SUPPORTS / REFUTES: the evidence is recorded first, the hypothesis it bears on later.
        event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({"evidence_id": "evidence:early", "content": "Recorded before the hypothesis"}),
        ),
        // DECISION_REQUEST_FOR_DECISION: a request names a decision nobody has proposed yet.
        event(
            EventType::DecisionRequested,
            "agent:release-bot",
            json!({
                "topic_keys": ["arrows"],
                "decision_id": "decision:answer",
                "reason": "Needs an owner decision",
                "priority": "P1",
                "required_owner_id": "human:release-owner",
                "authority_class": "human_required",
                "requested_by": "agent:release-bot",
                "client_request_id": "client-request:arrows-1"
            }),
        ),
        // BLOCKER_FOR_DECISION: a blocker waits on a decision nobody has proposed yet.
        event(
            EventType::BlockerReported,
            "agent:release-bot",
            json!({
                "blocker_id": "blocker:early",
                "blocked_actor_id": "agent:release-bot",
                "decision_id": "decision:unblock",
                "topic_keys": ["arrows"],
                "blocked_ref": "run:arrows",
                "blocked_ref_type": "agent_run",
                "reason": "Waiting on a decision that does not exist yet",
                "priority": "P1",
                "last_progress_at": "2026-05-19T10:30:00Z",
                "required_owner_id": "human:release-owner"
            }),
        ),
        // ASK_FOR: an ask names a question nobody has recorded yet.
        event(
            EventType::QuestionAsked,
            "human:dana",
            json!({"question_id": "question:ask-late", "text": "Which arrow runs which way for an ask?"}),
        ),
        // NOTIFICATION_FOR_BLOCKER: a notification names a blocker that is reported later.
        event(
            EventType::NotificationSent,
            "agent:notifier",
            json!({
                "blocker_id": "blocker:late",
                "recipient_actor_id": "human:release-owner",
                "channel": "slack",
                "threshold_rule": "p1_human_required_direct_15m",
                "source_event_ids": [1],
                "dedupe_key": "tenant:arrows:blocker:late:P1",
                "sent_at": "2026-05-19T10:45:00Z"
            }),
        ),
        // The decision every attachment below hangs off, recorded before its targets.
        proposal("decision:old", &["option:old-a"]),
        // Fill in the placeholders above.
        proposal("decision:answer", &[]),
        proposal("decision:unblock", &[]),
        event(
            EventType::BlockerReported,
            "agent:release-bot",
            json!({
                "blocker_id": "blocker:late",
                "blocked_actor_id": "agent:release-bot",
                "decision_id": null,
                "topic_keys": ["arrows"],
                "blocked_ref": "run:arrows-late",
                "blocked_ref_type": "agent_run",
                "reason": "Reported after the notification about it",
                "priority": "P1",
                "last_progress_at": "2026-05-19T10:30:00Z",
                "required_owner_id": null
            }),
        ),
        // Everything below is recorded after `decision:old` and its option.
        proposal("decision:new", &["option:new-a"]),
        event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({"evidence_id": "evidence:late", "content": "Found after the decision"}),
        ),
        event(
            EventType::HypothesisRecorded,
            "actor:alice",
            json!({"hypothesis_id": "hypothesis:late", "statement": "Registered after the evidence"}),
        ),
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "child", "display_name": "Child"}),
        ),
        event(
            EventType::ProjectRegistered,
            "actor:alice",
            json!({"handle": "parent", "display_name": "Parent"}),
        ),
        // The attachments: each edge is stored from the older node to the newer one.
        event(
            EventType::DecisionSuperseded,
            "actor:alice",
            json!({"old_decision_id": "decision:new", "new_decision_id": "decision:old"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "SAME_AS", "from_id": "decision:old", "to_id": "decision:new"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "HAS_OPTION", "from_id": "decision:old", "to_id": "option:new-a"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "CHOSE", "from_id": "decision:old", "to_id": "option:new-a"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "BASED_ON", "from_id": "decision:old", "to_id": "evidence:late"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "ASSUMES", "from_id": "option:old-a", "to_id": "hypothesis:late"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "ASSUMES", "from_id": "decision:old", "to_id": "hypothesis:late"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "SUPPORTS", "from_id": "evidence:early", "to_id": "hypothesis:late"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "REFUTES", "from_id": "evidence:early", "to_id": "hypothesis:late"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "FOLLOWS_FROM", "from_id": "decision:old", "to_id": "decision:new"}),
        ),
        // ANSWERS: the first answer to a question is older than the question node its capture
        // records after the proposal.
        event(
            EventType::QuestionRecorded,
            "actor:alice",
            json!({"question_id": "question:late", "text": "Which arrow runs which way?"}),
        ),
        event(
            EventType::RelationAdded,
            "actor:alice",
            json!({"relation": "ANSWERS", "from_id": "decision:old", "to_id": "question:late"}),
        ),
        // Fills in the placeholder Question the early ASK_FOR above named.
        event(
            EventType::QuestionRecorded,
            "actor:alice",
            json!({"question_id": "question:ask-late", "text": "Which arrow runs which way for an ask?"}),
        ),
        event(
            EventType::ProjectLinked,
            "actor:alice",
            json!({"from": "child", "to": "parent", "kind": "part_of"}),
        ),
        event(
            EventType::ProjectLinked,
            "actor:alice",
            json!({"from": "child", "to": "parent", "kind": "depends_on"}),
        ),
    ] {
        ledger.append(event)?;
    }
    Ok(ledger)
}

#[test]
fn arrow_reverses_for_every_kind_that_can_when_its_target_is_recorded_later() -> Result<()> {
    let graph = project_ledger(&late_attachment_scenario()?)?;
    let arrows = oriented_edges(&graph)?;

    for kind in RelationKind::ALL {
        let (stored_source, stored_target) = kind.endpoints();
        let of_kind: Vec<&Arrow> = arrows
            .iter()
            .filter(|arrow| arrow.relation == kind)
            .collect();
        let Some(reversed_label) = kind.arrow_labels().reversed else {
            assert!(
                of_kind.iter().all(|arrow| !arrow.reversed),
                "{kind:?} points at an actor, which is older than any record"
            );
            continue;
        };

        let reversed: Vec<&&Arrow> = of_kind.iter().filter(|arrow| arrow.reversed).collect();
        assert!(
            !reversed.is_empty(),
            "the scenario reverses no {kind:?} arrow"
        );
        for arrow in reversed {
            assert_eq!(arrow.label, reversed_label, "{kind:?} reversed label");
            assert_eq!(
                (arrow.from_kind, arrow.to_kind),
                (stored_target, stored_source),
                "a reversed {kind:?} arrow runs from the stored target's kind to the stored source's"
            );
        }
    }

    // Spot-check the direction and label on the ones a reader would ask about first.
    let has = |relation, from: &str, to: &str, label: &str| {
        arrows.iter().any(|arrow| {
            arrow.relation == relation
                && arrow.from_id == from
                && arrow.to_id == to
                && arrow.label == label
                && arrow.reversed
        })
    };
    assert!(has(
        RelationKind::BasedOn,
        "evidence:late",
        "decision:old",
        "informs"
    ));
    assert!(has(
        RelationKind::DecisionRequestForDecision,
        "decision:answer",
        &request_id(&graph)?,
        "answers"
    ));
    assert!(has(
        RelationKind::Supersedes,
        "decision:new",
        "decision:old",
        "is superseded by"
    ));

    assert_arrows_run_newer_to_older(&graph, &arrows)
}

/// The id of the one `DecisionRequest` node in the graph.
fn request_id(graph: &MemoryGraph) -> Result<String> {
    let rows = graph.query(
        "MATCH (node:`DecisionRequest`) RETURN node.id AS id ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    match rows.first().and_then(|row| row.get("id")) {
        Some(GraphValue::String(id)) => Ok(id.clone()),
        _ => Err(ProjectorError::Projection("scenario has no decision request".to_owned()).into()),
    }
}

#[test]
fn arrow_orientation_does_not_change_when_a_node_is_annotated_later() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let decision = ledger.append(event(
        EventType::DecisionProposed,
        "actor:alice",
        json!({
            "decision_id": "decision:annotated",
            "title": "Annotated later",
            "rationale": "Nothing that happens to it afterwards may move its event_origin",
            "topic_keys": ["arrows"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ))?;
    let blocker = ledger.append(event(
        EventType::BlockerReported,
        "agent:release-bot",
        json!({
            "blocker_id": "blocker:annotated",
            "blocked_actor_id": "agent:release-bot",
            "decision_id": "decision:annotated",
            "topic_keys": ["arrows"],
            "blocked_ref": "run:annotated",
            "blocked_ref_type": "agent_run",
            "reason": "Waiting",
            "priority": "P1",
            "last_progress_at": "2026-05-19T10:30:00Z",
            "required_owner_id": null
        }),
    ))?;
    let notification_event = event(
        EventType::NotificationSent,
        "agent:notifier",
        json!({
            "blocker_id": "blocker:annotated",
            "recipient_actor_id": "human:release-owner",
            "channel": "slack",
            "threshold_rule": "p1_human_required_direct_15m",
            "source_event_ids": [2],
            "dedupe_key": "tenant:arrows:blocker:annotated:P1",
            "sent_at": "2026-05-19T10:45:00Z"
        }),
    );
    let notification_id = notification_event.event_uuid.to_string();
    let notification = ledger.append(notification_event)?;
    let classified = ledger.append(event(
        EventType::IngestBatchClassified,
        "agent:hivemind:classifier",
        json!({
            "batch_id": "batch:annotated",
            "classifier_model": "claude-haiku-4-5-20251001",
            "schema_version": "2",
            "captures": [{
                "kind": "decision",
                "title": "Captured then scored",
                "rationale": "The score must not move the capture's event_origin",
                "topic_keys": ["arrows"],
                "evidence_ids": [],
                "options": null,
                "chosen_option": null,
                "extraction_confidence": 0.9,
                "expressed_confidence": null,
                "supersedes_id": null,
                "premised_on_ids": [],
                "supports_ids": [],
                "refutes_ids": [],
                "actor_id": "human:dana",
                "accepted_by": [],
                "rejected_by": [],
                "blocked_actor_id": null,
                "decision_id": null
            }]
        }),
    ))?;
    let project = ledger.append(event(
        EventType::ProjectRegistered,
        "actor:alice",
        json!({"handle": "annotated", "display_name": "Annotated"}),
    ))?;

    // Everything below only annotates a node an earlier event created.
    ledger.append(event(
        EventType::BlockerResolved,
        "agent:release-bot",
        json!({"blocker_id": "blocker:annotated", "resolution_event_id": null, "resolution_reason": "done"}),
    ))?;
    ledger.append(event(
        EventType::NotificationAcknowledged,
        "human:release-owner",
        json!({"notification_id": notification_id, "ack_at": "2026-05-19T11:00:00Z", "snooze_until": null}),
    ))?;
    ledger.append(event(
        EventType::ProjectAnchored,
        "actor:alice",
        json!({"handle": "annotated", "anchor_kind": "folder", "value": "services/annotated"}),
    ))?;
    ledger.append(event(
        EventType::DecisionScored,
        "agent:hivemind:scorer",
        json!({
            "capture_node_id": format!("capture:{classified}:0"),
            "scorer_model": "claude-haiku-4-5-20251001",
            "weight_version": "v1",
            "supersedes_score_id": null,
            "quality_dims": {
                "framing": {"score": 0.8, "explanation": "clear"},
                "alternatives": {"score": 0.7, "explanation": "two options"},
                "information": {"score": 0.6, "explanation": "some data"},
                "reasoning": {"score": 0.9, "explanation": "sound"},
                "values_tradeoffs": {"score": 0.5, "explanation": "implicit"},
                "bias_exposure": {"score": 0.8, "explanation": "none seen"},
                "calibration": {"score": 0.7, "explanation": "reasonable"}
            },
            "importance": {
                "stakes": 10.0,
                "stakes_explanation": "wide",
                "irreversibility": 0.6,
                "irreversibility_explanation": "some cost",
                "actionability": 1.0,
                "actionability_explanation": "clear owner"
            }
        }),
    ))?;

    let graph = project_ledger(&ledger)?;
    let recorded = recorded_at(&graph)?;
    let origin = |kind, id: &str| recorded.get(&(kind, id.to_owned())).copied();
    assert_eq!(
        origin(NodeKind::Decision, "decision:annotated"),
        i64::try_from(decision).ok()
    );
    assert_eq!(
        origin(NodeKind::Blocker, "blocker:annotated"),
        i64::try_from(blocker).ok()
    );
    assert_eq!(
        origin(NodeKind::Notification, &notification_id),
        i64::try_from(notification).ok()
    );
    assert_eq!(
        origin(NodeKind::Decision, &format!("capture:{classified}:0")),
        i64::try_from(classified).ok()
    );
    assert_eq!(
        origin(NodeKind::Project, "annotated"),
        i64::try_from(project).ok()
    );
    Ok(())
}

#[test]
fn arrow_annotating_a_node_no_event_created_gives_it_a_placeholder_origin() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let resolved = ledger.append(event(
        EventType::BlockerResolved,
        "agent:release-bot",
        json!({"blocker_id": "blocker:unseen", "resolution_event_id": null, "resolution_reason": "done"}),
    ))?;
    let graph = project_ledger(&ledger)?;
    let recorded = recorded_at(&graph)?;
    assert_eq!(
        recorded
            .get(&(NodeKind::Blocker, "blocker:unseen".to_owned()))
            .copied(),
        i64::try_from(resolved).ok(),
        "a node named before any event created it is placed at the naming event, like any other placeholder"
    );
    Ok(())
}
