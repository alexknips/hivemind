//! Kuzu-backed graph projector: persists the decision graph in a Kuzu native graph database.

use std::collections::BTreeMap;
use std::fmt::Write as _;
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
        "CREATE NODE TABLE IF NOT EXISTS `Decision` (id STRING, title STRING, rationale STRING, topic_keys STRING[], expressed_confidence STRING, project STRING, project_source STRING, occurred_at STRING, quote STRING, question STRING, delegated_by STRING, score_framing DOUBLE, score_alternatives DOUBLE, score_information DOUBLE, score_reasoning DOUBLE, score_values_tradeoffs DOUBLE, score_bias_exposure DOUBLE, score_calibration DOUBLE, score_weight_version STRING, importance_stakes DOUBLE, importance_irreversibility DOUBLE, importance_actionability DOUBLE, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::DecisionRequest,
        "CREATE NODE TABLE IF NOT EXISTS `DecisionRequest` (id STRING, decision_id STRING, topic_keys STRING[], reason STRING, priority STRING, required_owner_id STRING, authority_class STRING, requested_by STRING, client_request_id STRING, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Actor,
        "CREATE NODE TABLE IF NOT EXISTS `Actor` (id STRING, kind STRING, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Evidence,
        "CREATE NODE TABLE IF NOT EXISTS `Evidence` (id STRING, content STRING, evidence_source STRING, recorded_at STRING, topic_keys STRING[], tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Option,
        "CREATE NODE TABLE IF NOT EXISTS `Option` (id STRING, label STRING, description STRING, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Hypothesis,
        "CREATE NODE TABLE IF NOT EXISTS `Hypothesis` (id STRING, statement STRING, kind STRING, check_by STRING, would_change_if STRING, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Question,
        "CREATE NODE TABLE IF NOT EXISTS `Question` (id STRING, text STRING, normalized_text STRING, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Blocker,
        "CREATE NODE TABLE IF NOT EXISTS `Blocker` (id STRING, blocked_actor_id STRING, decision_id STRING, topic_keys STRING[], blocked_ref STRING, blocked_ref_type STRING, reason STRING, priority STRING, last_progress_at STRING, required_owner_id STRING, reported_at STRING, reported_event_origin INT64, resolved_at STRING, resolution_event_id INT64, resolution_reason STRING, resolved_event_origin INT64, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Notification,
        "CREATE NODE TABLE IF NOT EXISTS `Notification` (id STRING, blocker_id STRING, recipient_actor_id STRING, channel STRING, threshold_rule STRING, source_event_ids STRING[], dedupe_key STRING, sent_at STRING, ack_at STRING, snooze_until STRING, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
    (
        NodeKind::Project,
        "CREATE NODE TABLE IF NOT EXISTS `Project` (id STRING, handle STRING, display_name STRING, purpose STRING, anchors STRING[], tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, PRIMARY KEY(id));",
    ),
];

