use std::str::FromStr;

use ::postgres::{Config, NoTls, Row};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use serde_json::{Map, Number, Value};

use crate::error::ProjectorError;
use crate::events::TenantId;
use crate::Result;

use super::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};

const DEFAULT_POOL_SIZE: u32 = 16;

type PgManager = PostgresConnectionManager<NoTls>;
type PgPool = Pool<PgManager>;

#[derive(Clone)]
pub struct PostgresGraphView {
    pool: PgPool,
    tenant_id: TenantId,
}

impl std::fmt::Debug for PostgresGraphView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresGraphView")
            .field("tenant_id", &self.tenant_id)
            .finish_non_exhaustive()
    }
}

impl PostgresGraphView {
    pub fn connect(database_url: &str, tenant_id: impl Into<String>) -> Result<Self> {
        Self::connect_with_pool_size(database_url, tenant_id, DEFAULT_POOL_SIZE)
    }

    pub fn connect_local_default(database_url: &str) -> Result<Self> {
        Self::connect(database_url, TenantId::LOCAL_VALUE)
    }

    pub fn connect_with_pool_size(
        database_url: &str,
        tenant_id: impl Into<String>,
        max_size: u32,
    ) -> Result<Self> {
        let config = Config::from_str(database_url).map_err(projector_error)?;
        let manager = PostgresConnectionManager::new(config, NoTls);
        let pool = Pool::builder()
            .max_size(max_size)
            .build(manager)
            .map_err(projector_error)?;

        Self::from_pool(pool, tenant_id)
    }

    fn from_pool(pool: PgPool, tenant_id: impl Into<String>) -> Result<Self> {
        let tenant_id = TenantId::new(tenant_id.into()).map_err(projector_error)?;
        let graph = Self { pool, tenant_id };
        graph.initialize_schema()?;
        Ok(graph)
    }

    pub fn for_tenant(&self, tenant_id: impl Into<String>) -> Result<Self> {
        Ok(Self {
            pool: self.pool.clone(),
            tenant_id: TenantId::new(tenant_id.into()).map_err(projector_error)?,
        })
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    pub fn initialize_schema(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(projector_error)?;
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS graph_nodes (
                    tenant_id text NOT NULL,
                    kind text NOT NULL,
                    id text NOT NULL,
                    properties jsonb NOT NULL DEFAULT '{}'::jsonb,
                    event_origin bigint,
                    source text,
                    source_ref text,
                    search_vector tsvector NOT NULL DEFAULT ''::tsvector,
                    PRIMARY KEY (tenant_id, kind, id)
                );
                CREATE INDEX IF NOT EXISTS graph_nodes_tenant_kind_idx
                    ON graph_nodes (tenant_id, kind);
                CREATE INDEX IF NOT EXISTS graph_nodes_search_idx
                    ON graph_nodes USING GIN (search_vector);
                CREATE TABLE IF NOT EXISTS graph_edges (
                    tenant_id text NOT NULL,
                    relation text NOT NULL,
                    from_id text NOT NULL,
                    to_id text NOT NULL,
                    properties jsonb NOT NULL DEFAULT '{}'::jsonb,
                    event_origin bigint,
                    source text,
                    source_ref text,
                    PRIMARY KEY (tenant_id, relation, from_id, to_id)
                );
                CREATE INDEX IF NOT EXISTS graph_edges_tenant_relation_from_idx
                    ON graph_edges (tenant_id, relation, from_id);
                CREATE INDEX IF NOT EXISTS graph_edges_tenant_relation_to_idx
                    ON graph_edges (tenant_id, relation, to_id);",
            )
            .map_err(projector_error)?;
        Ok(())
    }
}

