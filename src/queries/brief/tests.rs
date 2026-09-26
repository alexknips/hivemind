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
    // alice recorded it, but bob is who accepted it -- the two must stay distinguishable
    // rather than collapsing to "decided by: alice" (hivemind-zdsh.9).
    assert_eq!(brief.decided_by.decider_ids, vec!["human:bob".to_owned()]);
    assert_eq!(brief.decided_by.source, "cli");
    assert_eq!(brief.decided_by.review, ReviewShape::PeerReviewed);

    // Options exist but no evidence was attached, and nothing says it rests on anything. That
    // is a question about quality (the profile's), not a reason it stopped holding.
    assert!(brief.still_holds.held_up);
    assert!(brief.still_holds.reasons.is_empty());
    assert_eq!(brief.grounding_state, GroundingState::NothingDeclared);

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

// ── what the decision rests on (hivemind-gwhr.3) ─────────────────────────────────────────

use crate::queries::grounding::{
    GroundingAdded, GroundingItemState, GroundingKind, GroundingState,
};
use crate::queries::test_fixtures::Scenario;

#[test]
fn brief_lists_what_the_decision_rests_on_and_how_many_decisions_rest_on_it() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.accept("d:goal", "human:alex", "2026-01-01T00:00:01Z")?;
    scenario.evidence(
        "e:audit",
        "27 of 27 captures carry no premise",
        Some("mayor audit 2026-09-22"),
        "2026-01-01T00:00:02Z",
    )?;
    scenario.hypothesis(
        "h:bet",
        "Agents answer the question instead of skipping capture",
        "bet",
        Some("2026-10-15T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    let proposal = scenario.decision_with(
        "d:derived",
        "Premises are asked at capture",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
        false,
        &["e:audit"],
        &["h:bet"],
        Some("high"),
    )?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:derived",
        "d:goal",
        "agent:claude:crew",
        Some(proposal),
        "2026-01-02T00:00:01Z",
    )?;
    let graph = scenario.graph()?;
    let now = ts("2026-06-01T00:00:00Z");

    let brief = get_decision_brief_at(&graph, "d:derived", now)?
        .data
        .expect("decision exists");

    assert_eq!(brief.grounding_state, GroundingState::Grounded);
    assert_eq!(brief.expressed_confidence.as_deref(), Some("high"));
    assert_eq!(brief.dependents_count, 0);
    let rests_on: Vec<(GroundingKind, &str, &GroundingAdded)> = brief
        .rests_on
        .iter()
        .map(|item| (item.kind, item.label.as_str(), &item.added))
        .collect();
    assert_eq!(
        rests_on,
        vec![
            (
                GroundingKind::Decision,
                "Ship the hosted MVP",
                &GroundingAdded::AtCapture
            ),
            (
                GroundingKind::Evidence,
                "27 of 27 captures carry no premise",
                &GroundingAdded::AtCapture
            ),
            (
                GroundingKind::Bet,
                "Agents answer the question instead of skipping capture",
                &GroundingAdded::AtCapture
            ),
        ]
    );
    assert!(brief.still_holds.held_up);

    // The premise knows one decision rests on it, and carries no premise of its own.
    let goal = get_decision_brief_at(&graph, "d:goal", now)?
        .data
        .expect("decision exists");
    assert_eq!(goal.dependents_count, 1);
    assert_eq!(goal.grounding_state, GroundingState::NothingDeclared);
    assert!(goal.rests_on.is_empty());
    assert_eq!(goal.expressed_confidence, None);
    Ok(())
}

#[test]
fn brief_names_an_overdue_bet_as_unchecked_without_saying_the_decision_is_stale() -> Result<()> {
    let scenario = Scenario::new();
    scenario.hypothesis(
        "h:late",
        "Nobody will notice",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision_with(
        "d:1",
        "Ship it anyway",
        "human:alice",
        "2026-01-02T00:00:00Z",
        false,
        &[],
        &["h:late"],
        None,
    )?;
    let graph = scenario.graph()?;

    let brief = get_decision_brief_at(&graph, "d:1", ts("2026-06-01T00:00:00Z"))?
        .data
        .expect("decision exists");

    assert!(brief.still_holds.held_up);
    assert_eq!(brief.grounding_state, GroundingState::Bet);
    assert_eq!(brief.still_holds.unchecked.len(), 1);
    assert_eq!(brief.still_holds.unchecked[0].hypothesis_id, "h:late");
    assert_eq!(
        brief.rests_on[0].state,
        GroundingItemState::BetOpen {
            check_by: Some(ts("2026-02-01T00:00:00Z")),
            overdue: true,
        }
    );
    Ok(())
}
