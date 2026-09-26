// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::collections::BTreeSet;

use crate::events::HypothesisKind;
use crate::projector::memory::MemoryGraph;
use crate::queries::test_fixtures::{attention_scenario, ts, CountingGraph, Scenario};
use crate::queries::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
use crate::Result;

use super::*;

fn pairs(links: &[(String, String)]) -> BTreeSet<(&str, &str)> {
    links
        .iter()
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect()
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn scenario_facts() -> Result<(MemoryGraph, GroundingFacts)> {
    let graph = attention_scenario()?.graph()?;
    let facts = get_grounding_facts(&graph)?;
    Ok((graph, facts))
}

#[test]
fn an_empty_graph_has_no_grounding() -> Result<()> {
    let graph = Scenario::new().graph()?;

    let facts = get_grounding_facts(&graph)?;

    assert!(facts.premise_links.is_empty());
    assert!(facts.evidence_links.is_empty());
    assert!(facts.hypothesis_links.is_empty());
    assert!(facts.hypotheses.is_empty());
    assert!(facts.evidence_recorded_at.is_empty());
    Ok(())
}

#[test]
fn premise_links_are_every_follows_from_edge_including_cycles() -> Result<()> {
    let (_, facts) = scenario_facts()?;

    let links = pairs(&facts.premise_links);

    for link in [
        ("d:f-superseded", "d:p-old"),
        ("d:f-both", "d:p-old"),
        ("d:f-both", "d:p-rejected"),
        ("d:cycle-a", "d:cycle-b"),
        ("d:cycle-b", "d:cycle-a"),
        ("d:cycle-self", "d:cycle-self"),
    ] {
        assert!(links.contains(&link), "{link:?}");
    }
    assert!(!links.contains(&("d:p-old", "d:f-superseded")));
    let mut sorted = facts.premise_links.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(facts.premise_links, sorted);
    Ok(())
}

#[test]
fn a_hypothesis_is_a_decisions_whether_it_hangs_off_the_chosen_option_or_the_decision() -> Result<()>
{
    let (_, facts) = scenario_facts()?;

    let links = pairs(&facts.hypothesis_links);

    // Named at capture through the chosen option, and attributed directly.
    assert!(links.contains(&("d:bet-overdue", "h:bet-overdue")));
    assert!(links.contains(&("d:bet-overdue-direct", "h:bet-overdue")));
    // A decision that chose nothing has no path through an option.
    assert!(!links.iter().any(|(decision, _)| *decision == "d:ev-none"));
    Ok(())
}

#[test]
fn evidence_links_include_evidence_attached_after_capture() -> Result<()> {
    let (_, facts) = scenario_facts()?;

    let links = pairs(&facts.evidence_links);

    assert!(links.contains(&("d:ev-stale-two", "e:old")));
    assert!(links.contains(&("d:ev-stale-two", "e:very-old")));
    assert!(links.contains(&("d:ev-rechecked-later", "e:very-old")));
    assert!(links.contains(&("d:ev-rechecked-later", "e:newer")));
    Ok(())
}

#[test]
fn a_hypothesis_carries_its_kind_check_date_status_and_refuting_evidence() -> Result<()> {
    let (_, facts) = scenario_facts()?;
    let hypothesis = |id: &str| {
        facts
            .hypotheses
            .get(id)
            .expect("hypothesis is in the graph")
    };

    let overdue = hypothesis("h:bet-overdue");
    assert_eq!(overdue.kind, HypothesisKind::Bet);
    assert_eq!(overdue.check_by, Some(ts("2026-09-01T00:00:00Z")));
    assert_eq!(overdue.status, HypothesisStatus::Open);
    assert!(overdue.refuted_by.is_empty());

    assert_eq!(hypothesis("h:bet-undated").check_by, None);
    assert_eq!(hypothesis("h:bet-held").status, HypothesisStatus::Supported);

    let failed = hypothesis("h:bet-failed");
    assert_eq!(failed.status, HypothesisStatus::Refuted);
    assert_eq!(failed.refuted_by, ids(&["e:refute-bet"]));

    let assumption = hypothesis("h:assumption-refuted");
    assert_eq!(assumption.kind, HypothesisKind::Assumption);
    assert_eq!(assumption.status, HypothesisStatus::Refuted);
    // Sorted by id, not by when they were recorded.
    assert_eq!(assumption.refuted_by, ids(&["e:refute-a1", "e:refute-a2"]));
    Ok(())
}

#[test]
fn the_bulk_status_of_every_hypothesis_is_the_status_the_single_read_gives() -> Result<()> {
    let (graph, facts) = scenario_facts()?;

    for (hypothesis_id, record) in &facts.hypotheses {
        assert_eq!(
            record.status,
            derive_hypothesis_status(&graph, hypothesis_id)?,
            "{hypothesis_id}"
        );
    }
    assert_eq!(facts.hypotheses.len(), 7);
    Ok(())
}

#[test]
fn the_standings_are_the_statuses_the_single_read_gives_and_name_every_superseder() -> Result<()> {
    let (graph, facts) = scenario_facts()?;

    let decisions: BTreeSet<&str> = facts
        .premise_links
        .iter()
        .flat_map(|(decision, premise)| [decision.as_str(), premise.as_str()])
        .chain([
            "d:bet-dead",
            "d:bet-refused",
            "d:ev-rejected",
            "d:bet-overdue",
        ])
        .collect();
    for decision_id in decisions {
        assert_eq!(
            facts.standings.status_of(decision_id),
            derive_decision_status(&graph, decision_id)?,
            "{decision_id}"
        );
    }
    assert_eq!(
        facts.standings.status_of("d:p-old"),
        DecisionStatus::Superseded
    );
    assert_eq!(
        facts.standings.status_of("d:p-contested"),
        DecisionStatus::Contested
    );
    assert_eq!(
        facts.standings.superseders_of("d:p-double"),
        ["d:p-double-a", "d:p-double-b"]
    );
    assert!(facts.standings.superseders_of("d:p-standing").is_empty());
    Ok(())
}

#[test]
fn the_same_link_asserted_twice_is_one_link() -> Result<()> {
    let scenario = Scenario::new();
    let crew = "agent:claude:crew";
    scenario.decision("d:premise", "Premise", crew, "2026-01-01T00:00:00Z")?;
    scenario.decision("d:newer", "Newer", crew, "2026-01-02T00:00:00Z")?;
    scenario.decision("d:follower", "Follower", crew, "2026-01-03T00:00:00Z")?;
    scenario.evidence("e:1", "A reading", None, "2026-01-04T00:00:00Z")?;
    scenario.hypothesis("h:1", "A claim", "assumption", None, "2026-01-05T00:00:00Z")?;
    for round in 0..2 {
        let at = format!("2026-02-0{}T00:00:00Z", round + 1);
        scenario.relation("FOLLOWS_FROM", "d:follower", "d:premise", crew, None, &at)?;
        scenario.relation("BASED_ON", "d:follower", "e:1", crew, None, &at)?;
        scenario.relation("ASSUMES", "d:follower", "h:1", crew, None, &at)?;
        scenario.relation("REFUTES", "e:1", "h:1", crew, None, &at)?;
        scenario.supersede("d:premise", "d:newer", crew, &at)?;
    }
    let graph = scenario.graph()?;

    let facts = get_grounding_facts(&graph)?;

    assert_eq!(
        pairs(&facts.premise_links),
        BTreeSet::from([("d:follower", "d:premise")])
    );
    assert_eq!(facts.premise_links.len(), 1);
    assert_eq!(facts.evidence_links.len(), 1);
    assert_eq!(facts.hypothesis_links.len(), 1);
    assert_eq!(
        facts
            .hypotheses
            .get("h:1")
            .map(|record| record.refuted_by.clone()),
        Some(ids(&["e:1"]))
    );
    assert_eq!(facts.standings.superseders_of("d:premise"), ["d:newer"]);
    Ok(())
}

#[test]
fn evidence_carries_when_it_was_recorded() -> Result<()> {
    let (_, facts) = scenario_facts()?;

    assert_eq!(
        facts.evidence_recorded_at.get("e:old"),
        Some(&ts("2026-03-01T00:00:00Z"))
    );
    assert_eq!(
        facts.evidence_recorded_at.get("e:just-over"),
        Some(&ts("2026-06-27T23:59:59Z"))
    );
    assert_eq!(facts.evidence_recorded_at.len(), 10);
    Ok(())
}

#[test]
fn the_facts_cost_a_fixed_number_of_reads_however_large_the_graph() -> Result<()> {
    let small = Scenario::new();
    small.decision("d:1", "One", "agent:tester", "2026-01-01T00:00:00Z")?;
    let small = small.graph()?;
    let large = attention_scenario()?.graph()?;

    let counting_small = CountingGraph::new(&small);
    get_grounding_facts(&counting_small)?;
    let counting_large = CountingGraph::new(&large);
    get_grounding_facts(&counting_large)?;

    assert_eq!(counting_small.queries(), GROUNDING_FACT_READS);
    assert_eq!(counting_large.queries(), GROUNDING_FACT_READS);
    Ok(())
}

#[test]
fn decision_times_are_anchored_reads_and_skip_what_states_no_time() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision("d:1", "One", "agent:tester", "2026-01-01T00:00:00Z")?;
    scenario.decision("d:2", "Two", "agent:tester", "2026-02-02T00:00:00Z")?;
    scenario.request_naming("d:stub", "2026-03-03T00:00:00Z")?;
    let graph = scenario.graph()?;
    let counting = CountingGraph::new(&graph);

    let times = get_decision_times(&counting, ["d:1", "d:2", "d:2", "d:stub", "d:missing"])?;

    assert_eq!(times.len(), 2);
    assert_eq!(times.get("d:1"), Some(&ts("2026-01-01T00:00:00Z")));
    assert_eq!(times.get("d:2"), Some(&ts("2026-02-02T00:00:00Z")));
    // One lookup per distinct id, never a scan.
    assert_eq!(counting.queries(), 4);
    Ok(())
}
