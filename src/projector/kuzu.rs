use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use kuzu::{Connection, Database, LogicalType, SystemConfig, Value};

use crate::error::ProjectorError;
use crate::Result;

use super::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};

const GRAPH_DB_NAME: &str = "graph.kuzu";

const NODE_DDL: &[(NodeKind, &str)] = &[
    (
        NodeKind::Decision,
        "CREATE NODE TABLE IF NOT EXISTS `Decision` (id STRING, title STRING, rationale STRING, topic_keys STRING[], event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::DecisionRequest,
        "CREATE NODE TABLE IF NOT EXISTS `DecisionRequest` (id STRING, decision_id STRING, topic_keys STRING[], reason STRING, priority STRING, required_owner_id STRING, authority_class STRING, requested_by STRING, client_request_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Actor,
        "CREATE NODE TABLE IF NOT EXISTS `Actor` (id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Blocker,
        "CREATE NODE TABLE IF NOT EXISTS `Blocker` (id STRING, blocked_actor_id STRING, decision_id STRING, topic_keys STRING[], blocked_ref STRING, blocked_ref_type STRING, reason STRING, priority STRING, last_progress_at STRING, required_owner_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Evidence,
        "CREATE NODE TABLE IF NOT EXISTS `Evidence` (id STRING, content STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Notification,
        "CREATE NODE TABLE IF NOT EXISTS `Notification` (id STRING, blocker_id STRING, recipient_actor_id STRING, channel STRING, threshold_rule STRING, source_event_ids STRING[], dedupe_key STRING, sent_at STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Option,
        "CREATE NODE TABLE IF NOT EXISTS `Option` (id STRING, label STRING, description STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Hypothesis,
        "CREATE NODE TABLE IF NOT EXISTS `Hypothesis` (id STRING, statement STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
];

const RELATION_DDL: &[(RelationKind, &str)] = &[
    (
        RelationKind::ProposedBy,
        "CREATE REL TABLE IF NOT EXISTS `PROPOSED_BY` (FROM `Decision` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DecisionRequestedBy,
        "CREATE REL TABLE IF NOT EXISTS `DECISION_REQUESTED_BY` (FROM `DecisionRequest` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DecisionRequestForDecision,
        "CREATE REL TABLE IF NOT EXISTS `DECISION_REQUEST_FOR_DECISION` (FROM `DecisionRequest` TO `Decision`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DecisionRequestRequiredOwner,
        "CREATE REL TABLE IF NOT EXISTS `DECISION_REQUEST_REQUIRED_OWNER` (FROM `DecisionRequest` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::AcceptedBy,
        "CREATE REL TABLE IF NOT EXISTS `ACCEPTED_BY` (FROM `Decision` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::RejectedBy,
        "CREATE REL TABLE IF NOT EXISTS `REJECTED_BY` (FROM `Decision` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Supersedes,
        "CREATE REL TABLE IF NOT EXISTS `SUPERSEDES` (FROM `Decision` TO `Decision`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BlockedActor,
        "CREATE REL TABLE IF NOT EXISTS `BLOCKED_ACTOR` (FROM `Blocker` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BlockerForDecision,
        "CREATE REL TABLE IF NOT EXISTS `BLOCKER_FOR_DECISION` (FROM `Blocker` TO `Decision`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BlockerRequiredOwner,
        "CREATE REL TABLE IF NOT EXISTS `BLOCKER_REQUIRED_OWNER` (FROM `Blocker` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::NotificationForBlocker,
        "CREATE REL TABLE IF NOT EXISTS `NOTIFICATION_FOR_BLOCKER` (FROM `Notification` TO `Blocker`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::NotificationRecipient,
        "CREATE REL TABLE IF NOT EXISTS `NOTIFICATION_RECIPIENT` (FROM `Notification` TO `Actor`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BasedOn,
        "CREATE REL TABLE IF NOT EXISTS `BASED_ON` (FROM `Decision` TO `Evidence`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::HasOption,
        "CREATE REL TABLE IF NOT EXISTS `HAS_OPTION` (FROM `Decision` TO `Option`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Chose,
        "CREATE REL TABLE IF NOT EXISTS `CHOSE` (FROM `Decision` TO `Option`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Assumes,
        "CREATE REL TABLE IF NOT EXISTS `ASSUMES` (FROM `Decision` TO `Hypothesis`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Supports,
        "CREATE REL TABLE IF NOT EXISTS `SUPPORTS` (FROM `Evidence` TO `Hypothesis`, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Refutes,
        "CREATE REL TABLE IF NOT EXISTS `REFUTES` (FROM `Evidence` TO `Hypothesis`, event_origin INT64, source STRING, source_ref STRING);",
    ),
];

#[derive(Debug)]
pub struct KuzuGraph {
    path: PathBuf,
    database: Database,
}

impl KuzuGraph {
    pub fn open(hivemind_dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(hivemind_dir.as_ref()).map_err(projector_error)?;
        let path = hivemind_dir.as_ref().join(GRAPH_DB_NAME);
        let database = Database::new(&path, SystemConfig::default()).map_err(projector_error)?;
        let graph = Self { path, database };
        graph.initialize_schema()?;
        Ok(graph)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn initialize_schema(&self) -> Result<()> {
        let connection = self.connection()?;
        for (_, statement) in NODE_DDL {
            connection.query(statement).map_err(projector_error)?;
        }
        for (_, statement) in RELATION_DDL {
            connection.query(statement).map_err(projector_error)?;
        }
        Ok(())
    }

    fn connection(&self) -> Result<Connection<'_>> {
        Ok(Connection::new(&self.database).map_err(projector_error)?)
    }
}

impl GraphView for KuzuGraph {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
        let table = quote_identifier(kind.table_name())?;
        let mut params = BTreeMap::from([("id".to_string(), GraphValue::String(id.to_string()))]);
        params.extend(properties.clone());
        let query = format!(
            "MERGE (node:{table} {{id: $id}}){};",
            set_clause("node", properties)?
        );
        self.execute_query(&query, &params).map(|_| ())
    }

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()> {
        let (from_kind, to_kind) = kind.endpoints();
        let from_table = quote_identifier(from_kind.table_name())?;
        let to_table = quote_identifier(to_kind.table_name())?;
        let relation = quote_identifier(kind.table_name())?;
        let mut params = BTreeMap::from([
            (
                "from_id".to_string(),
                GraphValue::String(from_id.to_string()),
            ),
            ("to_id".to_string(), GraphValue::String(to_id.to_string())),
        ]);
        params.extend(properties.clone());
        let query = format!(
            "MATCH (from:{from_table} {{id: $from_id}}), (to:{to_table} {{id: $to_id}}) MERGE (from)-[rel:{relation}]->(to){};",
            set_clause("rel", properties)?
        );
        self.execute_query(&query, &params).map(|_| ())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        self.execute_query(cypher, params)
    }

    fn wipe(&self) -> Result<()> {
        let connection = self.connection()?;
        for kind in RelationKind::ALL.iter().rev() {
            connection
                .query(&format!(
                    "DROP TABLE IF EXISTS {};",
                    quote_identifier(kind.table_name())?
                ))
                .map_err(projector_error)?;
        }
        for kind in NodeKind::ALL.iter().rev() {
            connection
                .query(&format!(
                    "DROP TABLE IF EXISTS {};",
                    quote_identifier(kind.table_name())?
                ))
                .map_err(projector_error)?;
        }
        self.initialize_schema()
    }
}

