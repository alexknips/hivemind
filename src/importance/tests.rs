// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::path::Path;

use serde_json::json;

use crate::projector::memory::MemoryGraph;
use crate::queries::test_fixtures::{solid_assessment_dimensions, Scenario};
use crate::Result;

use super::*;

const CREW: &str = "agent:claude:crew";
const ALEX: &str = "human:alex";

/// Decisions recorded in the order given, so a later one is newer by ledger offset.
fn recorded(ids: &[&str]) -> Result<Scenario> {
    let s = Scenario::new();
    for (n, id) in ids.iter().enumerate() {
        s.decision(
            id,
            &format!("Decision {id}"),
            CREW,
            &format!("2026-01-01T00:00:{n:02}Z"),
        )?;
    }
    Ok(s)
}

fn follows(s: &Scenario, follower: &str, premise: &str) -> Result<()> {
    s.relation(
        "FOLLOWS_FROM",
        follower,
        premise,
        CREW,
        None,
        "2026-02-01T00:00:00Z",
    )
}

fn page(graph: &MemoryGraph, request: &ImportanceRequest) -> Result<ImportanceReport> {
    Ok(rank_decisions_by_importance(graph, request)?.data)
}

fn all(graph: &MemoryGraph) -> Result<ImportanceReport> {
    page(
        graph,
        &ImportanceRequest {
            limit: MAX_PAGE_SIZE,
            ..ImportanceRequest::default()
        },
    )
}

fn order(report: &ImportanceReport) -> Vec<&str> {
    report
        .decisions
        .iter()
        .map(|row| row.decision_id.as_str())
        .collect()
}

/// `a` has three decisions resting on it (two directly, one through `b`), `b` and `e` one each,
/// and `c`, `d` and `f` none. `e` was recorded after `b`.
fn ladder() -> Result<MemoryGraph> {
    let s = recorded(&["d:a", "d:b", "d:c", "d:d", "d:e", "d:f"])?;
    follows(&s, "d:b", "d:a")?;
    follows(&s, "d:c", "d:a")?;
    follows(&s, "d:d", "d:b")?;
    follows(&s, "d:f", "d:e")?;
    s.graph()
}

#[test]
fn the_decision_with_most_resting_on_it_leads_and_a_tie_goes_to_the_newest() -> Result<()> {
    let report = all(&ladder()?)?;

    // a has 3 through chains. b and e have 1 each, and e was recorded after b. Nothing rests on
    // f, d or c, and they follow newest first.
    assert_eq!(order(&report), ["d:a", "d:e", "d:b", "d:f", "d:d", "d:c"]);
    assert_eq!(report.ranked_total, 3);
    assert_eq!(report.not_assessed_total, 3);
    Ok(())
}

#[test]
fn ranked_decisions_carry_a_position_and_the_rest_read_not_assessed() -> Result<()> {
    let report = all(&ladder()?)?;

    let ranks: Vec<Option<usize>> = report.decisions.iter().map(|row| row.rank).collect();
    assert_eq!(
        ranks,
        vec![Some(1), Some(2), Some(3), None, None, None],
        "positions run 1.. over the ranked decisions only"
    );
    let importance: Vec<Importance> = report.decisions.iter().map(|row| row.importance).collect();
    assert_eq!(
        importance[..3],
        [Importance::Ranked, Importance::Ranked, Importance::Ranked]
    );
    assert!(importance[3..]
        .iter()
        .all(|reading| *reading == Importance::NotAssessed));

    let leader = &report.decisions[0];
    assert_eq!(leader.rests_on_it.direct, 2);
    assert_eq!(leader.rests_on_it.through_chains, 3);
    assert!(!leader.rests_on_it.chain_capped);
    assert_eq!(leader.rests_on_it.direct_ids, ["d:b", "d:c"]);
    assert_eq!(
        leader.reasons[0],
        "3 decisions follow from it: 2 directly, the rest through chains of decisions that follow from it"
    );
    let unranked = &report.decisions[5];
    assert_eq!(unranked.rests_on_it.through_chains, 0);
    assert!(
        unranked.reasons[0].starts_with("not assessed:"),
        "{:?}",
        unranked.reasons
    );
    Ok(())
}

