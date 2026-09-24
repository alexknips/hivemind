// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

#[test]
fn initializes_slice_one_schema() -> Result<()> {
    let temp_dir = test_graph_dir("schema");
    let graph = KuzuGraph::open(&temp_dir)?;

    for kind in NodeKind::ALL {
        let rows = graph.query(
            &format!(
                "MATCH (node:{}) RETURN count(node) AS count;",
                quote_identifier(kind.table_name())?
            ),
            &GraphParams::new(),
        )?;
        assert_eq!(rows.len(), 1);
    }

    for kind in RelationKind::ALL {
        let rows = graph.query(
            &format!(
                "MATCH ()-[rel:{}]->() RETURN count(rel) AS count;",
                quote_identifier(kind.table_name())?
            ),
            &GraphParams::new(),
        )?;
        assert_eq!(rows.len(), 1);
    }

    assert!(graph.path().exists());
    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

#[test]
fn upserts_decision_with_topic_keys() -> Result<()> {
    let temp_dir = test_graph_dir("topic_keys");
    let graph = KuzuGraph::open(&temp_dir)?;
    let properties = GraphProperties::from([
        (
            "title".to_string(),
            GraphValue::String("Choose Kuzu".to_string()),
        ),
        (
            "topic_keys".to_string(),
            GraphValue::StringList(vec!["graph".to_string(), "slice-1".to_string()]),
        ),
        ("event_origin".to_string(), GraphValue::Int(1)),
    ]);

    graph.upsert_node(NodeKind::Decision, "decision-1", &properties)?;
    let rows = graph.query(
        "MATCH (decision:`Decision` {id: $id}) RETURN decision.topic_keys AS topic_keys;",
        &GraphParams::from([(
            "id".to_string(),
            GraphValue::String("decision-1".to_string()),
        )]),
    )?;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("topic_keys"),
        Some(&GraphValue::StringList(vec![
            "graph".to_string(),
            "slice-1".to_string()
        ]))
    );
    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

#[test]
fn upserts_delegated_by_onto_an_existing_decision_without_disturbing_it() -> Result<()> {
    // hivemind-zdsh.6: the delegation marker is a Decision column, set by a second upsert
    // (the acceptance) after the proposal's own.
    let temp_dir = test_graph_dir("delegated-by");
    let graph = KuzuGraph::open(&temp_dir)?;
    graph.upsert_node(
        NodeKind::Decision,
        "decision-1",
        &GraphProperties::from([
            (
                "title".to_string(),
                GraphValue::String("Agent decides".to_string()),
            ),
            ("event_origin".to_string(), GraphValue::Int(1)),
        ]),
    )?;
    graph.upsert_node(
        NodeKind::Decision,
        "decision-1",
        &GraphProperties::from([(
            "delegated_by".to_string(),
            GraphValue::String("human:alex".to_string()),
        )]),
    )?;

    let rows = graph.query(
        "MATCH (decision:`Decision` {id: $id}) RETURN decision.title AS title, decision.delegated_by AS delegated_by, decision.event_origin AS event_origin;",
        &GraphParams::from([(
            "id".to_string(),
            GraphValue::String("decision-1".to_string()),
        )]),
    )?;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("delegated_by"),
        Some(&GraphValue::String("human:alex".to_string()))
    );
    assert_eq!(
        rows[0].get("title"),
        Some(&GraphValue::String("Agent decides".to_string()))
    );
    assert_eq!(rows[0].get("event_origin"), Some(&GraphValue::Int(1)));
    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

#[test]
fn upserts_hypothesis_with_kind_check_by_and_would_change_if() -> Result<()> {
    let temp_dir = test_graph_dir("hypothesis-kind");
    let graph = KuzuGraph::open(&temp_dir)?;
    let properties = GraphProperties::from([
        (
            "statement".to_string(),
            GraphValue::String("Latency stays under 50ms at 10x load".to_string()),
        ),
        ("kind".to_string(), GraphValue::String("bet".to_string())),
        (
            "check_by".to_string(),
            GraphValue::String("2026-10-01T00:00:00+00:00".to_string()),
        ),
        (
            "would_change_if".to_string(),
            GraphValue::String("A 10x load test shows p99 above 50ms".to_string()),
        ),
        ("event_origin".to_string(), GraphValue::Int(1)),
    ]);

    graph.upsert_node(NodeKind::Hypothesis, "hypothesis-bet", &properties)?;
    let rows = graph.query(
        "MATCH (h:`Hypothesis` {id: $id}) RETURN h.kind AS kind, h.check_by AS check_by, h.would_change_if AS would_change_if;",
        &GraphParams::from([(
            "id".to_string(),
            GraphValue::String("hypothesis-bet".to_string()),
        )]),
    )?;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("kind"),
        Some(&GraphValue::String("bet".to_string()))
    );
    assert_eq!(
        rows[0].get("would_change_if"),
        Some(&GraphValue::String(
            "A 10x load test shows p99 above 50ms".to_string()
        ))
    );
    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

#[test]
fn upserts_follows_from_edge_between_decisions() -> Result<()> {
    let temp_dir = test_graph_dir("follows-from");
    let graph = KuzuGraph::open(&temp_dir)?;
    let decision_properties = GraphProperties::from([(
        "title".to_string(),
        GraphValue::String("A decision".to_string()),
    )]);
    graph.upsert_node(NodeKind::Decision, "decision-later", &decision_properties)?;
    graph.upsert_node(NodeKind::Decision, "decision-earlier", &decision_properties)?;

    graph.upsert_edge(
        RelationKind::FollowsFrom,
        "decision-later",
        "decision-earlier",
        &GraphProperties::new(),
    )?;

    let rows = graph.query(
        "MATCH (from:`Decision` {id: $from_id})-[:`FOLLOWS_FROM`]->(to:`Decision`) RETURN to.id AS to_id;",
        &GraphParams::from([(
            "from_id".to_string(),
            GraphValue::String("decision-later".to_string()),
        )]),
    )?;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("to_id"),
        Some(&GraphValue::String("decision-earlier".to_string()))
    );
    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

#[test]
fn grounding_edges_store_who_added_them_and_the_causing_proposal() -> Result<()> {
    let temp_dir = test_graph_dir("grounding-provenance");
    let graph = KuzuGraph::open(&temp_dir)?;
    let decision_properties = GraphProperties::from([(
        "title".to_string(),
        GraphValue::String("A decision".to_string()),
    )]);
    for id in ["decision-later", "decision-earlier"] {
        graph.upsert_node(NodeKind::Decision, id, &decision_properties)?;
    }
    graph.upsert_node(
        NodeKind::Evidence,
        "evidence-1",
        &GraphProperties::from([
            (
                "content".to_string(),
                GraphValue::String("27 of 27 captures carry no premise".to_string()),
            ),
            (
                "evidence_source".to_string(),
                GraphValue::String("mayor audit 2026-09-22".to_string()),
            ),
            (
                "recorded_at".to_string(),
                GraphValue::String("2026-01-01T00:00:00+00:00".to_string()),
            ),
        ]),
    )?;

    let provenance = |added_by: &str, causation: Option<i64>| {
        let mut properties = GraphProperties::from([
            ("event_origin".to_string(), GraphValue::Int(7)),
            (
                "added_by".to_string(),
                GraphValue::String(added_by.to_string()),
            ),
            (
                "added_at".to_string(),
                GraphValue::String("2026-01-02T00:00:00+00:00".to_string()),
            ),
        ]);
        if let Some(causation) = causation {
            properties.insert("causation_event_id".to_string(), GraphValue::Int(causation));
        }
        properties
    };
    graph.upsert_edge(
        RelationKind::FollowsFrom,
        "decision-later",
        "decision-earlier",
        &provenance("actor:alice", Some(6)),
    )?;
    graph.upsert_edge(
        RelationKind::BasedOn,
        "decision-later",
        "evidence-1",
        &provenance("actor:bob", None),
    )?;

    let by_id = GraphParams::from([(
        "id".to_string(),
        GraphValue::String("decision-later".to_string()),
    )]);
    let follows = graph.query(
        "MATCH (a:`Decision` {id: $id})-[r:`FOLLOWS_FROM`]->(b:`Decision`) RETURN b.id AS id, r.event_origin AS event_origin, r.causation_event_id AS causation_event_id, r.added_by AS added_by, r.added_at AS added_at ORDER BY b.id;",
        &by_id,
    )?;
    assert_eq!(follows.len(), 1);
    assert_eq!(
        follows[0].get("causation_event_id"),
        Some(&GraphValue::Int(6))
    );
    assert_eq!(
        follows[0].get("added_by"),
        Some(&GraphValue::String("actor:alice".to_string()))
    );

    // An edge written with no causation reads back null, not zero.
    let based_on = graph.query(
        "MATCH (a:`Decision` {id: $id})-[r:`BASED_ON`]->(b:`Evidence`) RETURN b.id AS id, r.event_origin AS event_origin, r.causation_event_id AS causation_event_id, r.added_by AS added_by, r.added_at AS added_at ORDER BY b.id;",
        &by_id,
    )?;
    assert_eq!(based_on.len(), 1);
    assert_eq!(
        based_on[0].get("causation_event_id"),
        Some(&GraphValue::Null)
    );
    assert_eq!(
        based_on[0].get("added_by"),
        Some(&GraphValue::String("actor:bob".to_string()))
    );

    let evidence = graph.query(
        "MATCH (node:`Evidence` {id: $id}) RETURN node.id AS id, node.content AS content, node.evidence_source AS evidence_source, node.recorded_at AS recorded_at LIMIT 1;",
        &GraphParams::from([(
            "id".to_string(),
            GraphValue::String("evidence-1".to_string()),
        )]),
    )?;
    assert_eq!(evidence.len(), 1);
    assert_eq!(
        evidence[0].get("evidence_source"),
        Some(&GraphValue::String("mayor audit 2026-09-22".to_string()))
    );
    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

fn test_graph_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("hivemind-{name}-{nanos}"))
}