impl GraphView for PostgresGraphView {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
        self.validate_property_tenant(properties)?;
        let kind_name = kind.table_name();
        let properties_json = graph_properties_to_json(properties)?;
        let event_origin = optional_property_int(properties, "event_origin");
        let source = optional_property_string(properties, "source");
        let source_ref = optional_property_string(properties, "source_ref");
        let search_document = search_document(id, properties);
        let mut client = self.pool.get().map_err(projector_error)?;
        client
            .execute(
                "INSERT INTO graph_nodes (
                    tenant_id,
                    kind,
                    id,
                    properties,
                    event_origin,
                    source,
                    source_ref,
                    search_vector
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, to_tsvector('simple', $8))
                ON CONFLICT (tenant_id, kind, id) DO UPDATE SET
                    properties = graph_nodes.properties || EXCLUDED.properties,
                    event_origin = COALESCE(EXCLUDED.event_origin, graph_nodes.event_origin),
                    source = COALESCE(EXCLUDED.source, graph_nodes.source),
                    source_ref = COALESCE(EXCLUDED.source_ref, graph_nodes.source_ref),
                    search_vector = to_tsvector(
                        'simple',
                        graph_nodes.id || ' ' ||
                            (graph_nodes.properties || EXCLUDED.properties)::text
                    )",
                &[
                    &self.tenant_id.as_str(),
                    &kind_name,
                    &id,
                    &properties_json,
                    &event_origin,
                    &source,
                    &source_ref,
                    &search_document,
                ],
            )
            .map_err(projector_error)?;
        Ok(())
    }

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()> {
        self.validate_property_tenant(properties)?;
        let relation = kind.table_name();
        let properties_json = graph_properties_to_json(properties)?;
        let event_origin = optional_property_int(properties, "event_origin");
        let source = optional_property_string(properties, "source");
        let source_ref = optional_property_string(properties, "source_ref");
        let mut client = self.pool.get().map_err(projector_error)?;
        client
            .execute(
                "INSERT INTO graph_edges (
                    tenant_id,
                    relation,
                    from_id,
                    to_id,
                    properties,
                    event_origin,
                    source,
                    source_ref
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (tenant_id, relation, from_id, to_id) DO UPDATE SET
                    properties = graph_edges.properties || EXCLUDED.properties,
                    event_origin = COALESCE(EXCLUDED.event_origin, graph_edges.event_origin),
                    source = COALESCE(EXCLUDED.source, graph_edges.source),
                    source_ref = COALESCE(EXCLUDED.source_ref, graph_edges.source_ref)",
                &[
                    &self.tenant_id.as_str(),
                    &relation,
                    &from_id,
                    &to_id,
                    &properties_json,
                    &event_origin,
                    &source,
                    &source_ref,
                ],
            )
            .map_err(projector_error)?;
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        if cypher.contains("RETURN count(rel) AS count;") {
            let relation = query_relation(cypher)?;
            let id = required_param_string(params, "id")?;
            let incoming = cypher.contains("<-[rel:");
            return self.relation_count(relation, id, incoming);
        }

        if cypher.contains("RETURN node.id AS id LIMIT 1;") {
            let kind = query_node_kind(cypher)?;
            let id = required_param_string(params, "id")?;
            return self.node_exists(kind, id);
        }

        if cypher.contains("MATCH (d:`Decision` {id: $id}) RETURN d.id AS id LIMIT 1;") {
            let id = required_param_string(params, "id")?;
            return self.node_exists(NodeKind::Decision, id);
        }

        if cypher.contains("RETURN node.id AS id") {
            let kind = query_node_kind(cypher)?;
            return self.node_rows(kind);
        }

        if cypher.contains("RETURN from.id AS from_id, to.id AS to_id") {
            let relation = query_relation(cypher)?;
            return self.relation_edges(relation);
        }

        if cypher.contains(
            "RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys LIMIT 1;",
        ) {
            let id = required_param_string(params, "id")?;
            return self.decision_by_id(id);
        }

        if cypher.contains("RETURN count(d) AS count;") {
            let topic = required_param_string(params, "topic")?;
            return self.decision_topic_count(topic);
        }

        if cypher.contains("WHERE $topic IN d.topic_keys RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys ORDER BY d.id LIMIT 1000;") {
            let topic = required_param_string(params, "topic")?;
            return self.decisions_for_topic(topic);
        }

        if cypher.contains("RETURN n.id AS") {
            let relation = query_relation(cypher)?;
            let decision_id = required_param_string(params, "id")?;
            let alias = neighbor_alias(cypher)?;
            return self.outgoing_neighbor_ids(relation, decision_id, alias);
        }

        if cypher.contains("RETURN b.id AS id, r.event_origin AS event_origin ORDER BY b.id;") {
            let relation = query_relation(cypher)?;
            let id = required_param_string(params, "id")?;
            let incoming = cypher.contains("<-[r:`");
            return self.neighbor_pairs(relation, id, incoming);
        }

        if cypher.contains("MATCH (d:`Decision` {id: $id})-[:`SUPERSEDES`]->(other:`Decision`)") {
            let id = required_param_string(params, "id")?;
            return self.supersession_neighbors(id, SupersessionDirection::Older);
        }

        if cypher.contains("MATCH (other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision` {id: $id})") {
            let id = required_param_string(params, "id")?;
            return self.supersession_neighbors(id, SupersessionDirection::Newer);
        }

        Err(projector_error(format!("unsupported query: {cypher}")).into()) // ubs:ignore: internal query diagnostics are not hot-loop allocation risk.
    }

    fn wipe(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(projector_error)?;
        let tenant_id = self.tenant_id.as_str();
        client
            .execute(
                "DELETE FROM graph_edges WHERE tenant_id = $1",
                &[&tenant_id],
            )
            .map_err(projector_error)?;
        client
            .execute(
                "DELETE FROM graph_nodes WHERE tenant_id = $1",
                &[&tenant_id],
            )
            .map_err(projector_error)?;
        Ok(())
    }
}

