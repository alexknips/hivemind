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

/// Every fixture decision below is proposed by `human:alice` with no project stated, so it
/// lands in her derived personal project.
const ALICE_DECISIONS: &str = "projects/personal/human-alice/decisions/";

fn exported(
    graph: &impl GraphView,
    ledger: &impl EventLedger,
    req: &DecisionLogRequest,
) -> Result<DecisionLogExport> {
    match export_decision_log(graph, ledger, req)? {
        DecisionLogOutcome::Exported(export) => Ok(export),
        DecisionLogOutcome::ProjectNotFound { project } => {
            panic!("unexpected ProjectNotFound for {project}")
        }
    }
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

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    assert_eq!(export.ledger_offset, ledger.latest_offset()?);
    assert_eq!(
        export.files.len(),
        3,
        "root INDEX.md, the personal project's INDEX.md, one decision file"
    );
    assert!(export.files.contains_key("INDEX.md"));
    assert!(export
        .files
        .contains_key("projects/personal/human-alice/INDEX.md"));

    let (path, content) = export
        .files
        .iter()
        .find(|(path, _)| path.contains("/decisions/"))
        .expect("one decision file");
    assert_eq!(
        path,
        "projects/personal/human-alice/decisions/2026-01-02-adopt-postgres-1.md"
    );
    assert!(content.starts_with("---\n"));
    assert!(content.contains("id: \"decision-1\""));
    assert!(content.contains("status: \"accepted\""));
    assert!(content.contains("# Adopt Postgres"));
    assert!(content.contains("Postgres scales better than SQLite here"));
    assert!(content.contains("supported"));
    assert!(content.ends_with('\n'));

    let index = export.files.get("INDEX.md").expect("index present");
    assert!(index.contains("Do not edit; regenerate with `hivemind export`."));
    assert!(index.contains("## Personal project: human:alice"));
    assert!(index.contains(
        "[Adopt Postgres](projects/personal/human-alice/decisions/2026-01-02-adopt-postgres-1.md)"
    ));
    let project_index = export
        .files
        .get("projects/personal/human-alice/INDEX.md")
        .expect("project index present");
    assert!(project_index.contains("[Adopt Postgres](decisions/2026-01-02-adopt-postgres-1.md)"));
    assert!(project_index.contains("[All projects](../../../INDEX.md)"));
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

    let first = exported(&graph, &ledger, &DecisionLogRequest::default())?;
    let second = exported(&graph, &ledger, &DecisionLogRequest::default())?;

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

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    let decision_files: Vec<&String> = export
        .files
        .keys()
        .filter(|path| path.starts_with(ALICE_DECISIONS))
        .collect();
    assert_eq!(decision_files.len(), 2);
    assert_ne!(decision_files[0], decision_files[1]);
    for path in decision_files {
        assert!(
            path.starts_with("projects/personal/human-alice/decisions/2026-01-02-adopt-postgres-")
        );
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

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;
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
    let export = exported(&graph, &ledger, &request)?;

    assert_eq!(
        export
            .files
            .keys()
            .filter(|path| path.starts_with(ALICE_DECISIONS))
            .count(),
        1,
        "only the superseded decision matches the status filter"
    );
    let old_file = export
        .files
        .get("projects/personal/human-alice/decisions/2026-01-02-keep-the-cache-layer-old.md")
        .expect("superseded decision file present");
    assert!(old_file.contains("Status: superseded → Replace the cache layer (decision-new)"));
    assert!(!old_file.contains("]("));
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

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    let root_file = export
        .files
        .get("projects/personal/human-alice/decisions/2026-01-02-original-plan-root.md")
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
        ..DecisionLogRequest::default()
    };
    let export = exported(&graph, &ledger, &request)?;

    let decision_files: Vec<&String> = export
        .files
        .keys()
        .filter(|path| path.starts_with(ALICE_DECISIONS))
        .collect();
    assert_eq!(
        decision_files[0].as_str(),
        "projects/personal/human-alice/decisions/2026-02-01-storage-change-take-two-b.md"
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

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    let decision_files: Vec<&String> = export
        .files
        .keys()
        .filter(|path| path.starts_with(ALICE_DECISIONS))
        .collect();
    assert_eq!(decision_files.len(), 1);
    assert!(!decision_files[0].contains(':'));
    assert_eq!(
        decision_files[0].as_str(),
        "projects/personal/human-alice/decisions/2026-01-02-declare-pricing-flag-incident-orgincid.md"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-project layout (hivemind-s15q.8)
// ---------------------------------------------------------------------------

fn register_project_event(sequence: u128, handle: &str, display_name: Option<&str>) -> Event {
    event(
        sequence,
        EventType::ProjectRegistered,
        "human:alice",
        json!({"handle": handle, "display_name": display_name}),
        "2026-01-01T00:00:00Z",
    )
}

fn project_decision_event(
    sequence: u128,
    decision_id: &str,
    title: &str,
    timestamp: &str,
    actor: &str,
    project: Option<&str>,
) -> Event {
    let mut payload = json!({
        "decision_id": decision_id,
        "title": title,
        "rationale": format!("Rationale for {title}"),
        "topic_keys": ["storage"],
        "option_ids": [format!("{decision_id}-a"), format!("{decision_id}-b")],
        "chosen_option_id": format!("{decision_id}-b"),
    });
    if let Some(project) = project {
        payload["project"] = json!(project);
        payload["project_source"] = json!("stated");
    }
    event(
        sequence,
        EventType::DecisionProposed,
        actor,
        payload,
        timestamp,
    )
}

/// A small city: two shared projects with decisions, one registered but empty, two agent
/// sessions that share one personal project, and a human's personal decision.
fn city_events() -> Vec<Event> {
    vec![
        register_project_event(1, "platform", Some("Platform")),
        register_project_event(2, "billing", None),
        register_project_event(3, "beadline", None),
        project_decision_event(
            4,
            "decision-plat",
            "Standardise on Postgres",
            "2026-01-02T00:00:00Z",
            "human:alice",
            Some("platform"),
        ),
        project_decision_event(
            5,
            "decision-bill",
            "Charge per seat",
            "2026-01-03T00:00:00Z",
            "human:bob",
            Some("billing"),
        ),
        project_decision_event(
            6,
            "decision-mine",
            "Try a scratch idea",
            "2026-01-04T00:00:00Z",
            "human:alice",
            None,
        ),
        project_decision_event(
            7,
            "decision-agent",
            "Agent note",
            "2026-01-05T00:00:00Z",
            "agent:claude:session-1",
            None,
        ),
        project_decision_event(
            8,
            "decision-agent-2",
            "Second agent note",
            "2026-01-06T00:00:00Z",
            "agent:claude:session-2",
            None,
        ),
    ]
}

#[test]
fn export_groups_decisions_per_project() -> Result<()> {
    let (ledger, graph) = graph_and_ledger(city_events())?;

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    let paths: Vec<&str> = export.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        vec![
            "INDEX.md",
            "projects/beadline/INDEX.md",
            "projects/billing/INDEX.md",
            "projects/billing/decisions/2026-01-03-charge-per-seat-bill.md",
            "projects/personal/agent-claude/INDEX.md",
            "projects/personal/agent-claude/decisions/2026-01-05-agent-note-agent.md",
            "projects/personal/agent-claude/decisions/2026-01-06-second-agent-note-agent2.md",
            "projects/personal/human-alice/INDEX.md",
            "projects/personal/human-alice/decisions/2026-01-04-try-a-scratch-idea-mine.md",
            "projects/platform/INDEX.md",
            "projects/platform/decisions/2026-01-02-standardise-on-postgres-plat.md",
        ],
        "every decision is exported once, under its own project; a registered project with no \
         decisions still gets its record; both agent sessions share one personal project"
    );

    let root = &export.files["INDEX.md"];
    assert!(root.contains("Counts: proposed=5\n"));
    let sections: Vec<usize> = [
        "## beadline\n",
        "## billing\n",
        "## Platform (platform)\n",
        "## Personal project: agent:claude\n",
        "## Personal project: human:alice\n",
    ]
    .iter()
    .map(|heading| {
        root.find(heading)
            .unwrap_or_else(|| panic!("root index has section {heading:?}"))
    })
    .collect();
    assert!(
        sections.windows(2).all(|pair| pair[0] < pair[1]),
        "shared projects by handle, then personal projects by address"
    );
    assert!(root.contains("Project record: [projects/billing/INDEX.md](projects/billing/INDEX.md)"));
    assert!(root.contains(
        "[Charge per seat](projects/billing/decisions/2026-01-03-charge-per-seat-bill.md)"
    ));

    let billing = &export.files["projects/billing/INDEX.md"];
    assert!(billing.starts_with("# billing\n"));
    assert!(billing.contains("Project: `billing`"));
    assert!(billing.contains("[Charge per seat](decisions/2026-01-03-charge-per-seat-bill.md)"));
    assert!(
        !billing.contains("Standardise on Postgres"),
        "a project's record holds only its own decisions"
    );

    let beadline = &export.files["projects/beadline/INDEX.md"];
    assert!(beadline.contains("Counts: none."));
    assert!(!beadline.contains("| Date |"));
    Ok(())
}

#[test]
fn export_links_a_supersession_across_projects_relative_to_each_file() -> Result<()> {
    let events = vec![
        register_project_event(1, "platform", Some("Platform")),
        register_project_event(2, "billing", None),
        project_decision_event(
            3,
            "decision-old",
            "Old plan",
            "2026-01-02T00:00:00Z",
            "human:alice",
            Some("billing"),
        ),
        project_decision_event(
            4,
            "decision-new",
            "New plan",
            "2026-01-03T00:00:00Z",
            "human:alice",
            Some("platform"),
        ),
        event(
            5,
            EventType::DecisionSuperseded,
            "human:alice",
            json!({"old_decision_id": "decision-old", "new_decision_id": "decision-new"}),
            "2026-01-03T00:00:01Z",
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    let old_file = &export.files["projects/billing/decisions/2026-01-02-old-plan-old.md"];
    let to_new = "[New plan](../../platform/decisions/2026-01-03-new-plan-new.md)";
    assert!(old_file.contains(&format!("Status: superseded → {to_new}")));
    assert!(old_file.contains(&format!("Superseded by: {to_new}")));

    let new_file = &export.files["projects/platform/decisions/2026-01-03-new-plan-new.md"];
    assert!(new_file
        .contains("Supersedes: [Old plan](../../billing/decisions/2026-01-02-old-plan-old.md)"));

    let billing = &export.files["projects/billing/INDEX.md"];
    assert!(billing
        .contains("superseded → [New plan](../platform/decisions/2026-01-03-new-plan-new.md)"));
    let root = &export.files["INDEX.md"];
    assert!(root.contains(
        "superseded → [New plan](projects/platform/decisions/2026-01-03-new-plan-new.md)"
    ));
    Ok(())
}

#[test]
fn export_project_filter_keeps_only_that_projects_files() -> Result<()> {
    let events = vec![
        register_project_event(1, "platform", Some("Platform")),
        register_project_event(2, "billing", None),
        project_decision_event(
            3,
            "decision-old",
            "Old plan",
            "2026-01-02T00:00:00Z",
            "human:alice",
            Some("billing"),
        ),
        project_decision_event(
            4,
            "decision-new",
            "New plan",
            "2026-01-03T00:00:00Z",
            "human:alice",
            Some("platform"),
        ),
        event(
            5,
            EventType::DecisionSuperseded,
            "human:alice",
            json!({"old_decision_id": "decision-old", "new_decision_id": "decision-new"}),
            "2026-01-03T00:00:01Z",
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let request = DecisionLogRequest {
        project: Some("billing".to_owned()),
        ..DecisionLogRequest::default()
    };
    let export = exported(&graph, &ledger, &request)?;

    let paths: Vec<&str> = export.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        vec![
            "INDEX.md",
            "projects/billing/INDEX.md",
            "projects/billing/decisions/2026-01-02-old-plan-old.md",
        ]
    );
    let root = &export.files["INDEX.md"];
    assert!(root.contains("Filters: project=billing  topic=any"));
    assert!(root.contains("## billing\n"));
    assert!(!root.contains("platform"));

    let old_file = &export.files["projects/billing/decisions/2026-01-02-old-plan-old.md"];
    assert!(
        old_file.contains("Status: superseded → New plan (decision-new)"),
        "a successor in another project is named as text, never a dead link"
    );
    assert!(!old_file.contains("]("));
    Ok(())
}

#[test]
fn export_unknown_project_is_not_found_and_a_known_empty_one_is_not() -> Result<()> {
    let (ledger, graph) = graph_and_ledger(city_events())?;

    let typo = DecisionLogRequest {
        project: Some("billng".to_owned()),
        ..DecisionLogRequest::default()
    };
    assert_eq!(
        export_decision_log(&graph, &ledger, &typo)?,
        DecisionLogOutcome::ProjectNotFound {
            project: "billng".to_owned()
        }
    );

    let registered_but_empty = DecisionLogRequest {
        project: Some(" beadline ".to_owned()),
        ..DecisionLogRequest::default()
    };
    let export = exported(&graph, &ledger, &registered_but_empty)?;
    assert!(export.files["projects/beadline/INDEX.md"].contains("Counts: none."));
    assert_eq!(export.files.len(), 2);

    let personal_without_decisions = DecisionLogRequest {
        project: Some("personal:human:nobody".to_owned()),
        ..DecisionLogRequest::default()
    };
    let export = exported(&graph, &ledger, &personal_without_decisions)?;
    assert!(export
        .files
        .contains_key("projects/personal/human-nobody/INDEX.md"));

    let blank = DecisionLogRequest {
        project: Some("  ".to_owned()),
        ..DecisionLogRequest::default()
    };
    assert!(export_decision_log(&graph, &ledger, &blank).is_err());
    Ok(())
}

#[test]
fn export_personal_project_directories_never_collide_or_escape() -> Result<()> {
    let events = vec![
        project_decision_event(
            1,
            "decision-one",
            "One",
            "2026-01-02T00:00:00Z",
            "human:a.b",
            None,
        ),
        project_decision_event(
            2,
            "decision-two",
            "Two",
            "2026-01-02T00:00:01Z",
            "human:a-b",
            None,
        ),
        project_decision_event(
            3,
            "decision-three",
            "Three",
            "2026-01-02T00:00:02Z",
            "human:x/../y",
            None,
        ),
        project_decision_event(
            4,
            "decision-four",
            "Four",
            "2026-01-02T00:00:03Z",
            "decisions",
            None,
        ),
    ];
    let (ledger, graph) = graph_and_ledger(events)?;

    let export = exported(&graph, &ledger, &DecisionLogRequest::default())?;

    let dotted = format!(
        "projects/personal/human-a-b-{}/INDEX.md",
        short_hash("personal:human:a.b")
    );
    let dashed = format!(
        "projects/personal/human-a-b-{}/INDEX.md",
        short_hash("personal:human:a-b")
    );
    let reserved = format!(
        "projects/personal/decisions-{}/INDEX.md",
        short_hash("personal:decisions")
    );
    for index in [
        dotted.as_str(),
        dashed.as_str(),
        reserved.as_str(),
        "projects/personal/human-x----y/INDEX.md",
    ] {
        assert!(export.files.contains_key(index), "missing {index}");
    }
    assert_ne!(dotted, dashed);
    for path in export.files.keys() {
        assert!(
            !path
                .split('/')
                .any(|part| part.is_empty() || part.starts_with('.')),
            "{path} must stay a plain relative path"
        );
    }
    Ok(())
}

#[test]
fn relative_link_walks_up_only_as_far_as_needed() {
    assert_eq!(
        relative_link("", "projects/a/decisions/x.md"),
        "projects/a/decisions/x.md"
    );
    assert_eq!(
        relative_link("projects/a", "projects/a/decisions/x.md"),
        "decisions/x.md"
    );
    assert_eq!(
        relative_link("projects/a/decisions", "projects/a/decisions/y.md"),
        "y.md"
    );
    assert_eq!(
        relative_link("projects/a/decisions", "projects/b/decisions/y.md"),
        "../../b/decisions/y.md"
    );
    assert_eq!(
        relative_link("projects/personal/x", "INDEX.md"),
        "../../../INDEX.md"
    );
}
