// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::queries::{HypothesisStatus, NeighborEdge, NeighborNode, NeighborhoodRoot};

#[test]
fn status_filter_cycles_through_supported_values() {
    let mut app = DecisionSearchApp::new(TuiConfig {
        query: None,
        topic_keys: Vec::new(),
        statuses: Vec::new(),
        actor_ids: Vec::new(),
        sources: Vec::new(),
        limit: 25,
        dot_output: PathBuf::from("out.dot"),
    });

    let expected = [
        "proposed",
        "accepted",
        "rejected",
        "contested",
        "superseded",
        "any",
    ];
    for label in expected {
        app.cycle_status_filter();
        assert_eq!(app.status_label(), label);
    }
}

#[test]
fn search_request_normalizes_inputs() {
    let app = DecisionSearchApp::new(TuiConfig {
        query: Some(" queue ".to_owned()),
        topic_keys: vec!["infra".to_owned(), " storage ".to_owned()],
        statuses: vec![DecisionStatus::Accepted],
        actor_ids: vec!["actor:1".to_owned()],
        sources: vec!["agent".to_owned()],
        limit: 10,
        dot_output: PathBuf::from("out.dot"),
    });

    let request = app.search_request();
    assert_eq!(request.query.as_deref(), Some("queue"));
    assert_eq!(request.topic_keys, vec!["infra", "storage"]);
    assert_eq!(request.statuses, vec![DecisionStatus::Accepted]);
    assert_eq!(request.actor_ids, vec!["actor:1"]);
    assert_eq!(request.sources, vec!["agent"]);
    assert_eq!(request.limit, 10);
}

#[test]
fn dot_export_renders_node_statuses_and_relation_labels() {
    let neighborhood = NeighborhoodView {
        root: NeighborhoodRoot {
            id: "d1".to_owned(),
            kind: NodeKind::Decision,
            present: true,
        },
        nodes: vec![
            NeighborNode {
                id: "d1".to_owned(),
                kind: NodeKind::Decision,
                decision_status: Some(DecisionStatus::Accepted),
                hypothesis_status: None,
            },
            NeighborNode {
                id: "h1".to_owned(),
                kind: NodeKind::Hypothesis,
                decision_status: None,
                hypothesis_status: Some(HypothesisStatus::Refuted),
            },
        ],
        edges: vec![NeighborEdge {
            from: "d1".to_owned(),
            to: "h1".to_owned(),
            relation: RelationKind::PremisedOn,
            event_origin: Some(7),
        }],
    };

    let dot = render_neighborhood_dot(&neighborhood);

    assert!(dot.contains("Decision:d1"));
    assert!(dot.contains("status: accepted"));
    assert!(dot.contains("status: refuted"));
    assert!(dot.contains("ASSUMES"));
}