impl PostgresGraphView {
    fn validate_property_tenant(&self, properties: &GraphProperties) -> Result<()> {
        match properties.get("tenant_id") {
            Some(GraphValue::String(value)) if value == self.tenant_id.as_str() => Ok(()),
            Some(GraphValue::String(value)) => Err(projector_error(format!(
                "projection tenant mismatch: graph={} event={value}",
                self.tenant_id
            ))
            .into()),
            Some(GraphValue::Null) | None => Ok(()),
            Some(other) => Err(projector_error(format!(
                "tenant_id property must be a string or null, got {other:?}"
            ))
            .into()),
        }
    }

    fn relation_count(
        &self,
        relation: RelationKind,
        node_id: &str,
        incoming: bool,
    ) -> Result<Vec<GraphRow>> {
        let endpoint_column = if incoming { "to_id" } else { "from_id" };
        let relation = relation.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        let query = format!(
            "SELECT COUNT(*)::bigint AS count
               FROM graph_edges
              WHERE tenant_id = $1 AND relation = $2 AND {endpoint_column} = $3"
        );
        let count: i64 = client
            .query_one(&query, &[&self.tenant_id.as_str(), &relation, &node_id])
            .map_err(projector_error)?
            .get("count");
        Ok(vec![GraphRow::from([(
            "count".to_owned(),
            GraphValue::Int(count),
        )])])
    }

    fn node_exists(&self, kind: NodeKind, id: &str) -> Result<Vec<GraphRow>> {
        let kind = kind.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        let row = client
            .query_opt(
                "SELECT id
                   FROM graph_nodes
                  WHERE tenant_id = $1 AND kind = $2 AND id = $3",
                &[&self.tenant_id.as_str(), &kind, &id],
            )
            .map_err(projector_error)?;
        Ok(row.map_or_else(Vec::new, |_| {
            vec![GraphRow::from([(
                "id".to_owned(),
                GraphValue::String(id.to_owned()),
            )])]
        }))
    }

