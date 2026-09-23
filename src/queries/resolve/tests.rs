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

fn decision_proposed(sequence: u128, decision_id: &str, title: &str, topic_keys: &[&str]) -> Event {
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::from_u128(sequence),
        correlation_id: Some("resolve-test".to_owned()),
        causation_event_id: None,
        event_type: EventType::DecisionProposed,
        actor_id: "agent:tester".to_owned(),
        source: EventSource::Cli,
        source_ref: None,
        payload: json!({
            "decision_id": decision_id,
            "title": title,
            "rationale": "because reasons",
            "topic_keys": topic_keys,
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
        ts: Some(ts("2026-01-01T00:00:00Z")),
    }
}

fn ts(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses")
        .with_timezone(&Utc)
}

#[test]
fn resolves_unique_title_match() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(
            1,
            "d:billing",
            "Adopt async queue for billing",
            &["billing"],
        ),
        decision_proposed(2, "d:db", "Use Postgres directly", &["storage"]),
    ])?;

    let response = resolve_decision_by_description(&graph, "async queue", None)?;
    match response.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:billing");
            assert_eq!(candidate.rank, 1);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
    assert_eq!(response.result_count, 1);
    Ok(())
}

#[test]
fn ambiguous_when_two_candidates_share_the_best_tier() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(
            1,
            "d:billing",
            "Adopt async queue for billing",
            &["billing"],
        ),
        decision_proposed(
            2,
            "d:notify",
            "Adopt async queue for notifications",
            &["notifications"],
        ),
    ])?;

    let response = resolve_decision_by_description(&graph, "adopt async queue", None)?;
    match response.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 2);
            // Newest (highest event_origin) sorts first as the recency tiebreak.
            assert_eq!(candidates[0].decision_id, "d:notify");
            assert_eq!(candidates[1].decision_id, "d:billing");
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    assert_eq!(response.result_count, 2);
    Ok(())
}

#[test]
fn topic_hint_narrows_ambiguous_down_to_resolved() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(
            1,
            "d:billing",
            "Adopt async queue for billing",
            &["billing"],
        ),
        decision_proposed(
            2,
            "d:notify",
            "Adopt async queue for notifications",
            &["notifications"],
        ),
    ])?;

    let response = resolve_decision_by_description(&graph, "adopt async queue", Some("billing"))?;
    match response.data {
        ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:billing"),
        other => panic!("expected Resolved, got {other:?}"),
    }
    Ok(())
}

#[test]
fn exact_title_match_wins_over_weaker_matches_without_ambiguity() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:exact", "Adopt async queue", &["infra"]),
        decision_proposed(2, "d:partial-a", "Adopt async retries", &["infra"]),
        decision_proposed(3, "d:partial-b", "Async queue follow-up", &["infra"]),
    ])?;

    let response = resolve_decision_by_description(&graph, "Adopt async queue", None)?;
    match response.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:exact");
            assert_eq!(candidate.rank, 0);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
    Ok(())
}

#[test]
fn not_found_when_no_decision_matches_all_terms() -> Result<()> {
    let graph = graph_from_events([decision_proposed(1, "d:billing", "Adopt async queue", &[])])?;

    let response = resolve_decision_by_description(&graph, "nonexistent widget", None)?;
    assert_eq!(response.data, ResolveOutcome::NotFound);
    assert_eq!(response.result_count, 0);
    Ok(())
}

#[test]
fn empty_description_is_rejected() {
    let graph = graph_from_events([]).expect("empty graph");
    let error = resolve_decision_by_description(&graph, "   ", None).expect_err("empty rejected");
    assert!(format!("{error}").contains("description"));
}

const STORAGE_TITLE: &str =
    "Demo cell storage moves to shared Postgres backend instead of per-host SQLite";

fn resolved_id(graph: &MemoryGraph, description: &str) -> Result<Option<String>> {
    Ok(
        match resolve_decision_by_description(graph, description, None)?.data {
            ResolveOutcome::Resolved { candidate } => Some(candidate.decision_id),
            _ => None,
        },
    )
}

#[test]
fn natural_questions_resolve_to_the_decision() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:storage", STORAGE_TITLE, &["storage"]),
        decision_proposed(2, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;

    for description in [
        "Postgres",
        "demo cell storage",
        "move the demo cell to shared Postgres",
        "why did we move the demo cell to shared Postgres",
        "Why did we move the demo cell to shared Postgres?",
        "why was demo cell storage moved to shared postgres",
    ] {
        assert_eq!(
            resolved_id(&graph, description)?.as_deref(),
            Some("d:storage"),
            "{description:?} should resolve to the storage decision"
        );
    }
    Ok(())
}

#[test]
fn inflected_words_match_their_stem() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:queue",
        "Adopt async queues for billing",
        &[],
    )])?;

    let resolved = resolved_id(&graph, "adopting an async queue for billing")?;

    assert_eq!(resolved.as_deref(), Some("d:queue"));
    Ok(())
}

#[test]
fn stemming_compares_words_not_substrings() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:strategy",
        "Use strategy pattern for retries",
        &[],
    )])?;

    // "string" stems to "str", a prefix of "strategy": it must not match by prefix.
    assert_eq!(resolved_id(&graph, "string")?, None);
    // A literal substring still matches, exactly as before stemming existed.
    assert_eq!(resolved_id(&graph, "strat")?.as_deref(), Some("d:strategy"));
    Ok(())
}

#[test]
fn negation_is_kept_so_it_cannot_resolve_to_the_opposite_decision() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:kafka",
        "Adopt Kafka for events",
        &[],
    )])?;

    assert_eq!(resolved_id(&graph, "do not adopt kafka")?, None);
    assert_eq!(
        resolved_id(&graph, "why did we adopt kafka")?.as_deref(),
        Some("d:kafka")
    );
    Ok(())
}

#[test]
fn a_question_of_only_stopwords_matches_nothing() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:billing", "Adopt async queue for billing", &[]),
        decision_proposed(2, "d:notify", "Adopt async queue for notifications", &[]),
    ])?;

    let response = resolve_decision_by_description(&graph, "why did we", None)?;

    assert_eq!(response.data, ResolveOutcome::NotFound);
    Ok(())
}

#[test]
fn ambiguity_gate_still_applies_to_natural_questions() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:billing", "Move billing to Postgres", &[]),
        decision_proposed(2, "d:notify", "Move notifications to Postgres", &[]),
    ])?;

    let response = resolve_decision_by_description(&graph, "why did we move to postgres", None)?;

    match response.data {
        ResolveOutcome::Ambiguous { candidates } => assert_eq!(candidates.len(), 2),
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    Ok(())
}
