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
    decision_proposed_because(sequence, decision_id, title, "because reasons", topic_keys)
}

fn decision_proposed_because(
    sequence: u128,
    decision_id: &str,
    title: &str,
    rationale: &str,
    topic_keys: &[&str],
) -> Event {
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
            "rationale": rationale,
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

/// Every spelling of a negated question about adopting Kafka: bare, contracted, a typographic
/// apostrophe, asked as a why-question and as a bare imperative.
const NEGATED_KAFKA_QUESTIONS: &[&str] = &[
    "do not adopt kafka",
    "don't adopt kafka",
    "don\u{2019}t adopt kafka",
    "didn't we adopt kafka",
    "why didn't we adopt kafka",
    "why won't we adopt kafka",
    "why aren't we adopting kafka",
    "never adopt kafka",
];

/// What both kinds of asker, the one that writes and the one that only shows, make of a
/// description.
fn both_askers(graph: &MemoryGraph, description: &str) -> Result<[ResolveOutcome; 2]> {
    Ok([
        resolve_decision_by_description(graph, description, None)?.data,
        resolve_decision_for_reading(graph, description, None)?.data,
    ])
}

#[test]
fn a_negated_question_never_resolves_to_the_opposite_decision() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:kafka",
        "Adopt Kafka for events",
        &[],
    )])?;

    // Every spelling of the negation (hivemind-g889) gets the same answer: the decision that
    // adopted Kafka is a close candidate that says why -- the question is negated, it is not --
    // and is never a resolution, for a verb that writes or one that reads. The negation is not a
    // word it lacks.
    for question in NEGATED_KAFKA_QUESTIONS {
        for outcome in both_askers(&graph, question)? {
            match outcome {
                ResolveOutcome::Ambiguous { candidates } => {
                    assert_eq!(candidates.len(), 1, "{question:?}");
                    assert_eq!(candidates[0].decision_id, "d:kafka", "{question:?}");
                    assert!(candidates[0].polarity_mismatch, "{question:?}");
                    assert!(candidates[0].is_close(), "{question:?}");
                    assert!(
                        candidates[0].missing_terms.is_empty(),
                        "{question:?} lacks no word: {:?}",
                        candidates[0].missing_terms
                    );
                }
                other => panic!("{question:?} must stay a close candidate, got {other:?}"),
            }
        }
    }

    // Without the negation it still resolves, and says nothing about polarity.
    for question in ["why did we adopt kafka", "adopt kafka"] {
        for outcome in both_askers(&graph, question)? {
            match outcome {
                ResolveOutcome::Resolved { candidate } => {
                    assert_eq!(candidate.decision_id, "d:kafka", "{question:?}");
                    assert!(!candidate.polarity_mismatch, "{question:?}");
                    assert!(!candidate.needs_annotation(), "{question:?}");
                }
                other => panic!("{question:?} must resolve, got {other:?}"),
            }
        }
    }
    Ok(())
}

#[test]
fn a_negated_question_resolves_to_the_decision_that_is_negated_too() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:kafka", "Adopt Kafka for events", &[]),
        decision_proposed(2, "d:no-kafka", "Do not adopt Kafka for billing", &[]),
    ])?;

    // Both say "adopt" and "kafka"; only one is negated, in any spelling.
    for question in NEGATED_KAFKA_QUESTIONS {
        for outcome in both_askers(&graph, question)? {
            match outcome {
                ResolveOutcome::Resolved { candidate } => {
                    assert_eq!(candidate.decision_id, "d:no-kafka", "{question:?}");
                    assert!(!candidate.polarity_mismatch, "{question:?}");
                    assert!(candidate.missing_terms.is_empty(), "{question:?}");
                }
                other => panic!("{question:?} must resolve to the negated one, got {other:?}"),
            }
        }
    }

    // Asked without a negation the polarity does not choose: both match, as before.
    for outcome in both_askers(&graph, "adopt kafka")? {
        match outcome {
            ResolveOutcome::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 2);
                assert!(candidates.iter().all(|candidate| !candidate.is_close()));
            }
            other => panic!("expected the two full matches, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn polarity_is_read_from_the_title_not_the_reasons_given() -> Result<()> {
    // A rationale says "not" for many reasons that leave the decision itself positive: it must not
    // make "don't adopt kafka" resolve to the decision that adopted it.
    let graph = graph_from_events([decision_proposed_because(
        1,
        "d:kafka",
        "Adopt Kafka for events",
        "We do not need exactly-once delivery, and nothing else is as cheap.",
        &[],
    )])?;

    for outcome in both_askers(&graph, "don't adopt kafka")? {
        match outcome {
            ResolveOutcome::Ambiguous { candidates } => {
                assert!(candidates[0].polarity_mismatch);
            }
            other => panic!("expected a close candidate, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_close_candidate_of_the_right_polarity_leads_one_of_the_wrong_polarity() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:kafka", "Adopt Kafka for events", &[]),
        decision_proposed(2, "d:no-kafka", "Do not adopt Kafka for billing", &[]),
    ])?;

    // No decision has all of adopt/kafka/billing/queue. The negated one lacks only "queue" and
    // says what was asked; the positive one lacks "billing" and "queue" and is the opposite.
    let asked = "why didn't we adopt kafka for billing queue";
    let response = resolve_decision_for_reading(&graph, asked, None)?;
    match response.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:no-kafka");
            assert_eq!(candidate.missing_terms, vec!["queue".to_owned()]);
            assert!(!candidate.polarity_mismatch);
        }
        other => panic!("expected the negated decision, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_candidate_says_it_is_the_opposite_only_when_it_is() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:kafka",
        "Adopt Kafka for events",
        &[],
    )])?;

    let ResolveOutcome::Ambiguous { candidates } =
        resolve_decision_for_reading(&graph, "don't adopt kafka", None)?.data
    else {
        panic!("expected a close candidate");
    };
    assert_eq!(
        serde_json::to_value(&candidates[0]).expect("candidate serializes")["polarity_mismatch"],
        json!(true)
    );

    let ResolveOutcome::Resolved { candidate } =
        resolve_decision_for_reading(&graph, "adopt kafka", None)?.data
    else {
        panic!("expected a resolution");
    };
    assert!(serde_json::to_value(&candidate)
        .expect("candidate serializes")
        .get("polarity_mismatch")
        .is_none());
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