impl KuzuGraph {
    fn execute_query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        let connection = self.connection()?;
        let mut result = if params.is_empty() {
            connection.query(cypher).map_err(projector_error)?
        } else {
            let mut prepared = connection.prepare(cypher).map_err(projector_error)?;
            let kuzu_params = params
                .iter()
                .map(|(key, value)| (key.as_str(), to_kuzu_value(value)))
                .collect();
            connection
                .execute(&mut prepared, kuzu_params)
                .map_err(projector_error)?
        };
        let column_names = result.get_column_names();
        let mut rows = Vec::new();
        for row in &mut result {
            let mut graph_row = GraphRow::new();
            for (name, value) in column_names.iter().zip(row) {
                graph_row.insert(name.clone(), from_kuzu_value(value));
            }
            rows.push(graph_row);
        }
        Ok(rows)
    }
}

fn set_clause(alias: &str, properties: &GraphProperties) -> Result<String> {
    if properties.is_empty() {
        return Ok(String::new());
    }

    let mut assignments = Vec::new();
    for key in properties.keys() {
        assignments.push(format!(
            "{alias}.{} = ${key}",
            quote_identifier(key.as_str())?
        ));
    }
    Ok(format!(" SET {}", assignments.join(", ")))
}

fn quote_identifier(identifier: &str) -> Result<String> {
    if !identifier.is_empty()
        && identifier
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        Ok(format!("`{identifier}`"))
    } else {
        Err(ProjectorError::Projection(format!("invalid graph identifier: {identifier}")).into())
    }
}

fn to_kuzu_value(value: &GraphValue) -> Value {
    match value {
        GraphValue::Null => Value::Null(LogicalType::String),
        GraphValue::Bool(value) => Value::Bool(*value),
        GraphValue::Int(value) => Value::Int64(*value),
        GraphValue::Float(value) => Value::Double(*value),
        GraphValue::String(value) => Value::String(value.clone()),
        GraphValue::StringList(values) => Value::List(
            LogicalType::String,
            values.iter().cloned().map(Value::String).collect(),
        ),
    }
}

fn from_kuzu_value(value: Value) -> GraphValue {
    match value {
        Value::Null(_) => GraphValue::Null,
        Value::Bool(value) => GraphValue::Bool(value),
        Value::Int64(value) => GraphValue::Int(value),
        Value::Int32(value) => GraphValue::Int(value.into()),
        Value::Int16(value) => GraphValue::Int(value.into()),
        Value::Int8(value) => GraphValue::Int(value.into()),
        Value::UInt64(value) => GraphValue::Int(value.try_into().unwrap_or(i64::MAX)),
        Value::UInt32(value) => GraphValue::Int(value.into()),
        Value::UInt16(value) => GraphValue::Int(value.into()),
        Value::UInt8(value) => GraphValue::Int(value.into()),
        Value::Double(value) => GraphValue::Float(value),
        Value::Float(value) => GraphValue::Float(value.into()),
        Value::String(value) => GraphValue::String(value),
        Value::List(LogicalType::String, values) | Value::Array(LogicalType::String, values) => {
            GraphValue::StringList(
                values
                    .into_iter()
                    .filter_map(|value| match value {
                        Value::String(value) => Some(value),
                        _ => None,
                    })
                    .collect(),
            )
        }
        other => GraphValue::String(other.to_string()),
    }
}

fn projector_error(error: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(error.to_string())
}

#[cfg(test)]
mod tests {
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
}
