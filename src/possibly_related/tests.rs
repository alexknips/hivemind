// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::path::Path;

use crate::projector::memory::MemoryGraph;
use crate::queries::test_fixtures::{CountingGraph, Scenario};
use crate::Result;

use super::*;

const ACTOR: &str = "agent:claude:test";

fn at(second: usize) -> String {
    format!("2026-09-30T10:00:{:02}Z", second % 60)
}

fn related(graph: &MemoryGraph, decision_id: &str) -> Result<PossiblyRelated> {
    related_page(graph, decision_id, 0, None)
}

fn related_page(
    graph: &MemoryGraph,
    decision_id: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<PossiblyRelated> {
    let response = possibly_related(
        graph,
        decision_id,
        &PossiblyRelatedRequest {
            limit,
            cursor: cursor.map(str::to_owned),
        },
    )?;
    Ok(response
        .data
        .unwrap_or_else(|| panic!("{decision_id} has an answer")))
}

fn ids(answer: &PossiblyRelated) -> Vec<&str> {
    answer
        .items
        .iter()
        .map(|item| item.decision_id.as_str())
        .collect()
}

fn reach(key: &str, carried_by: usize) -> TopicReach {
    TopicReach {
        key: key.to_owned(),
        carried_by,
    }
}

/// Thirty decisions filed under one area tag, and a pair that also share a topic only they carry.
/// The area tag alone would link every decision to every other (the hairball hivemind-xarm was
/// filed over); the specific key is what points at the one other decision.
fn area_tag_scenario() -> Result<Scenario> {
    let scenario = Scenario::new();
    for index in 0..30 {
        scenario.decision_topics(
            &format!("d:area-{index:02}"),
            &format!("Area decision {index}"),
            &["refinery"],
            ACTOR,
            &at(index),
        )?;
    }
    scenario.decision_topics(
        "d:gate-1",
        "Gate the Kuzu build",
        &["refinery", "kuzu-gate"],
        ACTOR,
        &at(40),
    )?;
    scenario.decision_topics(
        "d:gate-2",
        "Run Kuzu nightly",
        &["refinery", "kuzu-gate"],
        ACTOR,
        &at(41),
    )?;
    Ok(scenario)
}

#[test]
fn an_area_tag_that_many_decisions_carry_links_nothing_and_is_named_as_set_aside() -> Result<()> {
    let graph = area_tag_scenario()?.graph()?;

    let answer = related(&graph, "d:gate-1")?;

    assert_eq!(
        ids(&answer),
        ["d:gate-2"],
        "the 31 others share only the area tag"
    );
    assert_eq!(answer.total_matches, 1);
    assert_eq!(answer.items[0].shared_topic_keys, [reach("kuzu-gate", 2)]);
    assert!(!answer.items[0].same_conversation);
    assert_eq!(
        answer.ignored_topic_keys,
        [reach("refinery", 32)],
        "the filter is never silent"
    );

    // A decision filed under the area tag alone has nothing specific to link on.
    let alone = related(&graph, "d:area-00")?;
    assert!(alone.items.is_empty(), "{alone:?}");
    assert_eq!(alone.total_matches, 0);
    assert_eq!(alone.ignored_topic_keys, [reach("refinery", 32)]);
    assert_eq!(alone.next_cursor, None);
    Ok(())
}

#[test]
fn the_answer_says_it_is_inferred_and_not_a_recorded_relation() -> Result<()> {
    let graph = area_tag_scenario()?.graph()?;

    let answer = related(&graph, "d:gate-1")?;
    assert_eq!(answer.layer, "inferred");
    assert_eq!(answer.note, INFERRED_NOTE);
    let json = serde_json::to_value(&answer).expect("answer serializes");
    assert_eq!(json["layer"], "inferred");
    assert!(
        json["note"]
            .as_str()
            .is_some_and(|note| note.contains("No one recorded a relation")),
        "{json}"
    );
    Ok(())
}

#[test]
fn the_same_conversation_ranks_before_a_shared_topic_and_the_basis_is_in_the_answer() -> Result<()>
{
    let scenario = Scenario::new();
    // One classification event records three decisions; a fourth was proposed by hand.
    let event = scenario.classified_decisions(
        &[
            ("Ship the importer", &["alpha", "importer"]),
            ("Defer the exporter", &["beta"]),
            ("Name the importer", &["alpha"]),
        ],
        &at(1),
    )?;
    scenario.decision_topics("d:hand", "Document alpha", &["alpha"], ACTOR, &at(2))?;
    let graph = scenario.graph()?;
    let asked = format!("capture:{event}:0");

    let answer = related(&graph, &asked)?;

    // Same conversation first, the one sharing a key before the one sharing none; then the
    // decision recorded elsewhere that shares only "alpha" (carried by three decisions).
    assert_eq!(
        ids(&answer),
        [
            format!("capture:{event}:2").as_str(),
            format!("capture:{event}:1").as_str(),
            "d:hand",
        ]
    );
    let flags: Vec<bool> = answer
        .items
        .iter()
        .map(|item| item.same_conversation)
        .collect();
    assert_eq!(flags, [true, true, false]);
    assert_eq!(answer.items[0].shared_topic_keys, [reach("alpha", 3)]);
    assert!(answer.items[1].shared_topic_keys.is_empty());
    assert_eq!(answer.items[2].shared_topic_keys, [reach("alpha", 3)]);
    assert!(answer.ignored_topic_keys.is_empty());
    Ok(())
}

#[test]
fn one_session_classified_in_two_events_is_one_conversation() -> Result<()> {
    // hivemind-266t: the classifier recorded session "sess-1" in two events (its batches were
    // classified at two moments), and a different session in a third. The decisions share no topic
    // key, so only the session can say that the first two events' decisions belong together. The
    // batch ids do not look like their session: the received batches say which it is.
    let scenario = Scenario::new();
    scenario.received_batch("batch-1", "sess-1", &at(1))?;
    scenario.received_batch("batch-2", "sess-1", &at(2))?;
    scenario.received_batch("batch-3", "sess-2", &at(3))?;
    let first = scenario.classified_decisions_from_batches(
        &["batch-1"],
        &[
            ("Ship the importer", &["alpha"]),
            ("Defer the exporter", &["beta"]),
        ],
        &at(4),
    )?;
    let second = scenario.classified_decisions_from_batches(
        &["batch-2"],
        &[("Name the importer", &["gamma"])],
        &at(5),
    )?;
    let elsewhere = scenario.classified_decisions_from_batches(
        &["batch-3"],
        &[("Pick a logo", &["delta"])],
        &at(6),
    )?;
    let graph = scenario.graph()?;
    let (first_0, first_1) = (format!("capture:{first}:0"), format!("capture:{first}:1"));
    let second_0 = format!("capture:{second}:0");
    let elsewhere_0 = format!("capture:{elsewhere}:0");

    let from_first = related(&graph, &first_0)?;
    assert_eq!(
        ids(&from_first),
        [first_1.as_str(), second_0.as_str()],
        "the same event and the same session, nothing from the other session"
    );
    assert!(
        from_first.items.iter().all(|item| item.same_conversation),
        "{from_first:?}"
    );
    assert!(
        from_first
            .items
            .iter()
            .all(|item| item.shared_topic_keys.is_empty()),
        "the pair is joined by the session alone"
    );

    // Symmetric: the later event's decision finds the earlier event's decisions.
    let from_second = related(&graph, &second_0)?;
    assert_eq!(ids(&from_second), [first_0.as_str(), first_1.as_str()]);
    assert!(from_second.items.iter().all(|item| item.same_conversation));

    // The other session stands alone.
    assert!(related(&graph, &elsewhere_0)?.items.is_empty());
    Ok(())
}

#[test]
fn a_decision_recorded_by_hand_has_no_session_to_share() -> Result<()> {
    // A hand-proposed decision belongs to no capture session, even one that shares its topic key.
    let scenario = Scenario::new();
    scenario.received_batch("batch-1", "sess-1", &at(1))?;
    let event = scenario.classified_decisions_from_batches(
        &["batch-1"],
        &[("Ship the importer", &["alpha"])],
        &at(2),
    )?;
    scenario.decision_topics("d:hand", "Document alpha", &["alpha"], ACTOR, &at(3))?;
    let graph = scenario.graph()?;

    let answer = related(&graph, &format!("capture:{event}:0"))?;

    assert_eq!(ids(&answer), ["d:hand"]);
    assert!(!answer.items[0].same_conversation);
    Ok(())
}

#[test]
fn a_classification_of_batches_never_received_is_grouped_by_its_event() -> Result<()> {
    // Batches the ledger never received (a `hivemind emit` capture names a fresh batch id) leave
    // the decisions without a session, so the classification event that recorded them stays the
    // grouping: its own decisions are one conversation, and a second event is not joined to it.
    let scenario = Scenario::new();
    let first = scenario.classified_decisions_from_batches(
        &["unreceived-batch-1"],
        &[("One", &["a"]), ("Two", &["b"])],
        &at(1),
    )?;
    let second = scenario.classified_decisions_from_batches(
        &["unreceived-batch-2"],
        &[("Three", &["c"])],
        &at(2),
    )?;
    let graph = scenario.graph()?;

    let answer = related(&graph, &format!("capture:{first}:0"))?;

    assert_eq!(ids(&answer), [format!("capture:{first}:1").as_str()]);
    assert!(answer.items[0].same_conversation);
    assert!(related(&graph, &format!("capture:{second}:0"))?
        .items
        .is_empty());
    Ok(())
}

#[test]
fn a_rarer_shared_key_ranks_above_a_commoner_one() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision_topics("d:asked", "Asked", &["common", "rare"], ACTOR, &at(1))?;
    scenario.decision_topics("d:common", "Common", &["common"], ACTOR, &at(2))?;
    scenario.decision_topics("d:rare", "Rare", &["rare"], ACTOR, &at(3))?;
    for index in 0..4 {
        scenario.decision_topics(
            &format!("d:more-{index}"),
            "More",
            &["common"],
            ACTOR,
            &at(4 + index),
        )?;
    }
    let graph = scenario.graph()?;

    let answer = related(&graph, "d:asked")?;

    // "rare" is carried by 2 (weight 1); "common" by 6 (weight 1/5 each decision).
    assert_eq!(ids(&answer)[0], "d:rare");
    assert_eq!(answer.items[0].shared_topic_keys, [reach("rare", 2)]);
    assert_eq!(answer.total_matches, 6);
    // Equal weights fall back to the decision id, so the order is the same every time.
    assert_eq!(
        ids(&answer)[1..],
        ["d:common", "d:more-0", "d:more-1", "d:more-2", "d:more-3"]
    );
    assert_eq!(answer, related(&graph, "d:asked")?);
    Ok(())
}