    fn node_rows(&self, kind: NodeKind) -> Result<Vec<GraphRow>> {
        let kind = kind.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        let rows = client
            .query(
                "SELECT id, properties
                   FROM graph_nodes
                  WHERE tenant_id = $1 AND kind = $2
                  ORDER BY id",
                &[&self.tenant_id.as_str(), &kind],
            )
            .map_err(projector_error)?;
        rows.iter().map(row_to_graph_row).collect()
    }

    fn node_by_id(&self, kind: NodeKind, id: &str) -> Result<Option<GraphRow>> {
        let kind = kind.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        client
            .query_opt(
                "SELECT id, properties
                   FROM graph_nodes
                  WHERE tenant_id = $1 AND kind = $2 AND id = $3",
                &[&self.tenant_id.as_str(), &kind, &id],
            )
            .map_err(projector_error)?
            .map(|row| row_to_graph_row(&row))
            .transpose()
    }

    fn relation_edges(&self, relation: RelationKind) -> Result<Vec<GraphRow>> {
        let relation = relation.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        let rows = client
            .query(
                "SELECT from_id, to_id
                   FROM graph_edges
                  WHERE tenant_id = $1 AND relation = $2
                  ORDER BY from_id, to_id",
                &[&self.tenant_id.as_str(), &relation],
            )
            .map_err(projector_error)?;
        Ok(rows
            .iter()
            .map(|row| {
                GraphRow::from([
                    ("from_id".to_owned(), GraphValue::String(row.get("from_id"))),
                    ("to_id".to_owned(), GraphValue::String(row.get("to_id"))),
                ])
            })
            .collect())
    }

    fn decision_by_id(&self, id: &str) -> Result<Vec<GraphRow>> {
        Ok(self
            .node_by_id(NodeKind::Decision, id)?
            .map_or_else(Vec::new, |row| vec![row]))
    }

    fn decision_topic_count(&self, topic: &str) -> Result<Vec<GraphRow>> {
        let mut client = self.pool.get().map_err(projector_error)?;
        let count: i64 = client
            .query_one(
                "SELECT COUNT(*)::bigint AS count
                   FROM graph_nodes
                  WHERE tenant_id = $1
                    AND kind = $2
                    AND (properties -> 'topic_keys') ? $3",
                &[
                    &self.tenant_id.as_str(),
                    &NodeKind::Decision.table_name(),
                    &topic,
                ],
            )
            .map_err(projector_error)?
            .get("count");
        Ok(vec![GraphRow::from([(
            "count".to_owned(),
            GraphValue::Int(count),
        )])])
    }

    fn decisions_for_topic(&self, topic: &str) -> Result<Vec<GraphRow>> {
        let mut client = self.pool.get().map_err(projector_error)?;
        let rows = client
            .query(
                "SELECT id, properties
                   FROM graph_nodes
                  WHERE tenant_id = $1
                    AND kind = $2
                    AND (properties -> 'topic_keys') ? $3
                  ORDER BY id
                  LIMIT 1000",
                &[
                    &self.tenant_id.as_str(),
                    &NodeKind::Decision.table_name(),
                    &topic,
                ],
            )
            .map_err(projector_error)?;
        rows.iter().map(row_to_graph_row).collect()
    }

    fn outgoing_neighbor_ids(
        &self,
        relation: RelationKind,
        from_id: &str,
        alias: &str,
    ) -> Result<Vec<GraphRow>> {
        let relation = relation.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        let rows = client
            .query(
                "SELECT to_id
                   FROM graph_edges
                  WHERE tenant_id = $1 AND relation = $2 AND from_id = $3
                  ORDER BY to_id",
                &[&self.tenant_id.as_str(), &relation, &from_id],
            )
            .map_err(projector_error)?;
        Ok(rows
            .iter()
            .map(|row| GraphRow::from([(alias.to_owned(), GraphValue::String(row.get("to_id")))]))
            .collect())
    }

