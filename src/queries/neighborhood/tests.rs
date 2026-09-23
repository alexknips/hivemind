// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::Result;

use super::*;

fn graph_from_events(events: impl IntoIterator<Item = Event>) -> Result<MemoryGraph> {
    let ledger = InMemoryEventLedger::new();
    for event in events {
        ledger.append(event)?;
    }
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok(graph)
}

fn event(
    sequence: u128,
    event_type: EventType,
    actor_id: &str,
    payload: serde_json::Value,
) -> Event {
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::from_u128(sequence),
        correlation_id: Some("neighborhood-test".to_owned()),
        causation_event_id: None,
        event_type,
        actor_id: actor_id.to_owned(),
        source: EventSource::Cli,
        source_ref: None,
        payload,
        ts: Some(ts("2026-01-01T00:00:00Z")),
    }
}

fn ts(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses")
        .with_timezone(&Utc)
}

fn evidence_recorded(sequence: u128, evidence_id: &str, content: &str) -> Event {
    event(
        sequence,
        EventType::EvidenceRecorded,
        "human:alice",
        json!({ "evidence_id": evidence_id, "content": content, "source": "test" }),
    )
}

/// Alice records the storage decision, Bob accepts it. Two labelled options, one premise
/// hypothesis, one evidence item.
fn storage_decision_events(evidence_content: &str) -> Vec<Event> {
    vec![
        event(
            1,
            EventType::HypothesisRecorded,
            "human:alice",
            json!({
                "hypothesis_id": "hyp:one-ledger",
                "statement": "Every cell reader must see the same ledger"
            }),
        ),
        evidence_recorded(2, "ev:sync-lag", evidence_content),
        event(
            3,
            EventType::DecisionProposed,
            "human:alice",
            json!({
                "decision_id": "d:storage",
                "title": "Demo cell storage moves to shared Postgres backend instead of per-host SQLite",
                "rationale": "One shared ledger keeps every reader consistent without per-host sync",
                "topic_keys": ["storage"],
                "option_ids": ["opt:pg", "opt:sqlite"],
                "option_labels": ["Shared Postgres backend", "Per-host SQLite"],
                "chosen_option_id": "opt:pg",
                "hypothesis_ids": ["hyp:one-ledger"],
                "evidence_ids": ["ev:sync-lag"]
            }),
        ),
        event(
            4,
            EventType::DecisionAccepted,
            "human:bob",
            json!({ "decision_id": "d:storage" }),
        ),
    ]
}

fn node<'a>(view: &'a NeighborhoodView, id: &str) -> &'a NeighborNode {
    view.nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap_or_else(|| panic!("node {id} present in {:?}", view.nodes))
}

#[test]
fn root_carries_the_decision_answer() -> Result<()> {
    let graph = graph_from_events(storage_decision_events(
        "Replica lag hit 40s on the demo host",
    ))?;

    let view = get_decision_neighborhood(&graph, "d:storage", &NeighborhoodRequest::all())?.data;

    assert!(view.root.present);
    let brief = view.root.brief.as_ref().expect("present root has a brief");
    assert_eq!(
        brief.title,
        "Demo cell storage moves to shared Postgres backend instead of per-host SQLite"
    );
    assert_eq!(
        brief.rationale,
        "One shared ledger keeps every reader consistent without per-host sync"
    );
    assert_eq!(brief.status, DecisionStatus::Accepted);
    let chosen = brief.chosen_option.as_ref().expect("chosen option");
    assert_eq!(chosen.label, "Shared Postgres backend");
    let rejected: Vec<&str> = brief
        .rejected_options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    assert_eq!(rejected, vec!["Per-host SQLite"]);
    // Alice recorded it; Bob is who decided it.
    assert_eq!(brief.decided_by.proposer_id.as_deref(), Some("human:alice"));
    assert_eq!(brief.decided_by.decider_ids, vec!["human:bob".to_owned()]);
    assert!(brief.still_holds.held_up);
    Ok(())
}

#[test]
fn neighbour_nodes_carry_their_labels() -> Result<()> {
    let graph = graph_from_events(storage_decision_events(
        "Replica lag hit 40s on the demo host",
    ))?;

    let view = get_decision_neighborhood(&graph, "d:storage", &NeighborhoodRequest::all())?.data;

    assert_eq!(
        node(&view, "d:storage").label.as_deref(),
        Some("Demo cell storage moves to shared Postgres backend instead of per-host SQLite")
    );
    assert_eq!(
        node(&view, "opt:pg").label.as_deref(),
        Some("Shared Postgres backend")
    );
    assert_eq!(
        node(&view, "opt:sqlite").label.as_deref(),
        Some("Per-host SQLite")
    );
    assert_eq!(
        node(&view, "hyp:one-ledger").label.as_deref(),
        Some("Every cell reader must see the same ledger")
    );
    assert_eq!(
        node(&view, "ev:sync-lag").label.as_deref(),
        Some("Replica lag hit 40s on the demo host")
    );
    // An actor's id is its name; there is nothing more to label it with.
    assert_eq!(node(&view, "human:alice").label, None);
    Ok(())
}

