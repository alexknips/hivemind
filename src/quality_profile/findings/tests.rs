// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};

use crate::projector::GraphView;
use crate::queries::test_fixtures::{
    attention_scenario, ts, CountingGraph, Scenario, ATTENTION_NOW,
};
use crate::queries::GROUNDING_FACT_READS;
use crate::Result;

use super::*;

// ── helpers ───────────────────────────────────────────────────────────────────

fn now() -> DateTime<Utc> {
    ts(ATTENTION_NOW)
}

/// What a finding says, without its wording: decision, kind, nodes and basis time.
type Summary = (String, FindingKind, Vec<String>, Option<DateTime<Utc>>);

fn summary(finding: &AttentionFinding) -> Summary {
    (
        finding.decision_id.clone(),
        finding.kind,
        finding.node_ids.clone(),
        finding.basis_at,
    )
}

fn summaries(findings: &[AttentionFinding]) -> Vec<Summary> {
    findings.iter().map(summary).collect()
}

/// The findings of `attention_scenario` at `ATTENTION_NOW` with the default window, in the order
/// they come in: by decision id, then kind, then subject.
fn expected_scenario_findings() -> Vec<Summary> {
    use FindingKind::{
        AssumptionRefuted, BetFailed, BetPastCheckDate, EvidenceNotRechecked, PremiseRejected,
        PremiseSuperseded,
    };
    let table: [(&str, FindingKind, &[&str], Option<&str>); 13] = [
        (
            "d:assumption-refuted",
            AssumptionRefuted,
            &[
                "d:assumption-refuted",
                "e:refute-a2",
                "h:assumption-refuted",
            ],
            Some("2026-06-01T00:00:00Z"),
        ),
        (
            "d:bet-failed",
            BetFailed,
            &["d:bet-failed", "e:refute-bet", "h:bet-failed"],
            Some("2026-08-15T00:00:00Z"),
        ),
        (
            "d:bet-overdue",
            BetPastCheckDate,
            &["d:bet-overdue", "h:bet-overdue"],
            Some("2026-09-01T00:00:00Z"),
        ),
        (
            "d:bet-overdue-direct",
            BetPastCheckDate,
            &["d:bet-overdue-direct", "h:bet-overdue"],
            Some("2026-09-01T00:00:00Z"),
        ),
        (
            "d:cycle-a",
            PremiseSuperseded,
            &["d:cycle-a", "d:p-new", "d:p-old"],
            Some("2026-08-01T00:00:00Z"),
        ),
        (
            "d:ev-just-over",
            EvidenceNotRechecked,
            &["d:ev-just-over", "e:just-over"],
            Some("2026-06-27T23:59:59Z"),
        ),
        (
            "d:ev-stale",
            EvidenceNotRechecked,
            &["d:ev-stale", "e:old"],
            Some("2026-03-01T00:00:00Z"),
        ),
        (
            "d:ev-stale-two",
            EvidenceNotRechecked,
            &["d:ev-stale-two", "e:old"],
            Some("2026-03-01T00:00:00Z"),
        ),
        (
            "d:f-both",
            PremiseSuperseded,
            &["d:f-both", "d:p-new", "d:p-old"],
            Some("2026-08-01T00:00:00Z"),
        ),
        (
            "d:f-both",
            PremiseRejected,
            &["d:f-both", "d:p-rejected"],
            None,
        ),
        (
            "d:f-concurrent",
            PremiseSuperseded,
            &["d:f-concurrent", "d:p-double", "d:p-double-b"],
            Some("2026-08-05T00:00:00Z"),
        ),
        (
            "d:f-rejected",
            PremiseRejected,
            &["d:f-rejected", "d:p-rejected"],
            None,
        ),
        (
            "d:f-superseded",
            PremiseSuperseded,
            &["d:f-superseded", "d:p-new", "d:p-old"],
            Some("2026-08-01T00:00:00Z"),
        ),
    ];
    table
        .iter()
        .map(|(decision_id, kind, node_ids, basis_at)| {
            (
                (*decision_id).to_owned(),
                *kind,
                node_ids.iter().map(|id| (*id).to_owned()).collect(),
                basis_at.map(ts),
            )
        })
        .collect()
}

fn request(kinds: &[FindingKind], limit: usize) -> AttentionRequest {
    AttentionRequest {
        kinds: kinds.to_vec(),
        limit,
        cursor: None,
    }
}

/// Every finding, in one page.
fn all_findings(graph: &impl GraphView) -> Result<Vec<AttentionFinding>> {
    Ok(attention_findings_at(
        graph,
        &request(&[], MAX_QUERY_RESULTS),
        &AttentionConfig::default(),
        now(),
    )?
    .findings)
}

fn decisions_flagged(findings: &[AttentionFinding]) -> BTreeSet<&str> {
    findings
        .iter()
        .map(|finding| finding.decision_id.as_str())
        .collect()
}