    fn neighbor_pairs(
        &self,
        relation: RelationKind,
        id: &str,
        incoming: bool,
    ) -> Result<Vec<GraphRow>> {
        let relation = relation.table_name();
        let (select_column, endpoint_column) = if incoming {
            ("from_id", "to_id")
        } else {
            ("to_id", "from_id")
        };
        let query = format!(
            "SELECT {select_column} AS id, event_origin
               FROM graph_edges
              WHERE tenant_id = $1 AND relation = $2 AND {endpoint_column} = $3
              ORDER BY id"
        );
        let mut client = self.pool.get().map_err(projector_error)?;
        let rows = client
            .query(&query, &[&self.tenant_id.as_str(), &relation, &id])
            .map_err(projector_error)?;
        Ok(rows
            .iter()
            .map(|row| {
                let event_origin: Option<i64> = row.get("event_origin");
                GraphRow::from([
                    ("id".to_owned(), GraphValue::String(row.get("id"))),
                    (
                        "event_origin".to_owned(),
                        event_origin.map_or(GraphValue::Null, GraphValue::Int),
                    ),
                ])
            })
            .collect())
    }

    fn supersession_neighbors(
        &self,
        id: &str,
        direction: SupersessionDirection,
    ) -> Result<Vec<GraphRow>> {
        let (select_column, endpoint_column) = match direction {
            SupersessionDirection::Older => ("to_id", "from_id"),
            SupersessionDirection::Newer => ("from_id", "to_id"),
        };
        let query = format!(
            "SELECT {select_column} AS id
               FROM graph_edges
              WHERE tenant_id = $1 AND relation = $2 AND {endpoint_column} = $3
              ORDER BY id"
        );
        let relation = RelationKind::Supersedes.table_name();
        let mut client = self.pool.get().map_err(projector_error)?;
        let rows = client
            .query(&query, &[&self.tenant_id.as_str(), &relation, &id])
            .map_err(projector_error)?;
        Ok(rows
            .iter()
            .map(|row| GraphRow::from([("id".to_owned(), GraphValue::String(row.get("id")))]))
            .collect())
    }
}

enum SupersessionDirection {
    Older,
    Newer,
}

fn row_to_graph_row(row: &Row) -> Result<GraphRow> {
    let id: String = row.get("id");
    let properties: Value = row.get("properties");
    let mut graph_row = GraphRow::from([("id".to_owned(), GraphValue::String(id.clone()))]);
    let Value::Object(properties) = properties else {
        return Err(projector_error(format!("node {id} properties must be a JSON object")).into());
    };
    for (key, value) in properties {
        graph_row.insert(key, json_to_graph_value(value));
    }
    graph_row.insert("id".to_owned(), GraphValue::String(id));
    Ok(graph_row)
}

fn graph_properties_to_json(properties: &GraphProperties) -> Result<Value> {
    let mut object = Map::new();
    for (key, value) in properties {
        object.insert(key.clone(), graph_value_to_json(value)?); // ubs:ignore: JSON object keys must be owned when converting graph properties.
    }
    Ok(Value::Object(object))
}

fn graph_value_to_json(value: &GraphValue) -> Result<Value> {
    match value {
        GraphValue::Null => Ok(Value::Null),
        GraphValue::Bool(value) => Ok(Value::Bool(*value)),
        GraphValue::Int(value) => Ok(Value::Number(Number::from(*value))),
        GraphValue::Float(value) => Number::from_f64(*value).map(Value::Number).ok_or_else(|| {
            projector_error(format!("float graph value is not finite: {value}")).into()
        }),
        GraphValue::String(value) => Ok(Value::String(value.clone())),
        GraphValue::StringList(values) => Ok(Value::Array(
            values.iter().cloned().map(Value::String).collect(),
        )),
    }
}