#[test]
fn a_chain_is_followed_five_hops_and_then_says_it_stopped() -> Result<()> {
    let ids: Vec<String> = (0..8).map(|n| format!("d:n{n}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let s = recorded(&id_refs)?;
    for pair in ids.windows(2) {
        follows(&s, &pair[1], &pair[0])?;
    }
    let facts = get_importance_facts(&s.graph()?)?;

    // n0 -> n1 -> ... -> n7. Five hops from n0 reach n5; n6 and n7 lie beyond.
    assert_eq!(reach_through_chains(&facts, "d:n0"), (5, true));
    // Five hops from n2 reach n7, the end of the chain: nothing lies beyond.
    assert_eq!(reach_through_chains(&facts, "d:n2"), (5, false));
    assert_eq!(reach_through_chains(&facts, "d:n3"), (4, false));
    assert_eq!(reach_through_chains(&facts, "d:n7"), (0, false));
    Ok(())
}

#[test]
fn a_cycle_counts_each_decision_once_and_never_the_decision_itself() -> Result<()> {
    let s = recorded(&["d:a", "d:b", "d:c"])?;
    follows(&s, "d:b", "d:a")?;
    follows(&s, "d:a", "d:b")?;
    follows(&s, "d:a", "d:a")?;
    follows(&s, "d:c", "d:b")?;
    let facts = get_importance_facts(&s.graph()?)?;

    assert_eq!(reach_through_chains(&facts, "d:a"), (2, false));
    assert_eq!(reach_through_chains(&facts, "d:b"), (2, false));
    Ok(())
}

#[test]
fn a_decision_that_is_no_longer_in_force_leaves_the_list_and_is_counted() -> Result<()> {
    let s = recorded(&["d:old", "d:new", "d:no", "d:live", "d:fol"])?;
    follows(&s, "d:fol", "d:old")?;
    follows(&s, "d:fol", "d:no")?;
    follows(&s, "d:fol", "d:live")?;
    s.supersede("d:old", "d:new", CREW, "2026-03-01T00:00:00Z")?;
    s.reject("d:no", ALEX, "2026-03-02T00:00:00Z")?;
    let graph = s.graph()?;

    let default = all(&graph)?;
    assert!(!order(&default).contains(&"d:old"));
    assert!(!order(&default).contains(&"d:no"));
    assert_eq!(
        default.left_out,
        LeftOut {
            superseded: 1,
            rejected: 1
        }
    );
    assert_eq!(default.ranked_total, 1, "only d:live has something on it");

    let everything = page(
        &graph,
        &ImportanceRequest {
            include_not_in_force: true,
            limit: MAX_PAGE_SIZE,
            ..ImportanceRequest::default()
        },
    )?;
    assert_eq!(everything.left_out, LeftOut::default());
    let status_of = |id: &str| {
        everything
            .decisions
            .iter()
            .find(|row| row.decision_id == id)
            .map(|row| row.status)
    };
    assert_eq!(status_of("d:old"), Some(DecisionStatus::Superseded));
    assert_eq!(status_of("d:no"), Some(DecisionStatus::Rejected));
    assert_eq!(everything.ranked_total, 3);
    Ok(())
}

#[test]
fn who_decided_is_the_accepter_and_the_recorder_is_never_read_as_the_decider() -> Result<()> {
    let s = recorded(&["d:root", "d:p", "d:g", "d:m", "d:n"])?;
    for follower in ["d:p", "d:g", "d:m", "d:n"] {
        follows(&s, follower, "d:root")?;
    }
    s.accept("d:p", ALEX, "2026-03-01T00:00:00Z")?;
    s.accept("d:g", CREW, "2026-03-02T00:00:00Z")?;
    s.accept("d:m", ALEX, "2026-03-03T00:00:00Z")?;
    s.accept("d:m", CREW, "2026-03-04T00:00:00Z")?;
    let report = all(&s.graph()?)?;

    let row = |id: &str| {
        report
            .decisions
            .iter()
            .find(|row| row.decision_id == id)
            .expect("the decision is listed")
    };
    assert_eq!(row("d:p").decided_by.kind, DeciderKind::Person);
    assert_eq!(row("d:g").decided_by.kind, DeciderKind::Agent);
    assert_eq!(row("d:m").decided_by.kind, DeciderKind::Mixed);
    let nobody = row("d:n");
    assert_eq!(nobody.decided_by.kind, DeciderKind::NoneRecorded);
    assert!(nobody.decided_by.deciders.is_empty());
    assert_eq!(
        nobody.recorded_by.as_deref(),
        Some(CREW),
        "the recorder is shown, as the recorder"
    );
    assert_eq!(
        row("d:p").decided_by.deciders,
        vec![Decider {
            id: ALEX.to_owned(),
            kind: "human"
        }]
    );
    Ok(())
}

#[test]
fn an_agent_that_decided_within_a_persons_delegation_is_told_apart_from_one_that_decided_alone() {
    let decided_by = |ids: &[&str], delegated_by: Option<&str>| {
        let mut facts = bare_facts();
        facts.deciders = ids
            .iter()
            .map(|id| Decider {
                id: (*id).to_owned(),
                kind: if id.starts_with("human:") {
                    "human"
                } else if id.starts_with("agent:") {
                    "agent"
                } else {
                    "unknown"
                },
            })
            .collect();
        facts.delegated_by = delegated_by.map(str::to_owned);
        decider_kind(&facts)
    };

    assert_eq!(decided_by(&[CREW], None), DeciderKind::Agent);
    assert_eq!(
        decided_by(&[CREW], Some(ALEX)),
        DeciderKind::AgentWithinDelegation
    );
    assert_eq!(decided_by(&[ALEX], Some(ALEX)), DeciderKind::Person);
    assert_eq!(decided_by(&["bot:x"], None), DeciderKind::Unknown);
    assert_eq!(decided_by(&["bot:x", ALEX], None), DeciderKind::Mixed);
    assert_eq!(decided_by(&[], Some(ALEX)), DeciderKind::NoneRecorded);
}

fn bare_facts() -> DecisionFacts {
    DecisionFacts {
        title: "t".to_owned(),
        project: None,
        project_label: "no project recorded".to_owned(),
        occurred_at: None,
        event_origin: None,
        status: DecisionStatus::Accepted,
        deciders: Vec::new(),
        rejected_by: Vec::new(),
        recorded_by: None,
        delegated_by: None,
        model_judged: None,
    }
}

#[test]
fn the_report_tallies_who_decided_the_ranked_decisions_by_kind_only() -> Result<()> {
    let s = recorded(&["d:root", "d:p", "d:g", "d:n", "d:lone"])?;
    for follower in ["d:p", "d:g", "d:n"] {
        follows(&s, follower, "d:root")?;
    }
    follows(&s, "d:lone", "d:p")?;
    follows(&s, "d:lone", "d:g")?;
    s.accept("d:root", ALEX, "2026-03-01T00:00:00Z")?;
    s.accept("d:p", ALEX, "2026-03-02T00:00:00Z")?;
    s.accept("d:g", CREW, "2026-03-03T00:00:00Z")?;
    let report = all(&s.graph()?)?;

    // Ranked: d:root and d:p (persons), d:g (an agent). Nothing rests on d:n or d:lone.
    assert_eq!(
        report.ranked_decided_by,
        DeciderTally {
            person: 2,
            agent: 1,
            ..DeciderTally::default()
        }
    );
    Ok(())
}

#[test]
fn what_a_model_judged_is_shown_and_never_moves_the_rank() -> Result<()> {
    let s = recorded(&["d:big", "d:small", "d:follower"])?;
    follows(&s, "d:follower", "d:small")?;
    s.assessment_with_importance(
        "d:big",
        "claude-haiku-4-5",
        "assessment-v1",
        solid_assessment_dimensions("t"),
        (9.0, 1.0, 1.0),
        "2026-03-01T00:00:00Z",
    )?;
    let report = all(&s.graph()?)?;

    assert_eq!(order(&report), ["d:small", "d:follower", "d:big"]);
    let big = report
        .decisions
        .iter()
        .find(|row| row.decision_id == "d:big")
        .expect("listed");
    assert_eq!(big.importance, Importance::NotAssessed);
    assert_eq!(big.rank, None);
    let judged = big.model_judged.as_ref().expect("the model's numbers show");
    assert_eq!(judged.stakes, 9.0);
    assert_eq!(judged.model.as_deref(), Some("claude-haiku-4-5"));
    assert!(
        big.reasons
            .iter()
            .any(|reason| reason.contains("shown, never used to rank")),
        "{:?}",
        big.reasons
    );
    Ok(())
}

#[test]
fn pages_follow_one_another_without_a_gap_or_a_repeat() -> Result<()> {
    let graph = ladder()?;
    let whole = order_owned(&all(&graph)?);

    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let response = rank_decisions_by_importance(
            &graph,
            &ImportanceRequest {
                limit: 2,
                cursor: cursor.clone(),
                ..ImportanceRequest::default()
            },
        )?;
        pages += 1;
        seen.extend(order_owned(&response.data));
        assert_eq!(
            response.truncated,
            response.data.next_cursor.is_some(),
            "next_cursor is present exactly when the page is truncated"
        );
        match response.data.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert_eq!(pages, 3);
    assert_eq!(seen, whole);
    Ok(())
}

fn order_owned(report: &ImportanceReport) -> Vec<String> {
    order(report).into_iter().map(str::to_owned).collect()
}

#[test]
fn a_cursor_resumes_after_its_row_when_decisions_are_recorded_between_two_pages() -> Result<()> {
    let s = recorded(&["d:a", "d:b", "d:c", "d:d", "d:e", "d:f"])?;
    follows(&s, "d:b", "d:a")?;
    follows(&s, "d:c", "d:a")?;
    follows(&s, "d:d", "d:b")?;
    follows(&s, "d:f", "d:e")?;
    let first = page(
        &s.graph()?,
        &ImportanceRequest {
            limit: 2,
            ..ImportanceRequest::default()
        },
    )?;
    let after_first = first.next_cursor.clone().expect("more follow");

    // A newer decision with nothing on it, and one that outranks everything. The second sorts
    // before the cursor, so the walk does not revisit it; the first sorts after, so it is not
    // skipped.
    s.decision("d:late", "Late", CREW, "2026-04-01T00:00:00Z")?;
    s.decision("d:hub", "Hub", CREW, "2026-04-02T00:00:00Z")?;
    for follower in ["d:a", "d:b", "d:c", "d:d"] {
        follows(&s, follower, "d:hub")?;
    }
    let second = page(
        &s.graph()?,
        &ImportanceRequest {
            limit: 50,
            cursor: Some(after_first),
            ..ImportanceRequest::default()
        },
    )?;

    let shown: Vec<String> = first
        .decisions
        .iter()
        .chain(second.decisions.iter())
        .map(|row| row.decision_id.clone())
        .collect();
    let mut distinct = shown.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(shown.len(), distinct.len(), "no decision twice: {shown:?}");
    assert!(
        order(&second).contains(&"d:late"),
        "a decision recorded in between that sorts after the cursor is not skipped"
    );
    Ok(())
}

#[test]
fn a_page_is_never_larger_than_the_cap_and_says_so() -> Result<()> {
    let ids: Vec<String> = (0..55).map(|n| format!("d:{n:02}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let graph = recorded(&refs)?.graph()?;

    let response = rank_decisions_by_importance(
        &graph,
        &ImportanceRequest {
            limit: 1000,
            ..ImportanceRequest::default()
        },
    )?;

    assert_eq!(response.result_count, MAX_PAGE_SIZE);
    assert!(response.truncated);
    assert!(response.data.next_cursor.is_some());
    assert_eq!(response.data.not_assessed_total, 55);

    let default = rank_decisions_by_importance(&graph, &ImportanceRequest::default())?;
    assert_eq!(default.result_count, DEFAULT_PAGE_SIZE);
    Ok(())
}

#[test]
fn a_cursor_that_no_ranking_returned_is_refused() -> Result<()> {
    let graph = ladder()?;

    let error = rank_decisions_by_importance(
        &graph,
        &ImportanceRequest {
            cursor: Some("17".to_owned()),
            ..ImportanceRequest::default()
        },
    )
    .expect_err("an offset is not a position in the order");

    assert!(error.to_string().contains("cursor is not one"), "{error}");
    Ok(())
}

#[test]
fn an_empty_graph_ranks_nothing() -> Result<()> {
    let response =
        rank_decisions_by_importance(&Scenario::new().graph()?, &ImportanceRequest::default())?;

    assert_eq!(response.result_count, 0);
    assert!(!response.truncated);
    assert!(response.data.decisions.is_empty());
    assert_eq!(response.data.ranked_total, 0);
    assert_eq!(response.data.next_cursor, None);
    Ok(())
}

#[test]
fn a_row_serializes_with_its_position_basis_and_decider() -> Result<()> {
    let s = recorded(&["d:root", "d:leaf"])?;
    follows(&s, "d:leaf", "d:root")?;
    s.accept("d:root", ALEX, "2026-03-01T00:00:00Z")?;
    let report = all(&s.graph()?)?;

    let value = serde_json::to_value(&report).expect("the report serializes");

    let ranked = &value["decisions"][0];
    assert_eq!(ranked["rank"], json!(1));
    assert_eq!(ranked["decision_id"], json!("d:root"));
    assert_eq!(ranked["importance"], json!("ranked"));
    assert_eq!(ranked["status"], json!("accepted"));
    assert_eq!(ranked["rests_on_it"]["direct"], json!(1));
    assert_eq!(ranked["rests_on_it"]["through_chains"], json!(1));
    assert_eq!(ranked["decided_by"]["kind"], json!("person"));
    assert_eq!(ranked["decided_by"]["deciders"][0]["id"], json!(ALEX));
    assert_eq!(ranked["recorded_by"], json!(CREW));
    assert!(ranked.get("model_judged").is_none());
    let unranked = &value["decisions"][1];
    assert!(
        unranked.get("rank").is_none(),
        "no position when not assessed"
    );
    assert_eq!(unranked["importance"], json!("not_assessed"));
    assert_eq!(unranked["decided_by"]["kind"], json!("none_recorded"));
    assert_eq!(value["left_out"], json!({"superseded": 0, "rejected": 0}));
    assert_eq!(value["ranked_total"], json!(1));
    assert!(value.get("next_cursor").is_none());
    Ok(())
}

// ── Layer boundary ────────────────────────────────────────────────────────────

/// Importance is Layer 3: queries stay pure and the write layer stays dumb. Nothing in
/// `src/queries/` or `src/commands/` may reach for it.
#[test]
fn queries_and_commands_never_import_the_ranking() {
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
            !text.contains("use crate::importance") && !text.contains("crate::importance::"),
            "{} reaches for the importance ranking; it is Layer 3 and nothing below it may depend on it",
            path.display()
        );
    }
}

/// The ranking is a separate axis: it names none of the quality profile's seven dimensions and
/// returns no composite.
#[test]
fn the_ranking_never_touches_the_quality_profile() {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/importance.rs"))
            .expect("source file reads");
    assert!(!text.contains("use crate::quality_profile"));
    assert!(!text.contains("QualityProfile"));
}