/// Every page of `request` walked through its cursors, as (page sizes, all findings).
fn walk(
    graph: &impl GraphView,
    request: &AttentionRequest,
    config: &AttentionConfig,
    now: DateTime<Utc>,
) -> Result<(Vec<usize>, Vec<AttentionFinding>)> {
    let mut request = request.clone();
    let mut sizes = Vec::new();
    let mut findings = Vec::new();
    loop {
        let page = attention_findings_at(graph, &request, config, now)?;
        assert_eq!(page.next_cursor.is_some(), page.truncated);
        sizes.push(page.findings.len());
        findings.extend(page.findings);
        match page.next_cursor {
            Some(cursor) => request.cursor = Some(cursor),
            None => return Ok((sizes, findings)),
        }
        assert!(sizes.len() < 10_000, "the walk does not end");
    }
}

/// What every backend must read from `attention_scenario`: the same findings, in the same order,
/// whole or paged, and the same cost. Shared by the memory, Postgres and Kuzu tests.
pub(crate) fn assert_attention_scenario(graph: &impl GraphView) -> Result<()> {
    let expected = expected_scenario_findings();
    assert_eq!(summaries(&all_findings(graph)?), expected);

    let config = AttentionConfig::default();
    let (sizes, paged) = walk(graph, &request(&[], 4), &config, now())?;
    assert_eq!(sizes, [4, 4, 4, 1]);
    assert_eq!(summaries(&paged), expected);

    for kind in FindingKind::ALL {
        let of_kind = attention_findings_at(graph, &request(&[kind], 0), &config, now())?.findings;
        let want: Vec<&Summary> = expected.iter().filter(|s| s.1 == kind).collect();
        assert_eq!(
            summaries(&of_kind).iter().collect::<Vec<_>>(),
            want,
            "{kind:?}"
        );
    }
    Ok(())
}

// ── the lists ─────────────────────────────────────────────────────────────────

#[test]
fn every_list_holds_what_it_should_on_the_scenario() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    assert_attention_scenario(&graph)
}

#[test]
fn each_kind_finds_exactly_its_own_decisions() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let of = |kind: FindingKind| -> Result<Vec<String>> {
        Ok(attention_findings_at(
            &graph,
            &request(&[kind], 0),
            &AttentionConfig::default(),
            now(),
        )?
        .findings
        .into_iter()
        .map(|finding| finding.decision_id)
        .collect())
    };

    assert_eq!(
        of(FindingKind::BetPastCheckDate)?,
        ["d:bet-overdue", "d:bet-overdue-direct"]
    );
    assert_eq!(
        of(FindingKind::PremiseSuperseded)?,
        ["d:cycle-a", "d:f-both", "d:f-concurrent", "d:f-superseded"]
    );
    assert_eq!(
        of(FindingKind::PremiseRejected)?,
        ["d:f-both", "d:f-rejected"]
    );
    assert_eq!(
        of(FindingKind::AssumptionRefuted)?,
        ["d:assumption-refuted"]
    );
    assert_eq!(of(FindingKind::BetFailed)?, ["d:bet-failed"]);
    assert_eq!(
        of(FindingKind::EvidenceNotRechecked)?,
        ["d:ev-just-over", "d:ev-stale", "d:ev-stale-two"]
    );
    Ok(())
}

#[test]
fn a_bet_is_past_its_check_date_only_while_nothing_has_been_said_either_way() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let findings = all_findings(&graph)?;
    let bet_findings: BTreeSet<&str> = findings
        .iter()
        .filter(|finding| finding.kind == FindingKind::BetPastCheckDate)
        .map(|finding| finding.decision_id.as_str())
        .collect();

    // Past its date and open: flagged. Not yet due, undated and supported: not. Refuted: that is
    // a failed bet, not an unchecked one.
    assert_eq!(
        bet_findings,
        BTreeSet::from(["d:bet-overdue", "d:bet-overdue-direct"])
    );
    let flagged = decisions_flagged(&findings);
    for decision_id in [
        "d:bet-future",
        "d:bet-undated",
        "d:bet-held",
        "d:assumption-open",
    ] {
        assert!(!flagged.contains(decision_id), "{decision_id}");
    }
    Ok(())
}

#[test]
fn the_check_date_is_measured_against_the_clock_it_is_asked_at() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let bets_at = |at: DateTime<Utc>| -> Result<usize> {
        Ok(attention_findings_at(
            &graph,
            &request(&[FindingKind::BetPastCheckDate], 0),
            &AttentionConfig::default(),
            at,
        )?
        .findings
        .len())
    };

    assert_eq!(bets_at(ts("2026-08-31T23:59:59Z"))?, 0);
    // Not yet past: the date itself is the last moment it may still be checked.
    assert_eq!(bets_at(ts("2026-09-01T00:00:00Z"))?, 0);
    assert_eq!(bets_at(ts("2026-09-01T00:00:01Z"))?, 2);
    // One more bet falls due in December; the undated one never does.
    assert_eq!(bets_at(ts("2026-12-02T00:00:00Z"))?, 3);
    Ok(())
}

#[test]
fn a_decision_that_no_longer_stands_is_never_flagged() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let findings = all_findings(&graph)?;
    let flagged = decisions_flagged(&findings);

    // Each of these would be flagged if it stood: an overdue bet, a superseded premise or stale
    // evidence. Superseded and rejected decisions are done with.
    for decision_id in [
        "d:bet-dead",
        "d:bet-refused",
        "d:f-dead",
        "d:f-refused",
        "d:ev-rejected",
    ] {
        assert!(!flagged.contains(decision_id), "{decision_id}");
    }
    Ok(())
}

