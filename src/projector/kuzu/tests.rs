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

fn test_graph_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("hivemind-{name}-{nanos}"))
}
