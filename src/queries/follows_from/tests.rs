// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::projector::{memory::MemoryGraph, GraphProperties, GraphView, RelationKind};

use super::follows_from_path;

/// A graph whose `FOLLOWS_FROM` edges are `edges`, each `(decision, premise)`.
fn graph_with(edges: &[(&str, &str)]) -> MemoryGraph {
    let graph = MemoryGraph::default();
    for (decision, premise) in edges {
        graph
            .upsert_edge(
                RelationKind::FollowsFrom,
                decision,
                premise,
                &GraphProperties::new(),
            )
            .expect("edge is recorded");
    }
    graph
}

fn path(graph: &MemoryGraph, from: &str, to: &str) -> Option<Vec<String>> {
    follows_from_path(graph, from, to).expect("walk succeeds")
}

fn ids(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|item| (*item).to_owned()).collect())
}

#[test]
fn a_decision_reaches_itself_in_one_element() {
    let graph = graph_with(&[]);
    assert_eq!(path(&graph, "d-1", "d-1"), ids(&["d-1"]));
}

#[test]
fn a_direct_premise_is_a_two_element_chain() {
    let graph = graph_with(&[("d-1", "d-2")]);
    assert_eq!(path(&graph, "d-1", "d-2"), ids(&["d-1", "d-2"]));
}

#[test]
fn the_chain_runs_from_the_decision_through_every_premise_in_between() {
    let graph = graph_with(&[("d-1", "d-2"), ("d-2", "d-3"), ("d-3", "d-4")]);
    assert_eq!(
        path(&graph, "d-1", "d-4"),
        ids(&["d-1", "d-2", "d-3", "d-4"])
    );
}

#[test]
fn the_direction_matters_a_premise_does_not_rest_on_what_rests_on_it() {
    let graph = graph_with(&[("d-1", "d-2")]);
    assert_eq!(path(&graph, "d-2", "d-1"), None);
}

#[test]
fn unrelated_decisions_have_no_chain() {
    let graph = graph_with(&[("d-1", "d-2"), ("d-3", "d-4")]);
    assert_eq!(path(&graph, "d-1", "d-4"), None);
}

#[test]
fn only_follows_from_edges_count() {
    let graph = graph_with(&[]);
    graph
        .upsert_edge(
            RelationKind::Supersedes,
            "d-1",
            "d-2",
            &GraphProperties::new(),
        )
        .expect("edge is recorded");
    assert_eq!(path(&graph, "d-1", "d-2"), None);
}

#[test]
fn the_shortest_chain_wins_and_ties_break_by_id() {
    // d-1 reaches d-9 the long way round via d-2 -> d-3 and the short way via d-4 and d-5.
    let graph = graph_with(&[
        ("d-1", "d-2"),
        ("d-2", "d-3"),
        ("d-3", "d-9"),
        ("d-1", "d-5"),
        ("d-1", "d-4"),
        ("d-4", "d-9"),
        ("d-5", "d-9"),
    ]);
    assert_eq!(path(&graph, "d-1", "d-9"), ids(&["d-1", "d-4", "d-9"]));
}

#[test]
fn a_cycle_already_in_the_graph_terminates() {
    let graph = graph_with(&[("d-1", "d-2"), ("d-2", "d-3"), ("d-3", "d-1")]);
    assert_eq!(path(&graph, "d-1", "d-9"), None);
    assert_eq!(path(&graph, "d-2", "d-1"), ids(&["d-2", "d-3", "d-1"]));
}

#[test]
fn a_blank_id_is_an_error_not_a_miss() {
    let graph = graph_with(&[]);
    assert!(follows_from_path(&graph, " ", "d-1").is_err());
    assert!(follows_from_path(&graph, "d-1", "").is_err());
}