#[test]
fn a_contested_premise_is_a_disagreement_not_a_change() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let findings = all_findings(&graph)?;
    let flagged = decisions_flagged(&findings);

    assert!(!flagged.contains("d:f-contested"));
    assert!(!flagged.contains("d:f-standing"));
    Ok(())
}

#[test]
fn the_first_superseder_and_the_first_refutation_are_the_basis_not_the_lowest_id() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let findings = all_findings(&graph)?;
    let of = |decision_id: &str, kind: FindingKind| {
        findings
            .iter()
            .find(|finding| finding.decision_id == decision_id && finding.kind == kind)
            .expect("the finding exists")
    };

    // `d:p-double-a` sorts first but `d:p-double-b` was decided five days earlier.
    let superseded = of("d:f-concurrent", FindingKind::PremiseSuperseded);
    assert!(superseded.node_ids.contains(&"d:p-double-b".to_owned()));
    assert!(!superseded.node_ids.contains(&"d:p-double-a".to_owned()));
    assert_eq!(superseded.basis_at, Some(ts("2026-08-05T00:00:00Z")));
    // `e:refute-a1` sorts first but `e:refute-a2` was recorded a month earlier.
    let refuted = of("d:assumption-refuted", FindingKind::AssumptionRefuted);
    assert!(refuted.node_ids.contains(&"e:refute-a2".to_owned()));
    assert!(!refuted.node_ids.contains(&"e:refute-a1".to_owned()));
    assert_eq!(refuted.basis_at, Some(ts("2026-06-01T00:00:00Z")));
    Ok(())
}

#[test]
fn a_premise_that_was_rejected_has_no_recorded_time_and_says_so() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let page = attention_findings_at(
        &graph,
        &request(&[FindingKind::PremiseRejected], 0),
        &AttentionConfig::default(),
        now(),
    )?;

    assert_eq!(page.findings.len(), 2);
    for finding in &page.findings {
        assert_eq!(finding.basis_at, None);
        assert!(!finding.reason.contains(" on "), "{}", finding.reason);
    }
    Ok(())
}

#[test]
fn a_superseder_that_states_no_time_gives_a_finding_with_no_basis_time() -> Result<()> {
    let scenario = Scenario::new();
    let crew = "agent:claude:crew";
    scenario.decision(
        "d:premise",
        "Use the old queue",
        crew,
        "2026-02-01T00:00:00Z",
    )?;
    let proposal = scenario.decision("d:follower", "Batch jobs", crew, "2026-03-01T00:00:00Z")?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:follower",
        "d:premise",
        crew,
        Some(proposal),
        "2026-03-01T00:00:01Z",
    )?;
    // Named by a request and never proposed, so its node states no time.
    scenario.request_naming("d:stub-new", "2026-04-01T00:00:00Z")?;
    scenario.supersede("d:premise", "d:stub-new", crew, "2026-04-01T00:00:01Z")?;
    let graph = scenario.graph()?;

    let findings = all_findings(&graph)?;

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].kind, FindingKind::PremiseSuperseded);
    assert_eq!(findings[0].basis_at, None);
    assert_eq!(
        findings[0].node_ids,
        ["d:follower", "d:premise", "d:stub-new"]
    );
    assert_eq!(
        findings[0].reason,
        "the decision it follows from, d:premise, was superseded by d:stub-new"
    );
    Ok(())
}

#[test]
fn a_decision_that_superseded_its_own_premise_is_not_flagged_for_it() -> Result<()> {
    let scenario = Scenario::new();
    let crew = "agent:claude:crew";
    let follows = |decision_id: &str, premise_id: &str, timestamp: &str| -> Result<()> {
        scenario.relation(
            "FOLLOWS_FROM",
            decision_id,
            premise_id,
            crew,
            None,
            timestamp,
        )
    };
    scenario.decision("d:old", "Use the old queue", crew, "2026-02-01T00:00:00Z")?;
    scenario.decision(
        "d:refiner",
        "Use the old queue, tuned",
        crew,
        "2026-03-01T00:00:00Z",
    )?;
    scenario.decision("d:bystander", "Batch jobs", crew, "2026-03-02T00:00:00Z")?;
    follows("d:refiner", "d:old", "2026-03-01T00:00:01Z")?;
    follows("d:bystander", "d:old", "2026-03-02T00:00:01Z")?;
    scenario.supersede("d:old", "d:refiner", crew, "2026-03-01T00:00:02Z")?;
    // A premise superseded twice, once by the decision that follows it and once by another.
    scenario.decision("d:old2", "Use one region", crew, "2026-02-02T00:00:00Z")?;
    scenario.decision("d:both", "Use two regions", crew, "2026-03-10T00:00:00Z")?;
    scenario.decision("d:x", "Use three regions", crew, "2026-05-01T00:00:00Z")?;
    follows("d:both", "d:old2", "2026-03-10T00:00:01Z")?;
    scenario.supersede("d:old2", "d:both", crew, "2026-03-10T00:00:02Z")?;
    scenario.supersede("d:old2", "d:x", crew, "2026-05-01T00:00:01Z")?;
    let graph = scenario.graph()?;

    let findings = all_findings(&graph)?;

    // The refiner is the change. The bystander did not make it. `d:both` is told about the
    // other supersession, dated by that one and not by its own.
    assert_eq!(
        summaries(&findings),
        vec![
            (
                "d:both".to_owned(),
                FindingKind::PremiseSuperseded,
                vec!["d:both".to_owned(), "d:old2".to_owned(), "d:x".to_owned()],
                Some(ts("2026-05-01T00:00:00Z")),
            ),
            (
                "d:bystander".to_owned(),
                FindingKind::PremiseSuperseded,
                vec![
                    "d:bystander".to_owned(),
                    "d:old".to_owned(),
                    "d:refiner".to_owned()
                ],
                Some(ts("2026-03-01T00:00:00Z")),
            ),
        ]
    );
    Ok(())
}