fn json_to_graph_value(value: Value) -> GraphValue {
    match value {
        Value::Null => GraphValue::Null,
        Value::Bool(value) => GraphValue::Bool(value),
        Value::Number(value) => value.as_i64().map_or_else(
            || {
                value.as_u64().map_or_else(
                    || value.as_f64().map_or(GraphValue::Null, GraphValue::Float),
                    |value| {
                        i64::try_from(value)
                            .map(GraphValue::Int)
                            .unwrap_or(GraphValue::Float(value as f64))
                    },
                )
            },
            GraphValue::Int,
        ),
        Value::String(value) => GraphValue::String(value),
        Value::Array(values) => {
            if values.iter().all(|value| matches!(value, Value::String(_))) {
                let strings = values
                    .into_iter()
                    .filter_map(|value| match value {
                        Value::String(value) => Some(value),
                        _ => None,
                    })
                    .collect();
                GraphValue::StringList(strings)
            } else {
                GraphValue::String(Value::Array(values).to_string())
            }
        }
        Value::Object(value) => GraphValue::String(Value::Object(value).to_string()),
    }
}

fn optional_property_int(properties: &GraphProperties, key: &str) -> Option<i64> {
    match properties.get(key) {
        Some(GraphValue::Int(value)) => Some(*value),
        _ => None,
    }
}

fn optional_property_string(properties: &GraphProperties, key: &str) -> Option<String> {
    match properties.get(key) {
        Some(GraphValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn search_document(id: &str, properties: &GraphProperties) -> String {
    let mut parts = vec![id.to_owned()];
    for (key, value) in properties {
        parts.push(key.clone()); // ubs:ignore: search document owns projected property names for tsvector indexing.
        match value {
            GraphValue::Null => {}
            GraphValue::Bool(value) => parts.push(value.to_string()), // ubs:ignore: scalar text conversion feeds Postgres tsvector indexing.
            GraphValue::Int(value) => parts.push(value.to_string()), // ubs:ignore: scalar text conversion feeds Postgres tsvector indexing.
            GraphValue::Float(value) => parts.push(value.to_string()), // ubs:ignore: scalar text conversion feeds Postgres tsvector indexing.
            GraphValue::String(value) => parts.push(value.clone()), // ubs:ignore: search document owns projected property text for tsvector indexing.
            GraphValue::StringList(values) => parts.extend(values.clone()), // ubs:ignore: search document owns projected list text for tsvector indexing.
        }
    }
    parts.join(" ")
}

fn query_relation(cypher: &str) -> Result<RelationKind> {
    for relation in RelationKind::ALL {
        if contains_quoted_identifier(cypher, relation.table_name()) {
            return Ok(relation);
        }
    }
    Err(projector_error(format!("unknown relation in query: {cypher}")).into())
}

fn query_node_kind(cypher: &str) -> Result<NodeKind> {
    for kind in NodeKind::ALL {
        if contains_quoted_identifier(cypher, kind.table_name()) {
            return Ok(kind);
        }
    }
    Err(projector_error(format!("unknown node kind in query: {cypher}")).into())
}

fn required_param_string<'a>(params: &'a GraphParams, key: &str) -> Result<&'a str> {
    match params.get(key) {
        Some(GraphValue::String(value)) => Ok(value),
        _ => Err(projector_error(format!("missing string param: {key}")).into()),
    }
}

fn neighbor_alias(cypher: &str) -> Result<&'static str> {
    if cypher.contains("AS option_id") {
        Ok("option_id")
    } else if cypher.contains("AS evidence_id") {
        Ok("evidence_id")
    } else if cypher.contains("AS hypothesis_id") {
        Ok("hypothesis_id")
    } else {
        Err(projector_error(format!("unknown neighbor alias in query: {cypher}")).into())
    }
}

fn contains_quoted_identifier(cypher: &str, identifier: &str) -> bool {
    cypher
        .split('`')
        .skip(1)
        .step_by(2)
        .any(|quoted| quoted == identifier)
}

fn projector_error(error: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(error.to_string())
}

#[cfg(test)]
mod tests;
