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

const DESIGN_SYSTEM_TITLE: &str = "Design system library is shadcn/ui on Tailwind";
const COURTROOM_TITLE: &str =
    "Keep the 12 Angry Men courtroom demo on the website, positioned lower on the page";

#[test]
fn why_did_we_pick_choose_or_go_with_resolves_in_one_step() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:design", DESIGN_SYSTEM_TITLE, &["design"]),
        decision_proposed(2, "d:partners", "Design partners get early access", &[]),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;

    for description in [
        "why did we pick shadcn for the design system",
        "why did we choose shadcn for the design system",
        "why did we go with shadcn for the design system",
        "why did we decide on shadcn for the design system",
        "why did we settle on shadcn for the design system",
        "why did we opt for shadcn for the design system",
        "why we went with shadcn for the design system?",
    ] {
        let response = resolve_decision_by_description(&graph, description, None)?;
        match response.data {
            ResolveOutcome::Resolved { candidate } => {
                assert_eq!(candidate.decision_id, "d:design", "{description:?}");
                assert!(candidate.missing_terms.is_empty(), "{description:?}");
            }
            other => panic!("{description:?} should resolve, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_framing_adverb_is_not_a_missing_term() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:courtroom", COURTROOM_TITLE, &["website"]),
        decision_proposed(2, "d:site", "Publish the changelog on the docs site", &[]),
    ])?;

    for description in [
        "why is the courtroom demo still on the site",
        "why is the courtroom demo on the site again",
        "why do we even keep the courtroom demo on the site",
        "why is the courtroom demo currently on the site",
    ] {
        assert_eq!(
            resolved_id(&graph, description)?.as_deref(),
            Some("d:courtroom"),
            "{description:?} should resolve to the courtroom decision"
        );
    }
    Ok(())
}

#[test]
fn a_decision_that_uses_a_framing_verb_is_still_found_by_it() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:vendor", "Pick the cheapest vendor", &["vendors"]),
        decision_proposed(2, "d:postgres", "Go with Postgres for the ledger", &[]),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &[]),
    ])?;

    // The verb is dropped from the question, not from the decision's text.
    assert_eq!(
        resolved_id(&graph, "pick vendor")?.as_deref(),
        Some("d:vendor")
    );
    assert_eq!(
        resolved_id(&graph, "why did we pick the cheapest vendor")?.as_deref(),
        Some("d:vendor")
    );
    assert_eq!(
        resolved_id(&graph, "go with postgres")?.as_deref(),
        Some("d:postgres")
    );
    // A question that is only the verb still searches for it.
    assert_eq!(resolved_id(&graph, "pick")?.as_deref(), Some("d:vendor"));
    Ok(())
}

#[test]
fn go_the_language_is_not_dropped_as_a_verb() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:go", "Write the CLI in Go", &[]),
        decision_proposed(2, "d:rust", "Write the server in Rust", &[]),
    ])?;

    assert_eq!(
        resolved_id(&graph, "why did we pick go for the cli")?.as_deref(),
        Some("d:go")
    );
    assert_eq!(
        resolved_id(&graph, "why did we go with go for the cli")?.as_deref(),
        Some("d:go")
    );
    Ok(())
}

#[test]
fn a_word_the_record_lacks_is_still_missing_after_the_verb_is_dropped() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:design",
        DESIGN_SYSTEM_TITLE,
        &["design"],
    )])?;

    // "pick" no longer counts against the decision, but "finally" still does: dropping verbs
    // must not make close candidates look like full matches.
    let response = resolve_decision_by_description(
        &graph,
        "why did we finally pick shadcn for the design system",
        None,
    )?;
    match response.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].missing_terms, vec!["finally".to_owned()]);
        }
        other => panic!("expected a close candidate, got {other:?}"),
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

    // "not" is a term, so the decision that adopted Kafka is only a close candidate that says
    // it lacks "not" -- never a resolution.
    let response = resolve_decision_by_description(&graph, "do not adopt kafka", None)?;
    match response.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].missing_terms, vec!["not".to_owned()]);
        }
        other => panic!("expected close candidate, got {other:?}"),
    }
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

fn projects_graph() -> Result<MemoryGraph> {
    graph_from_events([
        decision_proposed(
            1,
            "d:personal",
            "An agent's personal project is per agent kind and tool, never per session",
            &["projects"],
        ),
        decision_proposed(
            2,
            "d:handles",
            "Project handles are lowercase slugs of at most forty characters",
            &["projects"],
        ),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])
}

#[test]
fn repeated_words_in_a_description_still_resolve() -> Result<()> {
    let graph = projects_graph()?;

    // "agent" appears twice; the duplicate must not count as an extra term to find.
    let resolved = resolved_id(&graph, "agent personal project per agent kind")?;

    assert_eq!(resolved.as_deref(), Some("d:personal"));
    Ok(())
}

#[test]
fn a_word_the_record_lacks_yields_close_candidates_not_not_found() -> Result<()> {
    let graph = projects_graph()?;

    let response = resolve_decision_by_description(
        &graph,
        "why did we finally make a personal project per agent kind",
        None,
    )?;

    match response.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates[0].decision_id, "d:personal");
            assert_eq!(candidates[0].missing_terms, vec!["finally", "make"]);
            // Every candidate says what it lacks, and none is silently picked.
            assert!(candidates.iter().all(|c| !c.missing_terms.is_empty()));
        }
        other => panic!("expected close candidates, got {other:?}"),
    }
    assert_eq!(response.result_count, 1);
    Ok(())
}

#[test]
fn a_full_match_beats_close_candidates() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:full", "Adopt async queue for billing", &[]),
        decision_proposed(2, "d:near", "Adopt async retries for billing", &[]),
    ])?;

    // d:near matches "adopt async billing" but not "queue"; d:full matches every term.
    let response = resolve_decision_by_description(&graph, "adopt async queue billing", None)?;

    match response.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:full");
            assert!(candidate.missing_terms.is_empty());
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
    Ok(())
}

#[test]
fn resolve_by_id_finds_an_existing_decision_and_reports_its_title() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:goal", "Keep the ledger append-only", &["ledger"]),
        decision_proposed(2, "d:other", "Use Postgres directly", &["storage"]),
    ])?;

    let response = resolve_decision_by_id(&graph, "  d:goal  ")?;
    match response.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:goal");
            assert_eq!(candidate.title, "Keep the ledger append-only");
            assert_eq!(candidate.rank, 0);
            assert_eq!(candidate.matched_fields, ["id"]);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
    assert_eq!(response.result_count, 1);
    assert!(!response.truncated);
    Ok(())
}

#[test]
fn one_matching_word_is_not_close_enough() -> Result<()> {
    let graph = projects_graph()?;

    // Only "billing" matches anything: 1 of 2 terms is not a close match.
    let response = resolve_decision_by_description(&graph, "billing unicorn", None)?;

    assert_eq!(response.data, ResolveOutcome::NotFound);
    Ok(())
}

#[test]
fn resolve_by_id_reports_a_missing_decision_as_not_found() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:goal",
        "Keep the ledger append-only",
        &["ledger"],
    )])?;

    let response = resolve_decision_by_id(&graph, "d:missing")?;
    assert_eq!(response.data, ResolveOutcome::NotFound);
    assert_eq!(response.result_count, 0);
    Ok(())
}

#[test]
fn resolve_by_id_rejects_a_blank_id() -> Result<()> {
    let graph = graph_from_events([])?;
    assert!(resolve_decision_by_id(&graph, "   ").is_err());
    Ok(())
}