#[test]
fn a_follows_from_cycle_is_read_once_and_never_loops() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let findings = all_findings(&graph)?;
    let of_cycle: Vec<&AttentionFinding> = findings
        .iter()
        .filter(|finding| finding.decision_id.starts_with("d:cycle"))
        .collect();

    // `d:cycle-a` follows a superseded premise as well; the two decisions that only follow each
    // other, and the one that follows itself, have nothing that changed.
    assert_eq!(of_cycle.len(), 1);
    assert_eq!(of_cycle[0].decision_id, "d:cycle-a");
    assert_eq!(of_cycle[0].kind, FindingKind::PremiseSuperseded);
    Ok(())
}

// ── evidence and its window ───────────────────────────────────────────────────

#[test]
fn evidence_is_not_re_checked_when_the_newest_item_linked_is_older_than_the_window() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let findings = all_findings(&graph)?;

    let stale: BTreeSet<&str> = findings
        .iter()
        .filter(|finding| finding.kind == FindingKind::EvidenceNotRechecked)
        .map(|finding| finding.decision_id.as_str())
        .collect();

    // 90 days before 2026-09-26 is exactly 2026-06-28T00:00:00Z: evidence recorded then
    // (`d:ev-boundary`) is not yet old, one second earlier (`d:ev-just-over`) is.
    assert_eq!(
        stale,
        BTreeSet::from(["d:ev-just-over", "d:ev-stale", "d:ev-stale-two"])
    );
    // The newest item is the basis, not the oldest.
    let two = findings
        .iter()
        .find(|finding| finding.decision_id == "d:ev-stale-two")
        .expect("flagged");
    assert_eq!(two.basis_at, Some(ts("2026-03-01T00:00:00Z")));
    assert!(two.node_ids.contains(&"e:old".to_owned()));
    // A recent item anywhere among the evidence, at capture or linked later, is a re-check.
    let flagged = decisions_flagged(&findings);
    for decision_id in [
        "d:ev-fresh",
        "d:ev-mixed",
        "d:ev-rechecked-later",
        "d:ev-none",
    ] {
        assert!(!flagged.contains(decision_id), "{decision_id}");
    }
    Ok(())
}

#[test]
fn the_window_is_configurable() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let stale_at = |days: u32| -> Result<BTreeSet<String>> {
        Ok(attention_findings_at(
            &graph,
            &request(&[FindingKind::EvidenceNotRechecked], 0),
            &AttentionConfig {
                evidence_window_days: days,
            },
            now(),
        )?
        .findings
        .into_iter()
        .map(|finding| finding.decision_id)
        .collect())
    };

    let quarter = stale_at(DEFAULT_EVIDENCE_WINDOW_DAYS)?;
    assert_eq!(quarter.len(), 3);
    // A month: the June notes and the boundary are old too, the dashboard readings are not.
    let month = stale_at(30)?;
    assert!(month.contains("d:ev-boundary"));
    assert!(!month.contains("d:ev-fresh"));
    assert!(month.is_superset(&quarter));
    // No window at all: everything cited before now is old.
    let none = stale_at(0)?;
    assert!(none.contains("d:ev-fresh"));
    assert!(none.contains("d:ev-mixed"));
    assert!(!none.contains("d:ev-none"));
    // A window longer than the record flags nothing, and cannot overflow the clock.
    assert!(stale_at(u32::MAX)?.is_empty());
    assert!(stale_at(365 * 10)?.is_empty());
    Ok(())
}

#[test]
fn a_page_says_which_clock_and_window_it_was_derived_with() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let page = attention_findings_at(
        &graph,
        &request(&[], 0),
        &AttentionConfig {
            evidence_window_days: 45,
        },
        now(),
    )?;

    assert_eq!(page.as_of, now());
    assert_eq!(page.evidence_window_days, 45);
    Ok(())
}

// ── identity ──────────────────────────────────────────────────────────────────

#[test]
fn a_finding_id_is_a_fixed_hash_of_kind_nodes_and_basis() {
    // Pinned: consumers dedupe on these across releases, so the input must not drift.
    assert_eq!(
        finding_id(
            FindingKind::BetPastCheckDate,
            &["d:1".to_owned(), "h:1".to_owned()],
            Some(ts("2026-09-01T00:00:00Z")),
        ),
        "finding-9759111b9669b66ccdf1b44757e6d2b6"
    );
    assert_eq!(
        finding_id(
            FindingKind::PremiseRejected,
            &["d:1".to_owned(), "d:2".to_owned()],
            None,
        ),
        "finding-95e5128ede038972a92a5e4f407c5f08"
    );
}