#[test]
fn a_decision_already_joined_by_a_recorded_relation_is_not_offered_as_possibly_related(
) -> Result<()> {
    let scenario = Scenario::new();
    for (index, id) in ["d:asked", "d:replaced", "d:premise", "d:copy", "d:other"]
        .into_iter()
        .enumerate()
    {
        scenario.decision_topics(id, id, &["shared"], ACTOR, &at(index))?;
    }
    scenario.supersede("d:replaced", "d:asked", ACTOR, &at(10))?;
    scenario.relation("FOLLOWS_FROM", "d:asked", "d:premise", ACTOR, None, &at(11))?;
    scenario.relation("SAME_AS", "d:copy", "d:asked", ACTOR, None, &at(12))?;
    let graph = scenario.graph()?;

    let answer = related(&graph, "d:asked")?;

    assert_eq!(ids(&answer), ["d:other"]);
    // The links are followed in both directions: the linked decisions do not offer the asked
    // one either, while a decision nothing links to it still does.
    for linked in ["d:replaced", "d:premise", "d:copy"] {
        assert!(
            !ids(&related(&graph, linked)?).contains(&"d:asked"),
            "{linked} is joined to d:asked by a recorded relation"
        );
    }
    assert!(ids(&related(&graph, "d:other")?).contains(&"d:asked"));
    Ok(())
}

