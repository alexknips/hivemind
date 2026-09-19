// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType, RelationKind as EventRelationKind};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::Result;

use super::*;

fn graph_and_ledger(
    events: impl IntoIterator<Item = Event>,
) -> Result<(InMemoryEventLedger, MemoryGraph)> {
    let ledger = InMemoryEventLedger::new();
    for event in events {
        ledger.append(event)?;
    }
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok((ledger, graph))
}

fn event(
    sequence: u128,
    event_type: EventType,
    actor_id: &str,
    payload: serde_json::Value,
    timestamp: &str,
) -> Event {
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::from_u128(sequence),
        correlation_id: Some("decision-log-test".to_owned()),
        causation_event_id: None,
        event_type,
        actor_id: actor_id.to_owned(),
        source: EventSource::Cli,
        source_ref: Some("decision-log-test".to_owned()),
        payload,
        ts: Some(ts(timestamp)),
    }
}

fn ts(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses")
        .with_timezone(&Utc)
}

fn decision_event(
    sequence: u128,
    decision_id: &str,
    title: &str,
    timestamp: &str,
    hypothesis_ids: &[&str],
    evidence_ids: &[&str],
) -> Event {
    event(
        sequence,
        EventType::DecisionProposed,
        "human:alice",
        json!({
            "decision_id": decision_id,
            "title": title,
            "rationale": format!("Rationale for {title}"),
            "topic_keys": ["storage"],
            "option_ids": [format!("{decision_id}-a"), format!("{decision_id}-b")],
            "chosen_option_id": format!("{decision_id}-b"),
            "hypothesis_ids": hypothesis_ids,
            "evidence_ids": evidence_ids,
        }),
        timestamp,
    )
}

