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
fn fewer_missing_words_still_beat_more_words_in_the_title() -> Result<()> {
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
    ])?;

    // adopt, async, queue, billing, zebra: d:one matches four of five, d:two three of five.
    match reading(&graph, "adopt async queue billing zebra")?.data {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, "d:one");
            assert_eq!(candidate.missing_terms, vec!["zebra"]);
        }
        other => panic!("expected the decision lacking one word, got {other:?}"),
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