// A read-only verb (`why`, `verify`, ...) resolves at the bar `recall` uses, and answers with a
// close candidate that leads alone (hivemind-3lko). A verb that writes keeps the stricter bar and
// never picks a close candidate for the asker.

fn reading(graph: &MemoryGraph, description: &str) -> Result<QueryResponse<ResolveOutcome>> {
    resolve_decision_for_reading(graph, description, None)
}

const LINKS_TITLE: &str = "Decision links use title slugs worked out the same in every browser";

#[test]
fn reading_answers_a_question_that_shares_half_its_words() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:links", LINKS_TITLE, &["links"]),
        decision_proposed(2, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;
    // keep, links, stable, browsers: the decision has links and browsers, two of four.
    let question = "how do we keep decision links stable in browsers";

    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:links");
            assert_eq!(candidate.missing_terms, vec!["keep", "stable"]);
        }
        other => panic!("expected the decision, got {other:?}"),
    }
    // A verb that writes needs more than half of the words: the same question finds nothing.
    assert_eq!(
        resolve_decision_by_description(&graph, question, None)?.data,
        ResolveOutcome::NotFound
    );
    Ok(())
}

#[test]
fn reading_resolves_a_lone_close_candidate_a_writer_only_lists() -> Result<()> {
    let graph = graph_from_events([decision_proposed(
        1,
        "d:design",
        DESIGN_SYSTEM_TITLE,
        &["design"],
    )])?;
    let question = "why did we finally pick shadcn for the design system";

    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:design");
            assert_eq!(candidate.missing_terms, vec!["finally"]);
        }
        other => panic!("expected the decision with what it lacks, got {other:?}"),
    }
    match resolve_decision_by_description(&graph, question, None)?.data {
        ResolveOutcome::Ambiguous { candidates } => assert_eq!(candidates.len(), 1),
        other => panic!("a writer lists the close candidate, got {other:?}"),
    }
    Ok(())
}

#[test]
fn reading_lists_close_candidates_that_lack_the_same_number_of_words() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:upheld", "Name the product Upheld", &[]),
        decision_proposed(2, "d:jev", "Call the product Jev", &[]),
    ])?;

    // product, called, upheld: each decision lacks exactly one, and each carries the two words it
    // has in its title, so nothing says which was meant.
    match reading(&graph, "is the product still called Upheld")?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            let mut ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            ids.sort_unstable();
            assert_eq!(ids, vec!["d:jev", "d:upheld"]);
            assert!(candidates.iter().all(|c| c.missing_terms.len() == 1));
        }
        other => panic!("expected both close candidates, got {other:?}"),
    }
    Ok(())
}

/// "is the product still called Upheld": the decision about the product's name lacks "called";
/// another says "product" and "called" only inside a long rationale and lacks "upheld"
/// (hivemind-3lko). Each lacks one word, so only where the words matched tells them apart.
fn upheld_graph() -> Result<MemoryGraph> {
    graph_from_events([
        decision_proposed_because(
            1,
            "d:triage",
            "Triage gate runs on a local model",
            "The product was called Jev while the gate was built against one shape.",
            &[],
        ),
        decision_proposed_because(
            2,
            "d:name",
            "Name the product Upheld",
            "A short name that says what the record does.",
            &[],
        ),
    ])
}

#[test]
fn reading_answers_with_the_candidate_whose_title_carries_the_words() -> Result<()> {
    let graph = upheld_graph()?;

    // d:name has product and upheld in its title and lacks "called"; d:triage has product and
    // called, but only in its rationale, and lacks "upheld". The title is what says which decision
    // is about the product's name, so that one is the answer, and it names what it lacks.
    match reading(&graph, "is the product still called Upheld")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:name");
            assert_eq!(candidate.missing_terms, vec!["called"]);
        }
        other => panic!("expected the decision about the name, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_writer_lists_the_same_candidates_in_the_same_order_and_picks_none() -> Result<()> {
    let graph = upheld_graph()?;

    match resolve_decision_by_description(&graph, "is the product still called Upheld", None)?.data
    {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids, vec!["d:name", "d:triage"]);
        }
        other => panic!("a writer lists close candidates, got {other:?}"),
    }
    Ok(())
}

#[test]
fn the_decision_whose_title_says_the_words_is_listed_ahead_of_one_that_lacks_fewer() -> Result<()> {
    let graph = graph_from_events([
        // Lacks only "zebra", but has three of the other four only in its rationale.
        decision_proposed_because(
            1,
            "d:one",
            "Queue notes",
            "Adopt the async queue for billing.",
            &[],
        ),
        // Lacks two words, but carries the three it has in its title.
        decision_proposed(2, "d:two", "Adopt async queue", &[]),
        decision_proposed(3, "d:db", "Use Postgres directly", &["storage"]),
        decision_proposed(4, "d:ui", "Two columns for the layout", &["ui"]),
    ])?;

    // adopt, async, queue, billing, zebra: d:one matches four of five, d:two three of five. The
    // title of d:two is what says which decision is about the question, so it is listed first;
    // it lacks more words than d:one, so neither is named as the answer (hivemind-tfde).
    for outcome in both_askers(&graph, "adopt async queue billing zebra")? {
        match outcome {
            ResolveOutcome::Ambiguous { candidates } => {
                let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
                assert_eq!(ids, vec!["d:two", "d:one"]);
                assert_eq!(candidates[0].missing_terms, vec!["billing", "zebra"]);
                assert_eq!(candidates[1].missing_terms, vec!["zebra"]);
            }
            other => panic!("expected the list, the decision about the question first: {other:?}"),
        }
    }
    Ok(())
}

/// "why did we choose Postgres for the hosted cell": the ledger has a decision about the hosted
/// plan whose rationale mentions the cell, and none about Postgres (hivemind-tfde).
fn hosted_graph() -> Result<MemoryGraph> {
    graph_from_events([
        decision_proposed_because(
            1,
            "d:plan",
            "Hosted plan is picked after the comparison and the testers' feedback",
            "The hosted cell stays on one node until the first testers report back.",
            &["pricing"],
        ),
        decision_proposed(2, "d:queue", "Adopt async queue for billing", &["billing"]),
        decision_proposed(3, "d:ui", "Two columns for the layout", &["ui"]),
    ])
}