#[test]
fn a_finding_id_changes_with_the_kind_the_nodes_or_the_basis() {
    let nodes = ["d:1".to_owned(), "h:1".to_owned()];
    let at = Some(ts("2026-09-01T00:00:00Z"));
    let base = finding_id(FindingKind::BetPastCheckDate, &nodes, at);

    assert_eq!(base, finding_id(FindingKind::BetPastCheckDate, &nodes, at));
    assert_ne!(base, finding_id(FindingKind::BetFailed, &nodes, at));
    assert_ne!(
        base,
        finding_id(
            FindingKind::BetPastCheckDate,
            &["d:1".to_owned(), "h:2".to_owned()],
            at
        )
    );
    assert_ne!(
        base,
        finding_id(
            FindingKind::BetPastCheckDate,
            &nodes,
            Some(ts("2026-09-01T00:00:01Z"))
        )
    );
    assert_ne!(
        base,
        finding_id(FindingKind::BetPastCheckDate, &nodes, None)
    );
}

#[test]
fn ids_are_the_same_on_every_read_and_on_a_graph_rebuilt_from_the_ledger() -> Result<()> {
    let scenario = attention_scenario()?;
    let graph = scenario.graph()?;
    let rebuilt = scenario.graph()?;

    let first = all_findings(&graph)?;

    assert_eq!(first, all_findings(&graph)?);
    assert_eq!(first, all_findings(&rebuilt)?);
    assert_eq!(
        serde_json::to_string(&first).expect("serializes"),
        serde_json::to_string(&all_findings(&rebuilt)?).expect("serializes")
    );
    let ids: BTreeSet<&str> = first.iter().map(|f| f.finding_id.as_str()).collect();
    assert_eq!(ids.len(), first.len(), "every finding has its own id");
    Ok(())
}

#[test]
fn an_id_does_not_depend_on_the_clock_while_the_basis_holds() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let config = AttentionConfig::default();
    let at = |now: DateTime<Utc>| -> Result<Vec<AttentionFinding>> {
        Ok(attention_findings_at(&graph, &request(&[], MAX_QUERY_RESULTS), &config, now)?.findings)
    };

    let today = at(now())?;
    let next_week = at(now() + Duration::days(7))?;

    for finding in &today {
        let later = next_week
            .iter()
            .find(|later| later.decision_id == finding.decision_id && later.kind == finding.kind)
            .expect("still flagged a week on");
        assert_eq!(
            later.finding_id, finding.finding_id,
            "{}",
            finding.decision_id
        );
    }
    Ok(())
}

#[test]
fn newer_evidence_gives_the_finding_a_new_id_and_a_newer_basis() -> Result<()> {
    let scenario = attention_scenario()?;
    let before = all_findings(&scenario.graph()?)?;
    // Another benchmark, still outside the window, is linked to `d:ev-stale` afterwards.
    scenario.relation(
        "BASED_ON",
        "d:ev-stale",
        "e:just-over",
        "human:alex",
        None,
        "2026-09-20T00:00:00Z",
    )?;

    let after = all_findings(&scenario.graph()?)?;
    let stale = |findings: &[AttentionFinding]| -> AttentionFinding {
        findings
            .iter()
            .find(|finding| finding.decision_id == "d:ev-stale")
            .expect("flagged")
            .clone()
    };

    let (old, new) = (stale(&before), stale(&after));
    assert_ne!(old.finding_id, new.finding_id);
    assert_eq!(old.basis_at, Some(ts("2026-03-01T00:00:00Z")));
    assert_eq!(new.basis_at, Some(ts("2026-06-27T23:59:59Z")));
    // Everything else stands as it was, ids included.
    let others = |findings: &[AttentionFinding]| -> Vec<AttentionFinding> {
        findings
            .iter()
            .filter(|finding| finding.decision_id != "d:ev-stale")
            .cloned()
            .collect()
    };
    assert_eq!(others(&before), others(&after));
    Ok(())
}

#[test]
fn an_earlier_superseder_moves_the_basis_and_the_id_of_a_premise_finding() -> Result<()> {
    let scenario = attention_scenario()?;
    let before = all_findings(&scenario.graph()?)?;
    scenario.decision(
        "d:p-older-new",
        "A queue nobody remembers",
        "agent:claude:crew",
        "2026-07-01T00:00:00Z",
    )?;
    scenario.supersede(
        "d:p-old",
        "d:p-older-new",
        "agent:claude:crew",
        "2026-09-21T00:00:00Z",
    )?;

    let after = all_findings(&scenario.graph()?)?;
    let of = |findings: &[AttentionFinding]| -> AttentionFinding {
        findings
            .iter()
            .find(|finding| {
                finding.decision_id == "d:f-superseded"
                    && finding.kind == FindingKind::PremiseSuperseded
            })
            .expect("flagged")
            .clone()
    };

    let (old, new) = (of(&before), of(&after));
    assert_eq!(old.basis_at, Some(ts("2026-08-01T00:00:00Z")));
    assert_eq!(new.basis_at, Some(ts("2026-07-01T00:00:00Z")));
    assert!(new.node_ids.contains(&"d:p-older-new".to_owned()));
    assert_ne!(old.finding_id, new.finding_id);
    Ok(())
}

