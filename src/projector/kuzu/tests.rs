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

fn test_graph_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("hivemind-{name}-{nanos}"))
}