#[test]
fn a_close_candidate_whose_headline_holds_one_of_the_words_is_listed_not_answered() -> Result<()> {
    let graph = hosted_graph()?;

    // d:plan holds "hosted" in its title and "cell" in its rationale, and lacks "postgres": the
    // word the question is about. One word in a title is what any decision on the subject has.
    match reading(&graph, "postgres for the hosted cell")?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].decision_id, "d:plan");
            assert_eq!(candidates[0].missing_terms, vec!["postgres"]);
        }
        other => panic!("expected the decision listed, not named as the answer: {other:?}"),
    }
    Ok(())
}

#[test]
fn a_close_candidate_whose_headline_holds_two_of_the_words_is_still_answered() -> Result<()> {
    let graph = hosted_graph()?;

    // The same decision, asked about with two words its title holds: it is the one asked about.
    match reading(&graph, "hosted plan comparison with postgres")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:plan");
            assert_eq!(candidate.missing_terms, vec!["postgres"]);
        }
        other => panic!("expected the decision with what it lacks: {other:?}"),
    }
    Ok(())
}

#[test]
fn a_recorded_question_orders_a_close_candidate_but_does_not_get_it_answered() -> Result<()> {
    let mut recorded = decision_proposed_because(
        1,
        "d:engine",
        "Chromium for the UI tests",
        "Real layout needs a real engine.",
        &[],
    );
    recorded.payload["question"] = json!("Which browser engine do front-end checks use?");
    let graph = graph_from_events([
        recorded,
        decision_proposed(2, "d:queue", "Adopt async queue for billing", &["billing"]),
        decision_proposed(3, "d:ui", "Two columns for the layout", &["ui"]),
    ])?;

    // Nothing in the title says "browser" or "engine", but the question the decision answers
    // does, so it is the decision most about the question and comes first. A recorded question
    // can be a paragraph long, so it never alone names the decision as the answer: the list
    // says what the decision lacks and the asker picks.
    match reading(&graph, "which browser engine runs the front-end checks")?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].decision_id, "d:engine");
            assert_eq!(candidates[0].missing_terms, vec!["runs"]);
        }
        other => {
            panic!("expected the decision whose recorded question is asked, listed: {other:?}")
        }
    }
    Ok(())
}

#[test]
fn the_decision_that_lacks_the_word_the_question_is_about_does_not_lead_the_list() -> Result<()> {
    let graph = graph_from_events([
        // About the demo data, in its title, and lacks "instead" and "read".
        decision_proposed_because(
            1,
            "d:demo",
            "Public demo data is a committed copy of the export",
            "A copy checked into the repo is verified by its hash.",
            &["public-demo"],
        ),
        // Says five of the six words in its rationale and none in its title, and lacks "read".
        decision_proposed_because(
            2,
            "d:name",
            "The product's new name is Upheld",
            "The demo data is checked in the repo instead of fetched.",
            &["naming"],
        ),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
        decision_proposed(4, "d:ui", "Two columns for the layout", &["ui"]),
    ])?;

    // d:name lacks fewer words, and used to be answered with. Its title and topic keys carry none
    // of the question, so the decision about the demo data comes first and nothing is answered.
    for outcome in both_askers(&graph, "demo data checked repo instead read")? {
        match outcome {
            ResolveOutcome::Ambiguous { candidates } => {
                let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
                assert_eq!(ids, vec!["d:demo", "d:name"]);
            }
            other => panic!("expected a list with the decision about the demo first: {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn reading_resolves_the_close_candidate_that_lacks_the_fewest_words() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:queue", "Adopt async queue for billing", &[]),
        decision_proposed(2, "d:retries", "Billing retries policy", &[]),
    ])?;

    // queue, billing, async, retries: d:queue lacks one word, d:retries two.
    match reading(&graph, "queue billing async retries")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:queue");
            assert_eq!(candidate.missing_terms, vec!["retries"]);
        }
        other => panic!("expected the closer decision, got {other:?}"),
    }
    Ok(())
}

#[test]
fn reading_lists_but_does_not_answer_with_one_shared_word() -> Result<()> {
    let graph = projects_graph()?;

    // Only "billing" matches: half of two words is enough to list, not to answer with.
    match reading(&graph, "billing unicorn")?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].decision_id, "d:queue");
            assert_eq!(candidates[0].missing_terms, vec!["unicorn"]);
        }
        other => panic!("expected a one-candidate list, got {other:?}"),
    }
    // No word shared is still nothing.
    assert_eq!(
        reading(&graph, "unicorn pegasus")?.data,
        ResolveOutcome::NotFound
    );
    Ok(())
}

#[test]
fn reading_still_prefers_a_full_match_and_keeps_the_ambiguity_gate() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:full", "Adopt async queue for billing", &[]),
        decision_proposed(2, "d:near", "Adopt async retries for billing", &[]),
        decision_proposed(3, "d:billing", "Move billing to Postgres", &[]),
        decision_proposed(4, "d:notify", "Move notifications to Postgres", &[]),
    ])?;

    match reading(&graph, "adopt async queue billing")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:full");
            assert!(candidate.missing_terms.is_empty());
        }
        other => panic!("expected the full match, got {other:?}"),
    }
    match reading(&graph, "why did we move to postgres")?.data {
        ResolveOutcome::Ambiguous { candidates } => assert_eq!(candidates.len(), 2),
        other => panic!("expected the two full matches, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_close_match_is_named_in_the_envelope_and_a_full_match_is_not() {
    let close = ResolvedCandidate {
        decision_id: "d:links".to_owned(),
        title: LINKS_TITLE.to_owned(),
        rank: 1,
        event_origin: 1,
        matched_fields: vec!["title".to_owned()],
        missing_terms: vec!["keep".to_owned(), "stable".to_owned()],
        polarity_mismatch: false,
        also_recorded_as: Vec::new(),
    };
    let full = ResolvedCandidate {
        missing_terms: Vec::new(),
        ..close.clone()
    };

    let mut envelope = json!({"result_count": 1, "data": {}});
    annotate_resolution(&mut envelope, &full);
    assert!(envelope.get("close_match").is_none());

    annotate_resolution(&mut envelope, &close);
    assert_eq!(
        envelope["close_match"],
        json!({
            "decision_id": "d:links",
            "title": LINKS_TITLE,
            "missing_terms": ["keep", "stable"],
        })
    );
}

// ---------------------------------------------------------------------------
// Records of one decision, linked SAME_AS (hivemind-83cj)
// ---------------------------------------------------------------------------

fn same_as(sequence: u128, from_id: &str, to_id: &str) -> Event {
    Event {
        event_type: EventType::RelationAdded,
        payload: json!({"relation": "SAME_AS", "from_id": from_id, "to_id": to_id}),
        ..decision_proposed(sequence, "unused", "unused", &[])
    }
}

/// Three records of one ruling, the third lacking two words the first two have.
fn fable_records() -> Vec<Event> {
    vec![
        decision_proposed(1, "d:1", "Run every town seat on Fable only this week", &[]),
        decision_proposed(
            2,
            "d:2",
            "Run every town seat on Fable with no added usage this week",
            &[],
        ),
        decision_proposed(
            3,
            "d:3",
            "Run every seat on Fable only until the reset",
            &[],
        ),
    ]
}

const FABLE_QUESTION: &str = "run town seat week";

#[test]
fn records_of_one_ruling_with_no_link_between_them_stay_ambiguous() -> Result<()> {
    let graph = graph_from_events(fable_records())?;

    let outcome = resolve_decision_for_reading(&graph, FABLE_QUESTION, None)?.data;

    match outcome {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates.len(), 2);
            assert!(candidates.iter().all(|c| c.also_recorded_as.is_empty()));
        }
        other => panic!("closeness alone never folds: {other:?}"),
    }
    Ok(())
}