#[test]
fn export_decision_log_composes_a_clean_decision() -> Result<()> {
    let events = vec![
        event(
            1,
            EventType::EvidenceRecorded,
            "actor:researcher",
            json!({"evidence_id": "evidence-1", "content": "Latency dropped after the switch", "source": "seed"}),
            "2026-01-01T00:00:00Z",
        ),
        event(
            2,
            EventType::HypothesisRecorded,
            "actor:researcher",
            json!({"hypothesis_id": "hypothesis-1", "statement": "Postgres scales better than SQLite here"}),
            "2026-01-01T00:00:01Z",
        ),
        decision_event(
            3,
            "decision-1",
            "Adopt Postgres",
            "2026-01-02T00:00:00Z",
            &["hypothesis-1"],
            &["evidence-1"],
        ),
        event(
            4,
            EventType::RelationAdded,
            "actor:analyst",
            json!({"relation": EventRelationKind::Supports, "from_id": "evidence-1", "to_id": "hypothesis-1"}),
            "2026-01-02T00:00:01Z",
        ),
        event(
            5,
            EventType::DecisionAccepted,
            "human:alice",
            json!({"decision_id": "decision-1"}),
            "2026-01-02T00:00:02Z",
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;

    assert_eq!(export.ledger_offset, ledger.latest_offset()?);
    assert_eq!(export.files.len(), 2, "INDEX.md plus one decision file");
    assert!(export.files.contains_key("INDEX.md"));

    let (path, content) = export
        .files
        .iter()
        .find(|(path, _)| path.starts_with("decisions/"))
        .expect("one decision file");
    assert_eq!(path, "decisions/2026-01-02-adopt-postgres-1.md");
    assert!(content.starts_with("---\n"));
    assert!(content.contains("id: \"decision-1\""));
    assert!(content.contains("status: \"accepted\""));
    assert!(content.contains("# Adopt Postgres"));
    assert!(content.contains("Postgres scales better than SQLite here"));
    assert!(content.contains("supported"));
    assert!(content.ends_with('\n'));

    let index = export.files.get("INDEX.md").expect("index present");
    assert!(index.contains("Do not edit; regenerate with `hivemind export`."));
    assert!(index.contains("[Adopt Postgres](decisions/2026-01-02-adopt-postgres-1.md)"));
    Ok(())
}

#[test]
fn export_decision_log_is_deterministic() -> Result<()> {
    let events = vec![
        decision_event(
            1,
            "decision-1",
            "Adopt Postgres",
            "2026-01-02T00:00:00Z",
            &[],
            &[],
        ),
        decision_event(
            2,
            "decision-2",
            "Adopt Kafka",
            "2026-01-03T00:00:00Z",
            &[],
            &[],
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let first = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;
    let second = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;

    assert_eq!(first.files, second.files);
    Ok(())
}

#[test]
fn export_decision_log_disambiguates_same_title_same_day() -> Result<()> {
    let events = vec![
        decision_event(
            1,
            "decision-alpha",
            "Adopt Postgres",
            "2026-01-02T00:00:00Z",
            &[],
            &[],
        ),
        decision_event(
            2,
            "decision-beta",
            "Adopt Postgres",
            "2026-01-02T12:00:00Z",
            &[],
            &[],
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;

    let decision_files: Vec<&String> = export
        .files
        .keys()
        .filter(|path| path.starts_with("decisions/"))
        .collect();
    assert_eq!(decision_files.len(), 2);
    assert_ne!(decision_files[0], decision_files[1]);
    for path in decision_files {
        assert!(path.starts_with("decisions/2026-01-02-adopt-postgres-"));
    }
    Ok(())
}

#[test]
fn export_decision_log_marks_refuted_hypothesis_and_links_successor_in_set() -> Result<()> {
    let events = vec![
        event(
            1,
            EventType::EvidenceRecorded,
            "actor:researcher",
            json!({"evidence_id": "evidence-1", "content": "Packet capture disproves the assumption", "source": "seed"}),
            "2026-01-01T00:00:00Z",
        ),
        event(
            2,
            EventType::HypothesisRecorded,
            "actor:researcher",
            json!({"hypothesis_id": "hypothesis-1", "statement": "The old cache layer is safe to keep"}),
            "2026-01-01T00:00:01Z",
        ),
        decision_event(
            3,
            "decision-old",
            "Keep the cache layer",
            "2026-01-02T00:00:00Z",
            &["hypothesis-1"],
            &[],
        ),
        event(
            4,
            EventType::RelationAdded,
            "actor:auditor",
            json!({"relation": EventRelationKind::Refutes, "from_id": "evidence-1", "to_id": "hypothesis-1"}),
            "2026-01-02T00:00:01Z",
        ),
        decision_event(
            5,
            "decision-new",
            "Replace the cache layer",
            "2026-01-03T00:00:00Z",
            &[],
            &[],
        ),
        event(
            6,
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({"old_decision_id": "decision-old", "new_decision_id": "decision-new"}),
            "2026-01-03T00:00:01Z",
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;
    let old_file = export
        .files
        .iter()
        .find(|(path, _)| path.contains("keep-the-cache-layer"))
        .map(|(_, content)| content)
        .expect("superseded decision file present");

    assert!(old_file.contains("**refuted**"));
    assert!(old_file.contains("Status: superseded → [Replace the cache layer]"));
    assert!(old_file.contains("superseded_by: \"decision-new\""));
    Ok(())
}

#[test]
fn export_decision_log_names_successor_as_text_when_filtered_out() -> Result<()> {
    let events = vec![
        decision_event(
            1,
            "decision-old",
            "Keep the cache layer",
            "2026-01-02T00:00:00Z",
            &[],
            &[],
        ),
        decision_event(
            2,
            "decision-new",
            "Replace the cache layer",
            "2026-01-03T00:00:00Z",
            &[],
            &[],
        ),
        event(
            3,
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({"old_decision_id": "decision-old", "new_decision_id": "decision-new"}),
            "2026-01-03T00:00:01Z",
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let request = DecisionLogRequest {
        statuses: vec![DecisionStatus::Superseded],
        ..DecisionLogRequest::default()
    };
    let export = export_decision_log(&graph, &ledger, &request)?;

    assert_eq!(
        export
            .files
            .keys()
            .filter(|path| path.starts_with("decisions/"))
            .count(),
        1,
        "only the superseded decision matches the status filter"
    );
    let old_file = export
        .files
        .get("decisions/2026-01-02-keep-the-cache-layer-old.md")
        .expect("superseded decision file present");
    assert!(old_file.contains("Status: superseded → Replace the cache layer (decision-new)"));
    assert!(!old_file.contains("](decisions/"));
    Ok(())
}

#[test]
fn export_decision_log_survives_a_branched_supersession() -> Result<()> {
    let events = vec![
        decision_event(
            1,
            "decision-root",
            "Original plan",
            "2026-01-02T00:00:00Z",
            &[],
            &[],
        ),
        decision_event(
            2,
            "decision-branch-a",
            "Branch A",
            "2026-01-03T00:00:00Z",
            &[],
            &[],
        ),
        decision_event(
            3,
            "decision-branch-b",
            "Branch B",
            "2026-01-04T00:00:00Z",
            &[],
            &[],
        ),
        event(
            4,
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({"old_decision_id": "decision-root", "new_decision_id": "decision-branch-a"}),
            "2026-01-03T00:00:01Z",
        ),
        event(
            5,
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({"old_decision_id": "decision-root", "new_decision_id": "decision-branch-b"}),
            "2026-01-04T00:00:01Z",
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;

    let root_file = export
        .files
        .get("decisions/2026-01-02-original-plan-root.md")
        .expect("root decision file present despite the branch");
    assert!(root_file.contains("Branch A"));
    assert!(root_file.contains("Branch B"));
    Ok(())
}

#[test]
fn export_decision_log_filters_are_anded() -> Result<()> {
    let events = vec![
        decision_event(
            1,
            "decision-a",
            "Storage change",
            "2026-01-01T00:00:00Z",
            &[],
            &[],
        ),
        decision_event(
            2,
            "decision-b",
            "Storage change take two",
            "2026-02-01T00:00:00Z",
            &[],
            &[],
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let request = DecisionLogRequest {
        since: Some(ts("2026-01-15T00:00:00Z")),
        topics: vec!["storage".to_owned()],
        statuses: vec![],
    };
    let export = export_decision_log(&graph, &ledger, &request)?;

    let decision_files: Vec<&String> = export
        .files
        .keys()
        .filter(|path| path.starts_with("decisions/"))
        .collect();
    assert_eq!(
        decision_files[0].as_str(),
        "decisions/2026-02-01-storage-change-take-two-b.md"
    );
    Ok(())
}

#[test]
fn export_decision_log_sanitizes_colon_namespaced_ids_into_filenames() -> Result<()> {
    // tests/support/organizational_scenarios.rs uses ids like "org:incident:decision:declare" —
    // no "decision-" prefix, and colons are not a safe filename character.
    let events = vec![decision_event(
        1,
        "org:incident:decision:declare",
        "Declare pricing flag incident",
        "2026-01-02T00:00:00Z",
        &[],
        &[],
    )];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?;

    let decision_files: Vec<&String> = export
        .files
        .keys()
        .filter(|path| path.starts_with("decisions/"))
        .collect();
    assert_eq!(decision_files.len(), 1);
    assert!(!decision_files[0].contains(':'));
    assert_eq!(
        decision_files[0].as_str(),
        "decisions/2026-01-02-declare-pricing-flag-incident-orgincid.md"
    );
    Ok(())
}