#[test]
fn each_item_carries_the_status_the_graph_derives() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision_topics("d:asked", "Asked", &["topic"], ACTOR, &at(1))?;
    scenario.decision_topics("d:open", "Open", &["topic"], ACTOR, &at(2))?;
    scenario.decision_topics("d:agreed", "Agreed", &["topic"], ACTOR, &at(3))?;
    scenario.decision_topics("d:gone", "Gone", &["topic"], ACTOR, &at(4))?;
    scenario.decision_topics("d:newer", "Newer", &["other"], ACTOR, &at(5))?;
    scenario.accept("d:agreed", "human:alex", &at(6))?;
    scenario.supersede("d:gone", "d:newer", ACTOR, &at(7))?;
    let graph = scenario.graph()?;

    let answer = related(&graph, "d:asked")?;

    let status = |id: &str| {
        answer
            .items
            .iter()
            .find(|item| item.decision_id == id)
            .map(|item| item.status)
    };
    assert_eq!(status("d:open"), Some(DecisionStatus::Proposed));
    assert_eq!(status("d:agreed"), Some(DecisionStatus::Accepted));
    assert_eq!(
        status("d:gone"),
        Some(DecisionStatus::Superseded),
        "a superseded decision is still shown, as superseded"
    );
    Ok(())
}