// ── paging ────────────────────────────────────────────────────────────────────

#[test]
fn a_page_that_is_cut_short_says_so_and_says_where_to_resume() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let config = AttentionConfig::default();

    let first = attention_findings_at(&graph, &request(&[], 5), &config, now())?;
    assert_eq!(first.findings.len(), 5);
    assert!(first.truncated);
    let cursor = first
        .next_cursor
        .clone()
        .expect("a cut-short page has a cursor");

    let second = attention_findings_at(
        &graph,
        &AttentionRequest {
            cursor: Some(cursor),
            ..request(&[], 5)
        },
        &config,
        now(),
    )?;

    let expected = expected_scenario_findings();
    assert_eq!(summaries(&first.findings), expected[..5]);
    assert_eq!(summaries(&second.findings), expected[5..10]);
    assert!(second.truncated);
    Ok(())
}

#[test]
fn a_page_that_holds_everything_left_is_not_truncated() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let config = AttentionConfig::default();

    let exact = attention_findings_at(&graph, &request(&[], 13), &config, now())?;
    let one_short = attention_findings_at(&graph, &request(&[], 12), &config, now())?;

    assert_eq!(exact.findings.len(), 13);
    assert!(!exact.truncated);
    assert_eq!(exact.next_cursor, None);
    assert_eq!(one_short.findings.len(), 12);
    assert!(one_short.truncated);
    assert!(one_short.next_cursor.is_some());
    Ok(())
}

#[test]
fn walking_the_cursors_visits_every_finding_once_whatever_the_page_size() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let expected = expected_scenario_findings();

    for limit in [1, 2, 3, 5, 7, 12, 13, 14, 1000] {
        let (sizes, findings) = walk(
            &graph,
            &request(&[], limit),
            &AttentionConfig::default(),
            now(),
        )?;
        assert_eq!(summaries(&findings), expected, "limit {limit}");
        assert!(sizes.iter().all(|size| *size <= limit), "limit {limit}");
    }
    Ok(())
}

/// `count` decisions, each resting on its own bet whose check date has passed.
fn overdue_bet_scenario(count: usize) -> Result<Scenario> {
    let scenario = Scenario::new();
    for index in 0..count {
        let hypothesis_id = format!("h:overdue-{index:05}");
        scenario.hypothesis(
            &hypothesis_id,
            "A bet",
            "bet",
            Some("2026-09-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        )?;
        scenario.decision_with(
            &format!("d:overdue-{index:05}"),
            "Overdue decision",
            "agent:claude:crew",
            "2026-02-01T00:00:00Z",
            true,
            &[],
            &[hypothesis_id.as_str()],
            None,
        )?;
    }
    Ok(scenario)
}

#[test]
fn a_limit_of_zero_is_the_default_page_and_a_huge_one_is_capped() -> Result<()> {
    let graph = overdue_bet_scenario(MAX_QUERY_RESULTS + 100)?.graph()?;
    let config = AttentionConfig::default();

    let default = attention_findings_at(&graph, &request(&[], 0), &config, now())?;
    let capped = attention_findings_at(&graph, &request(&[], usize::MAX), &config, now())?;

    assert_eq!(default.findings.len(), DEFAULT_PAGE_SIZE);
    assert!(default.truncated);
    // Never more than `MAX_QUERY_RESULTS`, and the rest is still reachable.
    assert_eq!(capped.findings.len(), MAX_QUERY_RESULTS);
    assert!(capped.truncated);
    assert!(capped.next_cursor.is_some());
    Ok(())
}

#[test]
fn the_cursor_is_a_position_so_findings_that_appear_earlier_do_not_shift_the_next_page(
) -> Result<()> {
    let scenario = attention_scenario()?;
    let config = AttentionConfig::default();
    let before = scenario.graph()?;
    let first = attention_findings_at(&before, &request(&[], 5), &config, now())?;
    let cursor = first.next_cursor.clone().expect("cut short");
    let second_before = attention_findings_at(
        &before,
        &AttentionRequest {
            cursor: Some(cursor.clone()),
            ..request(&[], 5)
        },
        &config,
        now(),
    )?;

    // Two decisions that sort before everything else come into attention between the pages.
    scenario.hypothesis(
        "h:early",
        "Early bet",
        "bet",
        Some("2026-09-02T00:00:00Z"),
        "2026-09-22T00:00:00Z",
    )?;
    for decision_id in ["d:a-1", "d:a-2"] {
        scenario.decision_with(
            decision_id,
            "Early decision",
            "agent:claude:crew",
            "2026-09-22T00:00:01Z",
            true,
            &[],
            &["h:early"],
            None,
        )?;
    }
    let after = scenario.graph()?;
    let second_after = attention_findings_at(
        &after,
        &AttentionRequest {
            cursor: Some(cursor),
            ..request(&[], 5)
        },
        &config,
        now(),
    )?;
    let fresh = attention_findings_at(&after, &request(&[], 2), &config, now())?;

    assert_eq!(second_after.findings, second_before.findings);
    assert_eq!(
        fresh
            .findings
            .iter()
            .map(|finding| finding.decision_id.as_str())
            .collect::<Vec<_>>(),
        ["d:a-1", "d:a-2"]
    );
    Ok(())
}

#[test]
fn a_cursor_an_attention_scan_did_not_return_is_refused() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    for cursor in [
        "5",
        "not json",
        "",
        r#"{"decision_id":"d:1","kind":"no_such_kind","subject_id":"x"}"#,
        r#"{"decision_id":"d:1"}"#,
    ] {
        let result = attention_findings_at(
            &graph,
            &AttentionRequest {
                cursor: Some(cursor.to_owned()),
                ..request(&[], 5)
            },
            &AttentionConfig::default(),
            now(),
        );
        assert!(result.is_err(), "{cursor:?}");
    }
    Ok(())
}

#[test]
fn an_empty_graph_has_no_findings() -> Result<()> {
    let graph = Scenario::new().graph()?;

    let page = attention_findings_at(&graph, &request(&[], 0), &AttentionConfig::default(), now())?;

    assert!(page.findings.is_empty());
    assert!(!page.truncated);
    assert_eq!(page.next_cursor, None);
    Ok(())
}

// ── shape ─────────────────────────────────────────────────────────────────────

#[test]
fn every_finding_lists_its_decision_and_sorted_distinct_node_ids() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    for finding in all_findings(&graph)? {
        assert!(finding.node_ids.contains(&finding.decision_id));
        let mut sorted = finding.node_ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(finding.node_ids, sorted, "{}", finding.decision_id);
        assert!(finding.finding_id.starts_with("finding-"));
        assert_eq!(finding.finding_id.len(), "finding-".len() + 32);
        assert!(!finding.reason.is_empty());
    }
    Ok(())
}

