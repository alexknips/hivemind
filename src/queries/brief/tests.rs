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
        correlation_id: Some("brief-test".to_owned()),
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

#[test]
fn brief_composes_context_and_outcome_for_a_clean_decision() -> Result<()> {
    let graph = graph_from_events([
        event(
            1,
            EventType::DecisionProposed,
            "human:alice",
            json!({
                "decision_id": "d:1",
                "title": "Adopt async queue",
                "rationale": "Durability beats latency here",
                "topic_keys": ["infra"],
                "option_ids": ["opt:async", "opt:sync"],
                "chosen_option_id": "opt:async",
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        event(
            2,
            EventType::DecisionAccepted,
            "human:bob",
            json!({ "decision_id": "d:1" }),
        ),
    ])?;

    let response = get_decision_brief(&graph, "d:1")?;
    let brief = response.data.expect("decision exists");

    assert_eq!(brief.decision_id, "d:1");
    assert_eq!(brief.title, "Adopt async queue");
    assert_eq!(brief.rationale, "Durability beats latency here");
    assert_eq!(brief.topic_keys, vec!["infra".to_owned()]);
    assert_eq!(brief.status, DecisionStatus::Accepted);

    // No option.recorded-style event exists for either option, so the label falls back to the
    // option_id — the stated schema gap (docs/AGENT_FLUENT_QUERYING.md §4), not a bug.
    let chosen = brief.chosen_option.expect("chosen option present");
    assert_eq!(chosen.option_id, "opt:async");
    assert_eq!(chosen.label, "opt:async");
    assert_eq!(brief.rejected_options.len(), 1);
    assert_eq!(brief.rejected_options[0].option_id, "opt:sync");

    assert_eq!(brief.decided_by.proposer_id.as_deref(), Some("human:alice"));
    assert_eq!(brief.decided_by.source, "cli");
    assert_eq!(brief.decided_by.review, ReviewShape::PeerReviewed);

    // Options exist but no evidence was attached: thin_structure fires without flipping held_up.
    assert!(brief.still_holds.held_up);
    assert!(brief.still_holds.reasons.iter().any(|reason| matches!(
        reason,
        OutcomeReason::ThinStructure {
            no_evidence: true,
            ..
        }
    )));

    Ok(())
}

#[test]
fn brief_reports_superseded_decision_as_not_held_up() -> Result<()> {
    let graph = graph_from_events([
        event(
            1,
            EventType::DecisionProposed,
            "agent:tester",
            json!({
                "decision_id": "d:old",
                "title": "Old approach",
                "rationale": "seemed fine at the time",
                "topic_keys": [],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        event(
            2,
            EventType::DecisionProposed,
            "agent:tester",
            json!({
                "decision_id": "d:new",
                "title": "New approach",
                "rationale": "learned more since",
                "topic_keys": [],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        event(
            3,
            EventType::DecisionSuperseded,
            "agent:tester",
            json!({ "old_decision_id": "d:old", "new_decision_id": "d:new" }),
        ),
    ])?;

    let response = get_decision_brief(&graph, "d:old")?;
    let brief = response.data.expect("decision exists");

    assert!(!brief.still_holds.held_up);
    assert!(brief.still_holds.reasons.iter().any(
        |reason| matches!(reason, OutcomeReason::SupersededBy { by_id, .. } if by_id == "d:new")
    ));

    Ok(())
}

#[test]
fn brief_returns_none_for_missing_decision() -> Result<()> {
    let graph = graph_from_events([])?;
    let response = get_decision_brief(&graph, "does-not-exist")?;
    assert!(response.data.is_none());
    assert_eq!(response.result_count, 0);
    Ok(())
}