#[test]
fn linked_records_resolve_as_one_decision_naming_the_others() -> Result<()> {
    let mut events = fable_records();
    events.push(same_as(4, "d:2", "d:1"));
    events.push(same_as(5, "d:3", "d:1"));
    let graph = graph_from_events(events)?;

    for response in [
        resolve_decision_for_reading(&graph, FABLE_QUESTION, None)?,
        resolve_decision_by_description(&graph, FABLE_QUESTION, None)?,
    ] {
        assert_eq!(response.result_count, 1);
        match response.data {
            ResolveOutcome::Resolved { candidate } => {
                assert_eq!(candidate.decision_id, "d:1", "the earliest record");
                assert!(candidate.missing_terms.is_empty(), "{candidate:?}");
                let others: Vec<&str> = candidate
                    .also_recorded_as
                    .iter()
                    .map(|copy| copy.decision_id.as_str())
                    .collect();
                assert_eq!(others, vec!["d:2", "d:3"]);
                assert_eq!(
                    candidate.also_recorded_as[1].title,
                    "Run every seat on Fable only until the reset"
                );
            }
            other => panic!("expected one decision, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_link_folds_only_the_records_it_joins() -> Result<()> {
    // d:2 and d:3 are one decision; d:1 is a second decision that happens to share the words.
    let mut events = fable_records();
    events.push(same_as(4, "d:3", "d:2"));
    let graph = graph_from_events(events)?;

    match resolve_decision_for_reading(&graph, FABLE_QUESTION, None)?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids.len(), 2, "{ids:?}");
            assert!(ids.contains(&"d:1") && ids.contains(&"d:2"), "{ids:?}");
            let folded = candidates
                .iter()
                .find(|c| c.decision_id == "d:2")
                .expect("d:2");
            assert_eq!(folded.also_recorded_as.len(), 1);
            assert_eq!(folded.also_recorded_as[0].decision_id, "d:3");
        }
        other => panic!("two different decisions stay a tie: {other:?}"),
    }
    Ok(())
}

#[test]
fn the_earliest_record_that_matches_is_shown_even_when_an_earlier_one_does_not() -> Result<()> {
    let mut events = vec![decision_proposed(
        1,
        "d:1",
        "Keep the demo cell on its own host",
        &[],
    )];
    events.push(decision_proposed(
        2,
        "d:2",
        "Move the demo cell to shared Postgres",
        &[],
    ));
    events.push(decision_proposed(
        3,
        "d:3",
        "Move the demo cell onto shared Postgres",
        &[],
    ));
    events.push(same_as(4, "d:2", "d:1"));
    events.push(same_as(5, "d:3", "d:1"));
    let graph = graph_from_events(events)?;

    match resolve_decision_for_reading(&graph, "move demo cell shared postgres", None)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:2");
            let others: Vec<&str> = candidate
                .also_recorded_as
                .iter()
                .map(|copy| copy.decision_id.as_str())
                .collect();
            assert_eq!(
                others,
                vec!["d:1", "d:3"],
                "the record that did not match is listed too"
            );
        }
        other => panic!("expected one decision, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_decision_looked_up_by_id_is_not_folded() -> Result<()> {
    let mut events = fable_records();
    events.push(same_as(4, "d:2", "d:1"));
    let graph = graph_from_events(events)?;

    match resolve_decision_by_id(&graph, "d:2")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:2");
            assert!(candidate.also_recorded_as.is_empty());
        }
        other => panic!("expected the decision asked for: {other:?}"),
    }
    Ok(())
}

#[test]
fn the_envelope_names_the_other_records_beside_data() {
    let candidate = ResolvedCandidate {
        decision_id: "d:1".to_owned(),
        title: "Run every town seat on Fable".to_owned(),
        rank: 1,
        event_origin: 1,
        matched_fields: vec!["title".to_owned()],
        missing_terms: Vec::new(),
        polarity_mismatch: false,
        also_recorded_as: vec![RecordedCopy {
            decision_id: "d:2".to_owned(),
            title: "Fable for every town seat".to_owned(),
        }],
    };
    let mut envelope = json!({"data": {}});
    annotate_resolution(&mut envelope, &candidate);

    assert_eq!(
        envelope["also_recorded_as"],
        json!([{"decision_id": "d:2", "title": "Fable for every town seat"}])
    );
    assert!(envelope.get("close_match").is_none());
    assert!(candidate.needs_annotation());

    let mut plain = json!({"data": {}});
    let alone = ResolvedCandidate {
        also_recorded_as: Vec::new(),
        ..candidate
    };
    annotate_resolution(&mut plain, &alone);
    assert_eq!(plain, json!({"data": {}}));
    assert!(!alone.needs_annotation());
}

