// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::projector::RelationKind;
use crate::queries::test_fixtures::Scenario;
use crate::queries::{get_decision_neighborhood, NeighborEdge, NeighborhoodRequest};
use crate::Result;

use super::*;

fn edges_of(
    scenario: &Scenario,
    decision_id: &str,
    relation: RelationKind,
) -> Result<Vec<NeighborEdge>> {
    let graph = scenario.graph()?;
    let response = get_decision_neighborhood(&graph, decision_id, &NeighborhoodRequest::all())?;
    Ok(response
        .data
        .edges
        .into_iter()
        .filter(|edge| edge.relation == relation)
        .collect())
}

#[test]
fn neighborhood_turns_an_arrow_around_when_its_target_was_recorded_later() -> Result<()> {
    let scenario = Scenario::new();
    scenario.evidence("e:early", "Recorded first", None, "2026-01-01T00:00:00Z")?;
    scenario.decision_with(
        "d:1",
        "Pick a queue",
        "human:alex",
        "2026-01-02T00:00:00Z",
        false,
        &["e:early"],
        &[],
        None,
    )?;
    scenario.evidence("e:late", "Found afterwards", None, "2026-01-03T00:00:00Z")?;
    scenario.relation(
        "BASED_ON",
        "d:1",
        "e:late",
        "human:alex",
        None,
        "2026-01-03T00:00:01Z",
    )?;

    let edges = edges_of(&scenario, "d:1", RelationKind::BasedOn)?;
    assert_eq!(edges.len(), 2);

    let early = edges
        .iter()
        .find(|edge| edge.stored_target() == "e:early")
        .expect("the early evidence is in the neighborhood");
    assert!(!early.reversed);
    assert_eq!(
        (early.from.as_str(), early.to.as_str(), early.label),
        ("d:1", "e:early", "based on")
    );

    let late = edges
        .iter()
        .find(|edge| edge.stored_target() == "e:late")
        .expect("the late evidence is in the neighborhood");
    assert!(late.reversed);
    assert_eq!(
        (late.from.as_str(), late.to.as_str(), late.label),
        ("e:late", "d:1", "informs")
    );
    assert_eq!(late.stored_source(), "d:1");
    assert_eq!(late.relation, RelationKind::BasedOn);
    Ok(())
}

#[test]
fn neighborhood_reads_a_supersession_recorded_the_wrong_way_round_along_its_arrow() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:first",
        "Recorded first",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision(
        "d:second",
        "Recorded second",
        "human:alex",
        "2026-01-02T00:00:00Z",
    )?;
    // The decision recorded first is named as the superseder of the one recorded second.
    scenario.supersede("d:second", "d:first", "human:alex", "2026-01-03T00:00:00Z")?;

    for root in ["d:first", "d:second"] {
        let edges = edges_of(&scenario, root, RelationKind::Supersedes)?;
        assert_eq!(edges.len(), 1, "{root} sees the one supersession");
        let edge = &edges[0];
        assert!(edge.reversed);
        assert_eq!(
            (edge.from.as_str(), edge.to.as_str(), edge.label),
            ("d:second", "d:first", "is superseded by")
        );
        assert_eq!(edge.stored_source(), "d:first");
        assert_eq!(edge.stored_target(), "d:second");
    }
    Ok(())
}

#[test]
fn neighborhood_keeps_a_supersession_that_runs_the_ordinary_way() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:old",
        "Recorded first",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision(
        "d:new",
        "Recorded second",
        "human:alex",
        "2026-01-02T00:00:00Z",
    )?;
    scenario.supersede("d:old", "d:new", "human:alex", "2026-01-03T00:00:00Z")?;

    let edges = edges_of(&scenario, "d:old", RelationKind::Supersedes)?;
    assert_eq!(edges.len(), 1);
    let edge = &edges[0];
    assert!(!edge.reversed);
    assert_eq!(
        (edge.from.as_str(), edge.to.as_str(), edge.label),
        ("d:new", "d:old", "supersedes")
    );
    Ok(())
}

#[test]
fn oriented_edges_points_every_arrow_at_the_older_node() -> Result<()> {
    let scenario = Scenario::new();
    scenario.hypothesis(
        "h:1",
        "The queue keeps up",
        "assumption",
        None,
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision_with(
        "d:1",
        "Pick a queue",
        "human:alex",
        "2026-01-02T00:00:00Z",
        false,
        &[],
        &["h:1"],
        None,
    )?;
    scenario.evidence("e:1", "Load test", None, "2026-01-03T00:00:00Z")?;
    scenario.relation(
        "SUPPORTS",
        "e:1",
        "h:1",
        "human:alex",
        None,
        "2026-01-03T00:00:01Z",
    )?;

    let graph = scenario.graph()?;
    let arrows = oriented_edges(&graph)?;

    // The decision names the hypothesis it rests on, and the evidence supports it: both
    // arrows already point at the older hypothesis.
    let rests_on = arrows
        .iter()
        .find(|arrow| arrow.relation == RelationKind::PremisedOnDirect)
        .expect("the decision rests on the hypothesis");
    assert_eq!(
        (
            rests_on.from_id.as_str(),
            rests_on.to_id.as_str(),
            rests_on.label
        ),
        ("d:1", "h:1", "rests on")
    );
    let supports = arrows
        .iter()
        .find(|arrow| arrow.relation == RelationKind::Supports)
        .expect("the evidence supports the hypothesis");
    assert_eq!(
        (
            supports.from_id.as_str(),
            supports.to_id.as_str(),
            supports.label
        ),
        ("e:1", "h:1", "supports")
    );
    assert!(arrows.iter().all(|arrow| !arrow.reversed));
    Ok(())
}