#[test]
fn superseding_decision_is_labelled_with_its_title() -> Result<()> {
    let mut events = storage_decision_events("Replica lag hit 40s on the demo host");
    events.push(event(
        5,
        EventType::DecisionProposed,
        "human:alice",
        json!({
            "decision_id": "d:storage-v2",
            "title": "Demo cell storage moves to managed Postgres",
            "rationale": "Self-hosted Postgres cost more operator time than it saved",
            "topic_keys": ["storage"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    ));
    events.push(event(
        6,
        EventType::DecisionSuperseded,
        "human:alice",
        json!({ "old_decision_id": "d:storage", "new_decision_id": "d:storage-v2" }),
    ));
    let graph = graph_from_events(events)?;

    let view = get_decision_neighborhood(&graph, "d:storage", &NeighborhoodRequest::all())?.data;

    assert_eq!(
        node(&view, "d:storage-v2").label.as_deref(),
        Some("Demo cell storage moves to managed Postgres")
    );
    let brief = view.root.brief.as_ref().expect("brief");
    assert!(
        !brief.still_holds.held_up,
        "a superseded decision must say so"
    );
    Ok(())
}

#[test]
fn long_evidence_is_clipped_with_a_visible_ellipsis() -> Result<()> {
    let long = "x".repeat(500);
    let graph = graph_from_events(storage_decision_events(&long))?;

    let view = get_decision_neighborhood(&graph, "d:storage", &NeighborhoodRequest::all())?.data;

    let label = node(&view, "ev:sync-lag").label.as_deref().expect("label");
    assert_eq!(label.chars().count(), NODE_LABEL_MAX_CHARS);
    assert!(label.ends_with('…'));
    Ok(())
}

#[test]
fn missing_decision_has_no_brief_and_serializes_without_answer_fields() -> Result<()> {
    let graph = graph_from_events(storage_decision_events("lag"))?;

    let response = get_decision_neighborhood(&graph, "no-such", &NeighborhoodRequest::all())?;

    assert!(!response.data.root.present);
    assert_eq!(response.data.root.brief, None);
    assert_eq!(
        serde_json::to_value(&response.data.root).expect("root serializes"),
        json!({ "id": "no-such", "kind": "decision", "present": false })
    );
    Ok(())
}

#[test]
fn root_json_is_flat_beside_the_id() -> Result<()> {
    let graph = graph_from_events(storage_decision_events("lag"))?;

    let view = get_decision_neighborhood(&graph, "d:storage", &NeighborhoodRequest::all())?.data;
    let root = serde_json::to_value(&view.root).expect("root serializes");

    assert_eq!(root["id"], "d:storage");
    assert_eq!(root["present"], true);
    assert_eq!(
        root["title"],
        "Demo cell storage moves to shared Postgres backend instead of per-host SQLite"
    );
    assert_eq!(root["chosen_option"]["label"], "Shared Postgres backend");
    assert_eq!(root["rejected_options"][0]["label"], "Per-host SQLite");
    assert_eq!(root["decided_by"]["decider_ids"][0], "human:bob");
    assert_eq!(root["status"], "accepted");
    Ok(())
}

#[test]
fn structure_has_the_same_graph_without_labels_or_brief() -> Result<()> {
    let graph = graph_from_events(storage_decision_events("lag"))?;
    let request = NeighborhoodRequest::all();

    let full = get_decision_neighborhood(&graph, "d:storage", &request)?;
    let bare = neighborhood_structure(&graph, "d:storage", &request)?;

    assert_eq!(bare.data.root.brief, None);
    assert!(bare.data.nodes.iter().all(|node| node.label.is_none()));
    assert_eq!(bare.data.edges, full.data.edges);
    assert_eq!(bare.result_count, full.result_count);
    assert_eq!(bare.truncated, full.truncated);
    let full_ids: Vec<&str> = full.data.nodes.iter().map(|n| n.id.as_str()).collect();
    let bare_ids: Vec<&str> = bare.data.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(bare_ids, full_ids);
    Ok(())
}
