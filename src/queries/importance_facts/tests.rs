// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::queries::test_fixtures::{solid_assessment_dimensions, CountingGraph, Scenario};
use crate::queries::DecisionStatus;
use crate::Result;

use super::*;

const CREW: &str = "agent:claude:crew";
const ALEX: &str = "human:alex";

fn follows(s: &Scenario, follower: &str, premise: &str, at: &str) -> Result<()> {
    s.relation("FOLLOWS_FROM", follower, premise, CREW, None, at)
}

#[test]
fn an_empty_graph_has_no_decisions_and_nothing_following() -> Result<()> {
    let graph = Scenario::new().graph()?;

    let facts = get_importance_facts(&graph)?;

    assert!(facts.decisions.is_empty());
    assert!(facts.direct_dependents.is_empty());
    Ok(())
}

#[test]
fn a_decision_is_read_with_its_standing_deciders_and_recorder() -> Result<()> {
    let s = Scenario::new();
    s.decision(
        "d:taken",
        "Use the file ledger",
        CREW,
        "2026-01-01T00:00:00Z",
    )?;
    s.accept("d:taken", ALEX, "2026-01-02T00:00:00Z")?;
    s.decision("d:open", "Name the product", CREW, "2026-01-03T00:00:00Z")?;
    s.decision("d:no", "Skip the audit", CREW, "2026-01-04T00:00:00Z")?;
    s.reject("d:no", ALEX, "2026-01-05T00:00:00Z")?;
    let graph = s.graph()?;

    let facts = get_importance_facts(&graph)?;

    let taken = &facts.decisions["d:taken"];
    assert_eq!(taken.title, "Use the file ledger");
    assert_eq!(taken.status, DecisionStatus::Accepted);
    assert_eq!(
        taken.deciders,
        vec![Decider {
            id: ALEX.to_owned(),
            kind: "human"
        }]
    );
    assert_eq!(taken.recorded_by.as_deref(), Some(CREW));
    assert!(taken.model_judged.is_none());

    let open = &facts.decisions["d:open"];
    assert_eq!(open.status, DecisionStatus::Proposed);
    assert!(open.deciders.is_empty(), "the recorder is not a decider");
    assert_eq!(open.recorded_by.as_deref(), Some(CREW));

    let no = &facts.decisions["d:no"];
    assert_eq!(no.status, DecisionStatus::Rejected);
    assert_eq!(no.rejected_by, vec![ALEX.to_owned()]);
    Ok(())
}

#[test]
fn a_node_that_was_only_named_is_not_a_decision() -> Result<()> {
    let s = Scenario::new();
    s.decision("d:real", "A decision", CREW, "2026-01-01T00:00:00Z")?;
    s.request_naming("d:stub", "2026-01-02T00:00:00Z")?;
    let graph = s.graph()?;

    let facts = get_importance_facts(&graph)?;

    assert!(facts.decisions.contains_key("d:real"));
    assert!(!facts.decisions.contains_key("d:stub"));
    Ok(())
}

#[test]
fn direct_dependents_are_the_distinct_followers_of_each_premise_and_never_the_decision_itself(
) -> Result<()> {
    let s = Scenario::new();
    for (id, at) in [
        ("d:root", "2026-01-01T00:00:00Z"),
        ("d:a", "2026-01-02T00:00:00Z"),
        ("d:b", "2026-01-03T00:00:00Z"),
    ] {
        s.decision(id, id, CREW, at)?;
    }
    follows(&s, "d:b", "d:root", "2026-01-04T00:00:00Z")?;
    follows(&s, "d:a", "d:root", "2026-01-05T00:00:00Z")?;
    // The same link twice is one follower; a decision that follows itself rests on nothing.
    follows(&s, "d:a", "d:root", "2026-01-06T00:00:00Z")?;
    follows(&s, "d:root", "d:root", "2026-01-07T00:00:00Z")?;
    let graph = s.graph()?;

    let facts = get_importance_facts(&graph)?;

    assert_eq!(
        facts.direct_dependents["d:root"],
        vec!["d:a".to_owned(), "d:b".to_owned()]
    );
    assert!(!facts.direct_dependents.contains_key("d:a"));
    Ok(())
}

#[test]
fn what_a_model_judged_is_read_with_who_judged_it_and_never_invented() -> Result<()> {
    let s = Scenario::new();
    s.decision("d:judged", "Judged", CREW, "2026-01-01T00:00:00Z")?;
    s.decision("d:plain", "Plain", CREW, "2026-01-02T00:00:00Z")?;
    s.decision(
        "d:quality-only",
        "Assessed without importance",
        CREW,
        "2026-01-03T00:00:00Z",
    )?;
    s.assessment_with_importance(
        "d:judged",
        "claude-haiku-4-5",
        "assessment-v1",
        solid_assessment_dimensions("t"),
        (5.0, 0.8, 1.0),
        "2026-01-04T00:00:00Z",
    )?;
    s.assessment(
        "d:quality-only",
        "claude-haiku-4-5",
        "assessment-v1",
        solid_assessment_dimensions("t"),
        "2026-01-05T00:00:00Z",
    )?;
    let graph = s.graph()?;

    let facts = get_importance_facts(&graph)?;

    let judged = facts.decisions["d:judged"]
        .model_judged
        .as_ref()
        .expect("the model's numbers are read");
    assert_eq!(
        (judged.stakes, judged.irreversibility, judged.actionability),
        (5.0, 0.8, 1.0)
    );
    assert_eq!(judged.model.as_deref(), Some("claude-haiku-4-5"));
    assert_eq!(judged.prompt_version.as_deref(), Some("assessment-v1"));
    assert!(judged.assessed_at_origin.is_some());
    assert!(facts.decisions["d:plain"].model_judged.is_none());
    assert!(
        facts.decisions["d:quality-only"].model_judged.is_none(),
        "an assessment that judged no importance states none"
    );
    Ok(())
}

#[test]
fn the_reads_do_not_grow_with_the_number_of_decisions() -> Result<()> {
    let small = Scenario::new();
    small.decision("d:1", "One", CREW, "2026-01-01T00:00:00Z")?;
    let large = Scenario::new();
    for n in 0..12 {
        large.decision(
            &format!("d:{n}"),
            &format!("Decision {n}"),
            CREW,
            "2026-01-01T00:00:00Z",
        )?;
        if n > 0 {
            follows(&large, &format!("d:{n}"), "d:0", "2026-01-02T00:00:00Z")?;
        }
    }
    let (small, large) = (small.graph()?, large.graph()?);

    let counting_small = CountingGraph::new(&small);
    get_importance_facts(&counting_small)?;
    let counting_large = CountingGraph::new(&large);
    get_importance_facts(&counting_large)?;

    assert_eq!(counting_small.queries(), IMPORTANCE_FACT_READS);
    assert_eq!(counting_large.queries(), IMPORTANCE_FACT_READS);
    Ok(())
}