#[test]
fn a_verb_that_shows_reads_other_word_forms_and_stand_ins_and_a_verb_that_writes_does_not(
) -> Result<()> {
    let graph = graph_from_events([
        decision_proposed_because(
            1,
            "d:ui",
            "The UI names every agent 'an agent'",
            "A reader needs to know a person from an agent.",
            &[],
        ),
        decision_proposed_because(
            2,
            "d:history",
            "Status history lists only what the log gives",
            "No time for acceptance, rejection or supersession.",
            &[],
        ),
    ])?;

    for (question, expected) in [
        ("interface agent", "d:ui"),
        ("status history superseded", "d:history"),
    ] {
        match resolve_decision_for_reading(&graph, question, None)?.data {
            ResolveOutcome::Resolved { candidate } => {
                assert_eq!(candidate.decision_id, expected, "{question:?}");
                assert!(candidate.missing_terms.is_empty(), "{question:?}");
            }
            other => panic!("{question:?} should resolve for a verb that shows: {other:?}"),
        }
        // A synonym or another form never picks what a verb writes to.
        let written = resolve_decision_by_description(&graph, question, None)?.data;
        assert!(
            !matches!(written, ResolveOutcome::Resolved { .. }),
            "{question:?} must not resolve for a verb that writes: {written:?}"
        );
    }
    Ok(())
}

#[test]
fn a_decision_with_the_word_is_not_tied_with_one_that_only_has_a_stand_in() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:ui", "UI layout is two columns", &[]),
        decision_proposed(2, "d:interface", "Interface layout is two columns", &[]),
    ])?;

    // Both match both words, in the same rank tier; only one has the word that was asked for.
    match resolve_decision_for_reading(&graph, "interface layout", None)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:interface");
        }
        other => panic!("expected the decision that says \"interface\", got {other:?}"),
    }
    Ok(())
}

// A decision that holds every word of a question only somewhere in a long rationale is not the
// one the question is about when another decision carries the words in its title and topic keys
// (hivemind-eral). Where the words sit is compared before whether every word is somewhere.

/// "is the product still called Upheld": a triage decision whose rationale happens to say all of
/// product, called and upheld, and the decision that names the product, which never says "called".
fn product_name_graph() -> Result<MemoryGraph> {
    graph_from_events([
        decision_proposed_because(
            1,
            "d:triage",
            "Triage gate runs on a local model",
            "The product was called Upheld in the first demos, and it is still called that in the pitch.",
            &["triage"],
        ),
        decision_proposed_because(
            2,
            "d:name",
            "Name the product Upheld",
            "A short name that says what the record does.",
            &["naming"],
        ),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])
}

#[test]
fn reading_answers_with_the_decision_about_the_question_not_the_one_that_holds_every_word(
) -> Result<()> {
    let graph = product_name_graph()?;

    // d:triage matches product, called and upheld in full, all in its rationale. d:name has
    // product and upheld in its title and lacks "called". The full match used to win silently.
    match reading(&graph, "is the product still called Upheld")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:name");
            assert_eq!(candidate.missing_terms, vec!["called"]);
            assert!(candidate.is_close());
        }
        other => panic!("expected the decision about the name, saying what it lacks: {other:?}"),
    }
    Ok(())
}

#[test]
fn a_writer_lists_the_decision_about_the_question_first_and_picks_none() -> Result<()> {
    let graph = product_name_graph()?;

    match resolve_decision_by_description(&graph, "is the product still called Upheld", None)?.data
    {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids, vec!["d:name", "d:triage"]);
            assert_eq!(candidates[0].missing_terms, vec!["called"]);
            assert!(candidates[1].missing_terms.is_empty());
        }
        other => panic!("a writer must not write to either without a pick: {other:?}"),
    }
    Ok(())
}

#[test]
fn a_close_candidate_that_carries_no_more_of_the_question_is_not_ahead_of_a_full_match(
) -> Result<()> {
    let graph = graph_from_events([
        // Holds every word, two of them in its title.
        decision_proposed_because(
            1,
            "d:full",
            "Name the product Upheld",
            "Called Upheld since the first demo.",
            &[],
        ),
        // Lacks "called" and has the same two words in its title: nothing more about the question.
        decision_proposed(2, "d:close", "Rename the product Upheld", &[]),
    ])?;

    for outcome in both_askers(&graph, "is the product still called Upheld")? {
        match outcome {
            ResolveOutcome::Resolved { candidate } => {
                assert_eq!(candidate.decision_id, "d:full");
                assert!(candidate.missing_terms.is_empty());
            }
            other => panic!("the full match stays the answer, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_full_match_with_the_words_in_its_title_is_ahead_of_one_that_holds_them_in_its_rationale(
) -> Result<()> {
    let graph = graph_from_events([
        decision_proposed(1, "d:title", "Adopt async queue for billing", &[]),
        decision_proposed_because(
            2,
            "d:body",
            "Billing notes",
            "We adopt the async queue for it.",
            &[],
        ),
    ])?;

    // Both are full matches in the same rank tier (the title of each holds a word), so the rank
    // tier alone would call them equals. One says all four words in its title: a verb that shows
    // answers with it, and a verb that writes lists both and picks none, since where the words
    // sit orders what it is shown but never picks what it writes to (hivemind-293q).
    match reading(&graph, "adopt async queue billing")?.data {
        ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:title"),
        other => panic!("expected the decision whose title says it, got {other:?}"),
    }
    match resolve_decision_by_description(&graph, "adopt async queue billing", None)?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids, vec!["d:title", "d:body"]);
        }
        other => panic!("a writer lists both full matches and picks none, got {other:?}"),
    }
    Ok(())
}