const RELATION_DDL: &[(RelationKind, &str)] = &[
    (
        RelationKind::ProposedBy,
        "CREATE REL TABLE IF NOT EXISTS `PROPOSED_BY` (FROM `Decision` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DecisionRequestedBy,
        "CREATE REL TABLE IF NOT EXISTS `DECISION_REQUESTED_BY` (FROM `DecisionRequest` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DecisionRequestForDecision,
        "CREATE REL TABLE IF NOT EXISTS `DECISION_REQUEST_FOR_DECISION` (FROM `DecisionRequest` TO `Decision`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DecisionRequestRequiredOwner,
        "CREATE REL TABLE IF NOT EXISTS `DECISION_REQUEST_REQUIRED_OWNER` (FROM `DecisionRequest` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::AcceptedBy,
        "CREATE REL TABLE IF NOT EXISTS `ACCEPTED_BY` (FROM `Decision` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::RejectedBy,
        "CREATE REL TABLE IF NOT EXISTS `REJECTED_BY` (FROM `Decision` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::RequestProposedBy,
        "CREATE REL TABLE IF NOT EXISTS `REQUEST_PROPOSED_BY` (FROM `DecisionRequest` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::RequestAcceptedBy,
        "CREATE REL TABLE IF NOT EXISTS `REQUEST_ACCEPTED_BY` (FROM `DecisionRequest` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::RequestRejectedBy,
        "CREATE REL TABLE IF NOT EXISTS `REQUEST_REJECTED_BY` (FROM `DecisionRequest` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Supersedes,
        "CREATE REL TABLE IF NOT EXISTS `SUPERSEDES` (FROM `Decision` TO `Decision`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BlockedActor,
        "CREATE REL TABLE IF NOT EXISTS `BLOCKED_ACTOR` (FROM `Blocker` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BlockerForDecision,
        "CREATE REL TABLE IF NOT EXISTS `BLOCKER_FOR_DECISION` (FROM `Blocker` TO `Decision`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BlockerRequiredOwner,
        "CREATE REL TABLE IF NOT EXISTS `BLOCKER_REQUIRED_OWNER` (FROM `Blocker` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::NotificationForBlocker,
        "CREATE REL TABLE IF NOT EXISTS `NOTIFICATION_FOR_BLOCKER` (FROM `Notification` TO `Blocker`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::NotificationRecipient,
        "CREATE REL TABLE IF NOT EXISTS `NOTIFICATION_RECIPIENT` (FROM `Notification` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::BasedOn,
        "CREATE REL TABLE IF NOT EXISTS `BASED_ON` (FROM `Decision` TO `Evidence`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, added_by STRING, added_at STRING, causation_event_id INT64);",
    ),
    (
        RelationKind::HasOption,
        "CREATE REL TABLE IF NOT EXISTS `HAS_OPTION` (FROM `Decision` TO `Option`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Chose,
        "CREATE REL TABLE IF NOT EXISTS `CHOSE` (FROM `Decision` TO `Option`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::PremisedOn,
        "CREATE REL TABLE IF NOT EXISTS `PREMISED_ON` (FROM `Option` TO `Hypothesis`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, added_by STRING, added_at STRING, causation_event_id INT64);",
    ),
    (
        RelationKind::PremisedOnDirect,
        "CREATE REL TABLE IF NOT EXISTS `PREMISED_ON_DIRECT` (FROM `Decision` TO `Hypothesis`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, added_by STRING, added_at STRING, causation_event_id INT64);",
    ),
    (
        RelationKind::Supports,
        "CREATE REL TABLE IF NOT EXISTS `SUPPORTS` (FROM `Evidence` TO `Hypothesis`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::Refutes,
        "CREATE REL TABLE IF NOT EXISTS `REFUTES` (FROM `Evidence` TO `Hypothesis`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::SameAs,
        "CREATE REL TABLE IF NOT EXISTS `SAME_AS` (FROM `Decision` TO `Decision`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::ParticipatedBy,
        "CREATE REL TABLE IF NOT EXISTS `PARTICIPATED_BY` (FROM `Decision` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::InitiatedBy,
        "CREATE REL TABLE IF NOT EXISTS `INITIATED_BY` (FROM `Decision` TO `Actor`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::PartOf,
        "CREATE REL TABLE IF NOT EXISTS `PART_OF` (FROM `Project` TO `Project`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::DependsOn,
        "CREATE REL TABLE IF NOT EXISTS `DEPENDS_ON` (FROM `Project` TO `Project`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
    ),
    (
        RelationKind::FollowsFrom,
        "CREATE REL TABLE IF NOT EXISTS `FOLLOWS_FROM` (FROM `Decision` TO `Decision`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING, added_by STRING, added_at STRING, causation_event_id INT64);",
    ),
    (
        RelationKind::Answers,
        "CREATE REL TABLE IF NOT EXISTS `ANSWERS` (FROM `Decision` TO `Question`, tenant_id STRING, event_origin INT64, source STRING, source_ref STRING);",
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

    /// Creates any table this build defines that the database lacks. `IF NOT EXISTS` never
    /// alters a table that is already there, so a `graph.kuzu` written by an older build keeps
    /// its old columns until `wipe` drops and recreates every table. The projection is derived
    /// state and every caller rebuilds it from the ledger straight after `open`
    /// (`rebuild_graph_for_tenant`), so an old database is replaced, never migrated.
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
        params.extend(bound_properties(properties));
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
        params.extend(bound_properties(properties));
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
                .query(&drop_table_query(kind.table_name())?)
                .map_err(projector_error)?;
        }
        for kind in NodeKind::ALL.iter().rev() {
            connection
                .query(&drop_table_query(kind.table_name())?)
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

/// A null property is written as the `NULL` literal, which takes the type of the column it lands
/// in. Bound as a parameter it would be a null STRING, and Kuzu refuses to store that in an
/// INT64 column (`resolution_event_id` is null for a resolution that names only a reason).
fn set_clause(alias: &str, properties: &GraphProperties) -> Result<String> {
    if properties.is_empty() {
        return Ok(String::new());
    }

    let mut assignments = Vec::new();
    for (key, value) in properties {
        let quoted = quote_identifier(key.as_str())?;
        let mut assignment = String::with_capacity(alias.len() + quoted.len() + key.len() + 5);
        if matches!(value, GraphValue::Null) {
            let _ = write!(assignment, "{alias}.{quoted} = NULL");
        } else {
            let _ = write!(assignment, "{alias}.{quoted} = ${key}");
        }
        assignments.push(assignment);
    }
    Ok(format!(" SET {}", assignments.join(", ")))
}

/// The properties `set_clause` binds as parameters: every one but the nulls it writes inline.
fn bound_properties(
    properties: &GraphProperties,
) -> impl Iterator<Item = (String, GraphValue)> + '_ {
    properties
        .iter()
        .filter(|(_, value)| !matches!(value, GraphValue::Null))
        .map(|(key, value)| (key.clone(), value.clone()))
}

fn drop_table_query(table: &str) -> Result<String> {
    let quoted = quote_identifier(table)?;
    let mut query = String::with_capacity(quoted.len() + "DROP TABLE IF EXISTS ;".len());
    let _ = write!(query, "DROP TABLE IF EXISTS {quoted};");
    Ok(query)
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
mod tests;