#[test]
fn each_finding_says_in_words_what_changed_or_lapsed() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let findings = all_findings(&graph)?;
    let reason_of = |decision_id: &str, kind: FindingKind| {
        findings
            .iter()
            .find(|finding| finding.decision_id == decision_id && finding.kind == kind)
            .map(|finding| finding.reason.as_str())
            .expect("the finding exists")
    };

    assert_eq!(
        reason_of("d:bet-overdue", FindingKind::BetPastCheckDate),
        "the bet h:bet-overdue was to be checked by 2026-09-01; nothing has been recorded for or against it"
    );
    assert_eq!(
        reason_of("d:f-superseded", FindingKind::PremiseSuperseded),
        "the decision it follows from, d:p-old, was superseded by d:p-new on 2026-08-01"
    );
    assert_eq!(
        reason_of("d:f-rejected", FindingKind::PremiseRejected),
        "the decision it follows from, d:p-rejected, was rejected"
    );
    assert_eq!(
        reason_of("d:assumption-refuted", FindingKind::AssumptionRefuted),
        "the assumption it rests on, h:assumption-refuted, was refuted by e:refute-a2 on 2026-06-01"
    );
    assert_eq!(
        reason_of("d:bet-failed", FindingKind::BetFailed),
        "the bet it rests on, h:bet-failed, failed: refuted by e:refute-bet on 2026-08-15"
    );
    assert_eq!(
        reason_of("d:ev-stale", FindingKind::EvidenceNotRechecked),
        "the newest evidence it rests on, e:old, was recorded on 2026-03-01, more than 90 days ago"
    );
    Ok(())
}

#[test]
fn the_wire_form_names_kinds_in_snake_case_and_omits_what_is_absent() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let page = attention_findings_at(
        &graph,
        &request(&[FindingKind::PremiseRejected], 0),
        &AttentionConfig::default(),
        now(),
    )?;
    let wire = serde_json::to_value(&page).expect("page serializes");

    assert_eq!(wire["truncated"], false);
    assert!(wire.get("next_cursor").is_none());
    assert_eq!(wire["evidence_window_days"], 90);
    let first = &wire["findings"][0];
    assert_eq!(first["kind"], "premise_rejected");
    assert!(first.get("basis_at").is_none());
    assert!(first["finding_id"].as_str().is_some());
    // No grade of any kind.
    for word in ["score", "tier", "rank", "grade"] {
        assert!(first.get(word).is_none(), "{word}");
    }
    Ok(())
}

#[test]
fn kinds_go_by_their_wire_names() {
    let names: Vec<&str> = FindingKind::ALL.iter().map(|kind| kind.as_str()).collect();

    assert_eq!(
        names,
        [
            "bet_past_check_date",
            "premise_superseded",
            "premise_rejected",
            "assumption_refuted",
            "bet_failed",
            "evidence_not_rechecked"
        ]
    );
    for kind in FindingKind::ALL {
        assert_eq!(
            serde_json::to_value(kind).expect("serializes"),
            kind.as_str()
        );
    }
    let mut sorted = FindingKind::ALL;
    sorted.sort();
    assert_eq!(
        sorted,
        FindingKind::ALL,
        "declaration order is the sort order"
    );
}