#[test]
fn between_equal_headlines_the_decision_whose_title_says_the_words_is_the_one_about_them(
) -> Result<()> {
    let graph = graph_from_events([
        // Both carry "product" and "naming" in their headline and have a word in their title, so
        // the rank tier calls them equals. One says both in its title; the other files "product"
        // under a topic key.
        decision_proposed(1, "d:filed", "Naming rules", &["product"]),
        decision_proposed(2, "d:titled", "Product naming rules", &[]),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;

    match reading(&graph, "product naming")?.data {
        ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:titled"),
        other => panic!("expected the decision whose title carries the words, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_negated_question_is_answered_by_the_decision_about_it_not_one_that_only_holds_every_word(
) -> Result<()> {
    let graph = graph_from_events([
        // About the question and negated like it, but it never says "show".
        decision_proposed(
            1,
            "d:ui",
            "The UI names every agent 'an agent', never by the tool or role it ran as",
            &[],
        ),
        // Says all five words in its rationale and none of them in its title: it holds every word
        // and is not about the question, and its title does not say what was denied either.
        decision_proposed_because(
            2,
            "d:listing",
            "Listing order after proof",
            "The UI could show which tool an agent ran in, in a list.",
            &[],
        ),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;
    let question = "why doesn't the UI show which tool an agent ran in?";

    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:ui");
            assert_eq!(candidate.missing_terms, vec!["show"]);
            assert!(!candidate.polarity_mismatch);
        }
        other => panic!("expected the negated decision about the question: {other:?}"),
    }
    match resolve_decision_by_description(&graph, question, None)?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            assert_eq!(candidates[0].decision_id, "d:ui");
            assert_eq!(candidates[1].decision_id, "d:listing");
            assert!(!candidates[1].polarity_mismatch);
        }
        other => panic!("a writer lists both and picks none: {other:?}"),
    }
    Ok(())
}

#[test]
fn the_decision_that_records_the_question_stays_ahead_of_a_decision_with_more_in_its_title(
) -> Result<()> {
    // The question as asked is the question this decision answers; another decision's title says
    // more of its words, and still is not the decision the question names.
    let mut recorded = decision_proposed_because(
        1,
        "d:recorded",
        "Chromium for the UI tests",
        "Real layout needs a real engine.",
        &[],
    );
    recorded.payload["question"] = json!("Which browser engine do front-end checks use?");
    let graph = graph_from_events([
        recorded,
        decision_proposed(2, "d:titled", "Browser engine front-end checks use", &[]),
    ])?;

    for outcome in both_askers(&graph, "Which browser engine do front-end checks use?")? {
        match outcome {
            ResolveOutcome::Resolved { candidate } => {
                assert_eq!(candidate.decision_id, "d:recorded");
                assert_eq!(candidate.rank, 0);
            }
            other => panic!("the recorded question names its decision, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_negated_question_is_not_answered_by_whatever_decision_says_no_somewhere() -> Result<()> {
    // hivemind-z05v: the negation of "... and why not the change days?" is polarity, and only a
    // title that says every word asked outright is the opposite of it. The decision asked about
    // states its answer without a negation and leaves "page" to its rationale; another decision,
    // about a competitor check, has "no" in its title and every word in its rationale.
    let graph = graph_from_events([
        decision_proposed_because(
            1,
            "d:history",
            "Status history lists only what the log gives the UI; change days wait on the next release",
            "The page lists the days the log gives it and none it would have to guess.",
            &[],
        ),
        decision_proposed_because(
            2,
            "d:competitors",
            "Competitor check: no change to positioning; showcase leads with contested status",
            "Each competitor page lists the days it was checked in its status history, and why not to change the days is in the notes.",
            &[],
        ),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;
    let question =
        "which days does the page list in its status history and why not the change days?";

    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:history");
            assert!(!candidate.polarity_mismatch);
            assert!(candidate.missing_terms.is_empty());
        }
        other => panic!("the decision asked about answers, got {other:?}"),
    }
    // Both hold every word and neither says the opposite of the question: a writer is given both,
    // the decision asked about first (hivemind-293q).
    match resolve_decision_by_description(&graph, question, None)?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids, vec!["d:history", "d:competitors"]);
            assert!(candidates
                .iter()
                .all(|c| !c.polarity_mismatch && !c.is_close()));
        }
        other => panic!("a writer lists both and picks none, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_title_is_the_opposite_only_when_it_says_everything_the_question_denies_outright() -> Result<()>
{
    let graph = graph_from_events([decision_proposed(
        1,
        "d:links",
        "Decision links use title slugs, not slugs remembered per browser",
        &[],
    )])?;

    // The title says "links use title slugs" outright and denies only the per-browser kind, so a
    // question that denies all of what it says is the opposite of it, whatever it denies elsewhere.
    for outcome in both_askers(&graph, "why don't decision links use title slugs")? {
        match outcome {
            ResolveOutcome::Ambiguous { candidates } => {
                assert_eq!(candidates.len(), 1);
                assert!(candidates[0].polarity_mismatch);
                assert!(candidates[0].missing_terms.is_empty());
            }
            other => panic!("the decision says what was denied, got {other:?}"),
        }
    }

    // Asking about "keep" as well, a word the title does not say, leaves the decision no longer
    // the opposite of the question: it is a close candidate that lacks "keep", like any other.
    match resolve_decision_for_reading(
        &graph,
        "why didn't we keep title slugs for decision links",
        None,
    )?
    .data
    {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:links");
            assert_eq!(candidate.missing_terms, vec!["keep".to_owned()]);
            assert!(!candidate.polarity_mismatch);
        }
        other => panic!("a title that leaves a word out is not the opposite, got {other:?}"),
    }
    Ok(())
}

/// The arrow question (hivemind-ctok): what the site's diagrams put on the arrow from a new
/// decision to the one it replaced. The decision that answers it says "site" and "replaces" in its
/// title and the diagrams and the arrow as *graphs* and an *edge*; the decision that used to lead
/// says "site", "one" and "replacement" and the diagrams as *graph*, and says nothing of an arrow.
/// A title that says the question in other words is about it, so it comes first.
#[test]
fn a_decision_whose_title_says_the_question_in_other_words_leads_one_with_a_generic_word_more(
) -> Result<()> {
    let graph = graph_from_events([
        decision_proposed_because(
            1,
            "d:label",
            "Site graphs label the supersedes edge 'replaces' and draw coming-next parts solid with a COMING NEXT mark",
            "The bead lists replaces among the product's graph words and the page says replace in plain words everywhere, while the app's graph reads supersedes; the site uses replaces and the claims file records the difference. Coming-next parts are merged and built, so they are drawn solid with a COMING NEXT mark; only planned parts are dashed and marked PLANNED.",
            &["website", "graphs"],
        ),
        decision_proposed_because(
            2,
            "d:chain",
            "Site graph 'one assumption falls' shows refutation marking only direct dependents; the chain goes stale on replacement",
            "The product marks a decision as not holding when an assumption it rests on directly is refuted, one hop. A decision that follows from such a decision is not marked until the decision in between is replaced, and then it reads stale. Drawing the whole chain lighting up on the refutation alone would show something neither built nor planned.",
            &["website", "graphs", "honesty"],
        ),
        decision_proposed_because(
            3,
            "d:name",
            "The product's new name is Upheld, home domain upheld.sh, replacing the working name (not Standing, not Decisis)",
            "A, Upheld. Home domain upheld.sh; the domain is bought by the maintainer; nothing is registered or reserved; the rename itself is not planned or started. Why the working name goes: it is overloaded and taken, on the main registries and with dozens of repositories of that exact name. Options: A = Upheld, a decision is upheld or it is not; B = Standing; C = Decisis. The site and the docs move to the new domain at the rename, one word at a time.",
            &["product-name", "naming", "rename", "domain"],
        ),
        decision_proposed_because(
            4,
            "d:demo-copy",
            "Public demo data: commit a byte-exact copy of the generator export, checked by recorded SHA-256",
            "The demo build must work without a hivemind checkout (CI, the site rebuild), so the generator's graph and briefs files are committed rather than read from hivemind at build time. A sync script copies them byte for byte and records the source commit and each file's hash; a test fails if a copy no longer matches.",
            &["ui", "public-demo", "example-data"],
        ),
        decision_proposed_because(
            5,
            "d:flow-titles",
            "Flow node titles are laid out in screen space so none overprints: nudge, then leader line, else hide",
            "Titles are overlays anchored above each node; on a phone fourteen titles share a narrow band. A greedy placer keeps each title on its spot if free, else the nearest free nudge with a faint leader line back to its node, else hides it; key titles first, then birth order.",
            &["ui", "flow", "labels", "layout"],
        ),
        decision_proposed_because(
            6,
            "d:slugs-browser",
            "Decision links use title slugs worked out the same in every browser, not slugs remembered per browser",
            "The server keeps no slug, and titles can change. Showing a slug remembered by one browser would let two browsers give one link to different decisions, so a shared link could open the wrong decision. Deterministic slugs open the same decision everywhere. Old links survive a retitle through the id suffix.",
            &["ui", "links", "routing"],
        ),
        decision_proposed_because(
            7,
            "d:status-history",
            "Decision page status history: one row per logged status change; the demo snapshot keeps its undated rows",
            "The server's status-events read dates every proposal, acceptance, rejection and supersession and names who made it and the status it led to, so each history row is one logged change. Nothing is derived in the browser. The public demo's snapshot carries only the graph and the briefs, so it keeps the rows it showed before.",
            &["ui", "status", "decision-page"],
        ),
        decision_proposed_because(
            8,
            "d:verdict-note",
            "A superseded decision that was contested says so on a second line of its one verdict note, naming who rejected it",
            "The decision page showed one verdict note and chose superseded before contested, so a contested first vote that was later superseded hid its disagreement. Keeping one note with a second line adds the contest without a new layout element and keeps the supersession first.",
            &["decision-page", "verdict"],
        ),
        decision_proposed_because(
            9,
            "d:deck",
            "Pitch deck: the value and who it is for instead of install steps, graphs over the held-up shot, still eight slides",
            "The install commands go; the deck says the value and who it is for, described by values. Each of the eight slides is made fuller instead of adding new ones.",
            &["pitch-deck", "positioning"],
        ),
        decision_proposed_because(
            10,
            "d:listing",
            "Listing order after proof: Claude Code plugin first, then MCP Registry, then community lists",
            "All three listings follow the own-use trial, in the order the plugin, the registry, the community lists. The registry is the next step right after having shown that it actually works.",
            &["channels", "distribution"],
        ),
        decision_proposed_because(
            11,
            "d:related-layer",
            "UI shows possibly-related decisions as an opt-in Graph layer: dashed, no arrowhead, first five per decision",
            "The layer is off by default, drawn dashed, muted and without arrowheads, joining the selected decision to the first page of five suggestions, so none reads as a recorded relation.",
            &["ui", "graph"],
        ),
        decision_proposed_because(
            12,
            "d:no-people",
            "The graph draws no people; who decided shows under the selected decision's title",
            "Showing relation words at rest would put the server's passive 'proposed by' and 'accepted by' on every actor edge, and there is no active verb that reads decision to actor along an arrow that runs newer to older. Who decided shows on the decision itself.",
            &["ui", "graph", "actors"],
        ),
        decision_proposed_because(
            13,
            "d:agent-name",
            "The UI names every agent 'an agent', never by the tool or role it ran as",
            "An agent's actor id names the tool that ran it and its role in that tool: internal names a reader cannot make sense of. What a reader needs is whether a person or an agent decided, so every agent reads alike.",
            &["ui", "naming"],
        ),
        decision_proposed_because(
            14,
            "d:hosted",
            "The paid shape is picked after the comparison and the testers' feedback; the waitlist price question replaces interviews",
            "The paid shape is chosen after the comparison run and the first testers' feedback, not after interviews. The waitlist price question replaces the interviews, because asking a price is cheaper than scheduling a call. Until then the hosted cell stays on one node and nobody is charged. The comparison covers a hosted plan with self-hosting free against one Pro plan valid hosted or self-hosted.",
            &["pricing", "packaging", "hosted", "waitlist"],
        ),
    ])?;
    let question =
        "what word do the site's diagrams put on the arrow from a new decision to the one it replaced?";

    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:label"),
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids.first(), Some(&"d:label"), "the list is {ids:?}");
        }
        other => panic!("expected the decision about the arrow first, got {other:?}"),
    }
    Ok(())
}

/// Three decisions that hold every word of "why doesn't the page show when it was enabled or
/// retired?" (hivemind-5ctc). The first says two of the four in its title and topic keys, which
/// is half and no more; the other two say one each and hold the rest in their rationales. The
/// first is the most about the question and is not the decision asked about.
fn audit_graph() -> Result<MemoryGraph> {
    graph_from_events([
        decision_proposed_because(
            1,
            "d:flagged",
            "A retired rule that was disputed says so on a second line of its one status note",
            "The status note was one line and chose retired before disputed, so a rule that was enabled and later retired hid its dispute; the page showed it nowhere.",
            &["audit-page"],
        ),
        decision_proposed_because(
            2,
            "d:history",
            "Audit page history lists one row per logged change",
            "Each change is dated: the day a rule was enabled and the day it was retired. The page shows no date for older rows.",
            &["audit-page"],
        ),
        decision_proposed_because(
            3,
            "d:hidden",
            "Rows with no date stay hidden until the log gives a day",
            "The page shows only the rows that the log can place. A rule enabled earlier or retired earlier keeps no day.",
            &["audit-page"],
        ),
        decision_proposed(4, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])
}

#[test]
fn a_full_match_that_leads_on_half_the_words_in_its_headline_is_listed_not_answered() -> Result<()>
{
    let graph = audit_graph()?;

    // d:flagged says "retired" in its title and "page" in a topic key: the most about the
    // question, by one word. Two decisions that are about the status history hold every word
    // as well, so naming d:flagged would hide them behind a confident answer. Negated or not.
    for question in [
        "why doesn't the page show when it was enabled or retired?",
        "why does the page show when it was enabled or retired?",
    ] {
        for outcome in both_askers(&graph, question)? {
            match outcome {
                ResolveOutcome::Ambiguous { candidates } => {
                    let ids: Vec<&str> =
                        candidates.iter().map(|c| c.decision_id.as_str()).collect();
                    assert_eq!(ids.first(), Some(&"d:flagged"), "{question:?}: {ids:?}");
                    assert_eq!(ids.len(), 3, "{question:?}: {ids:?}");
                    assert!(ids.contains(&"d:history") && ids.contains(&"d:hidden"));
                    assert!(
                        candidates.iter().all(|c| c.missing_terms.is_empty()),
                        "every one holds every word: {candidates:?}"
                    );
                }
                other => panic!("{question:?} is listed, not answered: {other:?}"),
            }
        }
    }
    Ok(())
}

#[test]
fn a_full_match_whose_headline_holds_most_of_the_words_is_still_answered() -> Result<()> {
    let graph = audit_graph()?;

    // Two of three words are in d:flagged's title and topic keys: more than half. A verb that
    // shows answers with it. Two other decisions hold every word too, so a verb that writes is
    // given all three, d:flagged first, and writes to none (hivemind-293q).
    let question = "why doesn't the page show when it was retired?";
    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:flagged");
            assert!(candidate.missing_terms.is_empty());
        }
        other => panic!("most of the words are in its headline: {other:?}"),
    }
    match resolve_decision_by_description(&graph, question, None)?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids.first(), Some(&"d:flagged"), "{ids:?}");
            assert_eq!(ids.len(), 3, "{ids:?}");
            assert!(candidates.iter().all(|c| c.missing_terms.is_empty()));
        }
        other => panic!("a writer lists the full matches and picks none, got {other:?}"),
    }
    Ok(())
}