#[test]
fn pages_cover_every_match_once_and_say_how_to_continue() -> Result<()> {
    let scenario = Scenario::new();
    let event = scenario.classified_decisions(
        &[
            ("One", &["t"]),
            ("Two", &["t"]),
            ("Three", &["t"]),
            ("Four", &["t"]),
            ("Five", &["t"]),
        ],
        &at(1),
    )?;
    let graph = scenario.graph()?;
    let asked = format!("capture:{event}:0");
    let asked = asked.as_str();

    let first = related_page(&graph, asked, 2, None)?;
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.total_matches, 4);
    assert_eq!(first.next_cursor.as_deref(), Some("2"));
    let truncated = possibly_related(
        &graph,
        asked,
        &PossiblyRelatedRequest {
            limit: 2,
            cursor: None,
        },
    )?;
    assert!(truncated.truncated, "a partial page says it is partial");

    let second = related_page(&graph, asked, 2, first.next_cursor.as_deref())?;
    assert_eq!(second.items.len(), 2);
    assert_eq!(
        second.next_cursor, None,
        "the last page has no continuation"
    );
    let last = possibly_related(
        &graph,
        asked,
        &PossiblyRelatedRequest {
            limit: 2,
            cursor: Some("2".to_owned()),
        },
    )?;
    assert!(!last.truncated);

    let mut seen: Vec<&str> = ids(&first);
    seen.extend(ids(&second));
    let everything = related_page(&graph, asked, 100, None)?;
    assert_eq!(seen, ids(&everything));
    Ok(())
}

#[test]
fn a_decision_that_does_not_exist_has_no_answer_and_bad_arguments_are_refused() -> Result<()> {
    let graph = area_tag_scenario()?.graph()?;

    let missing = possibly_related(&graph, "d:nope", &PossiblyRelatedRequest::default())?;
    assert!(missing.data.is_none());
    assert_eq!(missing.result_count, 0);

    assert!(possibly_related(&graph, "  ", &PossiblyRelatedRequest::default()).is_err());
    let bad_cursor = PossiblyRelatedRequest {
        limit: 5,
        cursor: Some("-1".to_owned()),
    };
    assert!(possibly_related(&graph, "d:gate-1", &bad_cursor).is_err());
    Ok(())
}

#[test]
fn the_cutoff_is_a_fixed_floor_for_a_small_ledger_and_a_share_of_a_large_one() {
    assert_eq!(generic_key_cutoff(0), 10);
    assert_eq!(generic_key_cutoff(533), 10);
    assert_eq!(generic_key_cutoff(550), 11);
    assert_eq!(generic_key_cutoff(10_000), 200);
}

#[test]
fn a_page_costs_the_same_reads_however_many_decisions_match() -> Result<()> {
    let small = Scenario::new();
    let small_event = small.classified_decisions(&[("A", &["t"]), ("B", &["t"])], &at(1))?;
    let large = Scenario::new();
    let many: Vec<(&str, &[&str])> = (0..40).map(|_| ("Same conversation", &["t"][..])).collect();
    let large_event = large.classified_decisions(&many, &at(1))?;
    let (small_graph, large_graph) = (small.graph()?, large.graph()?);

    let reads = |graph: &MemoryGraph, event: u64| -> Result<usize> {
        let counting = CountingGraph::new(graph);
        let asked = format!("capture:{event}:0");
        possibly_related(&counting, &asked, &PossiblyRelatedRequest::default())?;
        Ok(counting.queries())
    };

    assert_eq!(
        reads(&small_graph, small_event)?,
        reads(&large_graph, large_event)?
    );
    Ok(())
}

// ── Layer boundary ────────────────────────────────────────────────────────────

/// Possibly-related is Layer 3: queries stay pure and the write layer stays dumb. Nothing in
/// `src/queries/` or `src/commands/` may reach for it.
#[test]
fn queries_and_commands_never_import_possibly_related() {
    fn rust_files(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory reads") {
            let path = entry.expect("directory entry reads").path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for layer in ["queries", "commands"] {
        rust_files(&source.join(layer), &mut files);
    }
    assert!(
        !files.is_empty(),
        "found no query or command sources to check"
    );

    for path in files {
        let text = std::fs::read_to_string(&path).expect("source file reads");
        assert!(
            !text.contains("possibly_related"),
            "{} names possibly_related; it is Layer 3 and nothing below it may depend on it",
            path.display()
        );
    }
}