// ── cost ──────────────────────────────────────────────────────────────────────

/// `decisions` decisions, each resting on a bet (every fifth is overdue), plus a tenth as many
/// decisions following a superseded premise and citing stale evidence. Findings: 2/5 of
/// `decisions`.
fn bulk_scenario(decisions: usize) -> Result<Scenario> {
    let scenario = Scenario::new();
    let crew = "agent:claude:crew";
    for index in 0..decisions {
        let check_by = if index % 5 == 0 {
            "2026-09-01T00:00:00Z"
        } else {
            "2026-12-01T00:00:00Z"
        };
        let hypothesis_id = format!("h:bulk-{index:05}");
        scenario.hypothesis(
            &hypothesis_id,
            "A bet",
            "bet",
            Some(check_by),
            "2026-01-01T00:00:00Z",
        )?;
        scenario.decision_with(
            &format!("d:bulk-{index:05}"),
            "Bulk decision",
            crew,
            "2026-02-01T00:00:00Z",
            true,
            &[],
            &[hypothesis_id.as_str()],
            None,
        )?;
    }
    for index in 0..decisions / 10 {
        let (old, new, dependent) = (
            format!("d:old-{index:05}"),
            format!("d:new-{index:05}"),
            format!("d:dep-{index:05}"),
        );
        scenario.decision(&old, "Old premise", crew, "2026-02-01T00:00:00Z")?;
        scenario.decision(&new, "New premise", crew, "2026-08-01T00:00:00Z")?;
        scenario.supersede(&old, &new, crew, "2026-08-01T00:00:01Z")?;
        let proposal = scenario.decision(&dependent, "Dependent", crew, "2026-04-01T00:00:00Z")?;
        scenario.relation(
            "FOLLOWS_FROM",
            &dependent,
            &old,
            crew,
            Some(proposal),
            "2026-04-01T00:00:01Z",
        )?;

        let evidence_id = format!("e:stale-{index:05}");
        scenario.evidence(&evidence_id, "Old reading", None, "2026-03-01T00:00:00Z")?;
        scenario.decision_with(
            &format!("d:ev-{index:05}"),
            "Cites old evidence",
            crew,
            "2026-04-02T00:00:00Z",
            false,
            &[evidence_id.as_str()],
            &[],
            None,
        )?;
    }
    Ok(scenario)
}

#[test]
fn a_page_costs_a_fixed_number_of_bulk_reads_however_many_decisions_there_are() -> Result<()> {
    let config = AttentionConfig::default();
    let reads = |decisions: usize, kinds: &[FindingKind], limit: usize| -> Result<usize> {
        let graph = bulk_scenario(decisions)?.graph()?;
        let counting = CountingGraph::new(&graph);
        let page = attention_findings_at(&counting, &request(kinds, limit), &config, now())?;
        assert_eq!(page.findings.len(), limit);
        Ok(counting.queries())
    };

    // Bets and stale evidence need no anchored read: the same twelve reads for 50 decisions and
    // for 800.
    for kinds in [
        &[FindingKind::BetPastCheckDate][..],
        &[FindingKind::EvidenceNotRechecked][..],
    ] {
        assert_eq!(reads(50, kinds, 5)?, GROUNDING_FACT_READS);
        assert_eq!(reads(800, kinds, 5)?, GROUNDING_FACT_READS);
    }
    Ok(())
}

#[test]
fn a_page_of_superseded_premises_adds_one_anchored_read_per_distinct_superseder() -> Result<()> {
    let graph = bulk_scenario(800)?.graph()?;
    let counting = CountingGraph::new(&graph);

    let page = attention_findings_at(
        &counting,
        &request(&[FindingKind::PremiseSuperseded], 25),
        &AttentionConfig::default(),
        now(),
    )?;

    assert_eq!(page.findings.len(), 25);
    assert!(page.truncated);
    // Each dependent has its own premise and superseder here, so 25 findings are 25 lookups.
    assert_eq!(counting.queries(), GROUNDING_FACT_READS + 25);
    Ok(())
}

#[test]
fn a_large_graph_pages_through_every_finding_once_within_the_stated_cost() -> Result<()> {
    let decisions = 800;
    let graph = bulk_scenario(decisions)?.graph()?;
    let config = AttentionConfig::default();
    let limit = 60;

    let mut cursor = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let counting = CountingGraph::new(&graph);
        let page = attention_findings_at(
            &counting,
            &AttentionRequest {
                kinds: Vec::new(),
                limit,
                cursor: cursor.take(),
            },
            &config,
            now(),
        )?;
        assert!(page.findings.len() <= limit);
        assert!(counting.queries() <= GROUNDING_FACT_READS + limit);
        seen.extend(page.findings);
        pages += 1;
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    // 160 overdue bets, 80 superseded premises, 80 stale evidence items.
    assert_eq!(seen.len(), decisions * 2 / 5);
    assert_eq!(pages, seen.len().div_ceil(limit));
    let ids: BTreeSet<&str> = seen.iter().map(|f| f.finding_id.as_str()).collect();
    assert_eq!(ids.len(), seen.len(), "no finding twice");
    let one_page = all_findings(&graph)?;
    assert_eq!(seen, one_page);
    Ok(())
}