#[test]
fn the_only_decision_holding_every_word_is_answered_whatever_its_headline() -> Result<()> {
    let graph = graph_from_events([
        decision_proposed_because(
            1,
            "d:notes",
            "Notes",
            "The page shows when a rule was enabled or retired.",
            &[],
        ),
        decision_proposed(2, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])?;

    // Nothing else holds the words, so there is no one to list it among.
    for outcome in both_askers(
        &graph,
        "why doesn't the page show when it was enabled or retired?",
    )? {
        match outcome {
            ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:notes"),
            other => panic!("the one decision that holds them all answers: {other:?}"),
        }
    }
    Ok(())
}

/// Two decisions that hold every word of "hosted plan pricing" (hivemind-293q): one says all
/// three in its title, the other only in its rationale, as the decision on the licence line does
/// of a pricing it merely mentions. The title carries the whole question and the other does not.
fn pricing_graph() -> Result<MemoryGraph> {
    graph_from_events([
        decision_proposed_because(
            1,
            "d:plan",
            "Hosted plan pricing is picked after the comparison",
            "The price waits for the first testers' feedback; until then nobody is charged.",
            &["pricing"],
        ),
        decision_proposed_because(
            2,
            "d:licence",
            "No commercial licence line in the README",
            "Self-hosting stays free. The hosted plan pricing is picked after the comparison, so the README names no price.",
            &["licence"],
        ),
        decision_proposed(3, "d:queue", "Adopt async queue for billing", &["billing"]),
    ])
}

#[test]
fn a_writer_lists_two_decisions_that_hold_every_word_though_one_is_more_about_the_question(
) -> Result<()> {
    let graph = pricing_graph()?;
    let question = "hosted plan pricing";

    // A verb that shows answers with the decision whose title carries the question: a wrong pick
    // there shows the wrong decision.
    match reading(&graph, question)?.data {
        ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:plan"),
        other => panic!("a verb that shows answers with the decision about it, got {other:?}"),
    }
    // A verb that writes would put a rejection, a title or a premise on a decision nobody meant,
    // and the ledger cannot take it back: both are listed, the decision about it first.
    match resolve_decision_by_description(&graph, question, None)?.data {
        ResolveOutcome::Ambiguous { candidates } => {
            let ids: Vec<&str> = candidates.iter().map(|c| c.decision_id.as_str()).collect();
            assert_eq!(ids, vec!["d:plan", "d:licence"]);
            assert!(candidates.iter().all(|c| !c.is_close()));
        }
        other => panic!("a writer lists both and writes to neither, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_writer_is_answered_when_one_decision_alone_holds_every_word() -> Result<()> {
    let graph = pricing_graph()?;

    // Every word is in the licence decision and in no other, so there is no one to list it among.
    for description in ["commercial licence README", "commercial licence line"] {
        match resolve_decision_by_description(&graph, description, None)?.data {
            ResolveOutcome::Resolved { candidate } => {
                assert_eq!(candidate.decision_id, "d:licence", "{description:?}");
            }
            other => panic!("{description:?} names one decision alone, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_writer_is_answered_with_the_decision_the_description_names_outright_among_full_matches(
) -> Result<()> {
    let graph = pricing_graph()?;

    // d:licence holds every word of d:plan's title in its rationale, so the title is a full match
    // for both; it is the title of one of them, and naming a decision's title is not describing
    // two.
    let title = "Hosted plan pricing is picked after the comparison";
    match resolve_decision_by_description(&graph, title, None)?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:plan");
            assert_eq!(candidate.rank, 0);
        }
        other => panic!("a decision's own title names it, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_topic_hint_that_leaves_one_full_match_lets_a_writer_through() -> Result<()> {
    let graph = pricing_graph()?;

    match resolve_decision_by_description(&graph, "hosted plan pricing", Some("licence"))?.data {
        ResolveOutcome::Resolved { candidate } => assert_eq!(candidate.decision_id, "d:licence"),
        other => panic!("--topic leaves one decision, got {other:?}"),
    }
    Ok(())
}
