use std::str::FromStr;

use postgres::{Client, Config, NoTls};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use serde_json::Value as JsonValue;

use crate::error::ProjectorError;
use crate::Result;

use super::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};

type PgManager = PostgresConnectionManager<NoTls>;
type PgPool = Pool<PgManager>;

const DEFAULT_POOL_SIZE: u32 = 8;

/// Postgres-backed GraphView — tenant-scoped projection of the graph.
#[derive(Clone)]
pub struct PostgresGraphView {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresGraphView {
    pub fn connect(database_url: &str, tenant_id: impl Into<String>) -> Result<Self> {
        Self::connect_with_pool_size(database_url, tenant_id, DEFAULT_POOL_SIZE)
    }

    pub fn connect_with_pool_size(
        database_url: &str,
        tenant_id: impl Into<String>,
        max_size: u32,
    ) -> Result<Self> {
        let tenant_id = validate_tenant_id(tenant_id.into())?;
        let config = Config::from_str(database_url).map_err(pg_error)?;
        let manager = PostgresConnectionManager::new(config, NoTls);
        let pool = Pool::builder()
            .max_size(max_size)
            .build(manager)
            .map_err(pg_error)?;
        let view = Self { pool, tenant_id };
        view.initialize_schema()?;
        Ok(view)
    }

    pub fn for_tenant(&self, tenant_id: impl Into<String>) -> Result<Self> {
        Ok(Self {
            pool: self.pool.clone(),
            tenant_id: validate_tenant_id(tenant_id.into())?,
        })
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    fn initialize_schema(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(pg_error)?;
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS hm_nodes (
                    tenant_id   text    NOT NULL,
                    node_kind   text    NOT NULL,
                    node_id     text    NOT NULL,
                    properties  jsonb   NOT NULL DEFAULT '{}',
                    PRIMARY KEY (tenant_id, node_kind, node_id)
                );
                CREATE INDEX IF NOT EXISTS hm_nodes_tenant_kind_idx
                    ON hm_nodes (tenant_id, node_kind);
                CREATE TABLE IF NOT EXISTS hm_edges (
                    tenant_id     text    NOT NULL,
                    relation_kind text    NOT NULL,
                    from_id       text    NOT NULL,
                    to_id         text    NOT NULL,
                    event_origin  bigint,
                    PRIMARY KEY (tenant_id, relation_kind, from_id, to_id)
                );
                CREATE INDEX IF NOT EXISTS hm_edges_from_idx
                    ON hm_edges (tenant_id, relation_kind, from_id);
                CREATE INDEX IF NOT EXISTS hm_edges_to_idx
                    ON hm_edges (tenant_id, relation_kind, to_id);",
            )
            .map_err(pg_error)?;
        Ok(())
    }
}

impl GraphView for PostgresGraphView {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
        let props = graph_properties_to_json(properties);
        let mut client = self.pool.get().map_err(pg_error)?;
        client
            .execute(
                "INSERT INTO hm_nodes (tenant_id, node_kind, node_id, properties)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (tenant_id, node_kind, node_id)
                 DO UPDATE SET properties = hm_nodes.properties || EXCLUDED.properties",
                &[&self.tenant_id, &kind.table_name(), &id, &props],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()> {
        let event_origin: Option<i64> = match properties.get("event_origin") {
            Some(GraphValue::Int(n)) => Some(*n),
            _ => None,
        };
        let mut client = self.pool.get().map_err(pg_error)?;
        client
            .execute(
                "INSERT INTO hm_edges (tenant_id, relation_kind, from_id, to_id, event_origin)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (tenant_id, relation_kind, from_id, to_id) DO NOTHING",
                &[
                    &self.tenant_id,
                    &kind.table_name(),
                    &from_id,
                    &to_id,
                    &event_origin,
                ],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        let mut client = self.pool.get().map_err(pg_error)?;
        dispatch_query(&mut client, &self.tenant_id, cypher, params)
    }

    fn wipe(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(pg_error)?;
        client
            .execute(
                "DELETE FROM hm_nodes WHERE tenant_id = $1",
                &[&self.tenant_id],
            )
            .map_err(pg_error)?;
        client
            .execute(
                "DELETE FROM hm_edges WHERE tenant_id = $1",
                &[&self.tenant_id],
            )
            .map_err(pg_error)?;
        Ok(())
    }
}

// ── Query dispatch ────────────────────────────────────────────────────────────

fn dispatch_query(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    // ── Edge count ───────────────────────────────────────────────────────────
    if cypher.contains("RETURN count(rel) AS count;") {
        return query_edge_count(client, tenant_id, cypher, params);
    }

    // ── Decision count by topic ──────────────────────────────────────────────
    if cypher.contains("RETURN count(d) AS count;") {
        return query_topic_count(client, tenant_id, params);
    }

    // ── Decision by id (title/rationale/topic_keys) ──────────────────────────
    if cypher.contains("d.title AS title") && cypher.contains("LIMIT 1") {
        return query_decision_by_id(client, tenant_id, params);
    }

    // ── Node existence check LIMIT 1 ─────────────────────────────────────────
    if cypher.contains("RETURN node.id AS id LIMIT 1;")
        || cypher.contains("RETURN d.id AS id LIMIT 1;")
    {
        return query_node_exists(client, tenant_id, cypher, params);
    }

    // ── Supersession edges (outgoing) ─────────────────────────────────────────
    if cypher.contains("MATCH (d:`Decision` {id: $id})-[:`SUPERSEDES`]->(other:`Decision`)") {
        return query_supersedes_outgoing(client, tenant_id, params);
    }

    // ── Supersession edges (incoming) ─────────────────────────────────────────
    if cypher.contains("MATCH (other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision` {id: $id})") {
        return query_supersedes_incoming(client, tenant_id, params);
    }

    // ── Neighbor pairs with event_origin ─────────────────────────────────────
    if cypher.contains("RETURN b.id AS id, r.event_origin AS event_origin") {
        return query_neighbor_pairs(client, tenant_id, cypher, params);
    }

    // ── Neighbor ids (alias: option_id / evidence_id / hypothesis_id) ────────
    if cypher.contains("RETURN n.id AS") {
        return query_neighbor_ids(client, tenant_id, cypher, params);
    }

    // ── Edge list (from/to) ───────────────────────────────────────────────────
    if cypher.contains("RETURN from.id AS from_id, to.id AS to_id") {
        return query_edge_list(client, tenant_id, cypher, params);
    }

    // ── Topic-filtered decisions ──────────────────────────────────────────────
    if cypher.contains("WHERE $topic IN d.topic_keys") {
        return query_topic_decisions(client, tenant_id, params);
    }

    // ── All nodes of a kind (must follow LIMIT 1 / specific checks) ──────────
    if cypher.contains("RETURN node.id AS id") {
        return query_all_nodes(client, tenant_id, cypher, params);
    }

    Err(projection_error(format!("unsupported cypher query: {cypher}")).into())
}

// ── Query implementations ─────────────────────────────────────────────────────

fn query_edge_count(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let relation = parse_relation(cypher)?;
    let id = required_string_param(params, "id")?;
    let incoming = cypher.contains("<-[rel:");
    let count: i64 = if incoming {
        client
            .query_one(
                "SELECT COUNT(*)::bigint FROM hm_edges
                 WHERE tenant_id=$1 AND relation_kind=$2 AND to_id=$3",
                &[&tenant_id, &relation.table_name(), &id],
            )
            .map_err(pg_error)?
            .get(0)
    } else {
        client
            .query_one(
                "SELECT COUNT(*)::bigint FROM hm_edges
                 WHERE tenant_id=$1 AND relation_kind=$2 AND from_id=$3",
                &[&tenant_id, &relation.table_name(), &id],
            )
            .map_err(pg_error)?
            .get(0)
    };
    Ok(vec![GraphRow::from([(
        "count".to_owned(),
        GraphValue::Int(count),
    )])])
}

fn query_topic_count(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let topic = required_string_param(params, "topic")?;
    let count: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM hm_nodes
             WHERE tenant_id=$1 AND node_kind='Decision'
               AND properties->'topic_keys' @> jsonb_build_array($2::text)",
            &[&tenant_id, &topic],
        )
        .map_err(pg_error)?
        .get(0);
    Ok(vec![GraphRow::from([(
        "count".to_owned(),
        GraphValue::Int(count),
    )])])
}

fn query_decision_by_id(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;
    let row = client
        .query_opt(
            "SELECT node_id, properties FROM hm_nodes
             WHERE tenant_id=$1 AND node_kind='Decision' AND node_id=$2
             LIMIT 1",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    Ok(row
        .map(|row| {
            let node_id: String = row.get(0);
            let props: JsonValue = row.get(1);
            json_props_to_row(&node_id, &props)
        })
        .into_iter()
        .collect())
}

fn query_node_exists(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let kind = if cypher.contains("node.id") {
        parse_node_kind(cypher)?
    } else {
        NodeKind::Decision
    };
    let id = required_string_param(params, "id")?;
    let exists = client
        .query_opt(
            "SELECT node_id FROM hm_nodes
             WHERE tenant_id=$1 AND node_kind=$2 AND node_id=$3
             LIMIT 1",
            &[&tenant_id, &kind.table_name(), &id],
        )
        .map_err(pg_error)?
        .is_some();
    if exists {
        Ok(vec![GraphRow::from([(
            "id".to_owned(),
            GraphValue::String(id.to_owned()),
        )])])
    } else {
        Ok(Vec::new())
    }
}

fn query_supersedes_outgoing(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;
    let rows = client
        .query(
            "SELECT to_id FROM hm_edges
             WHERE tenant_id=$1 AND relation_kind='SUPERSEDES' AND from_id=$2
             ORDER BY to_id",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let to_id: String = row.get(0);
            GraphRow::from([("id".to_owned(), GraphValue::String(to_id))])
        })
        .collect())
}

fn query_supersedes_incoming(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;
    let rows = client
        .query(
            "SELECT from_id FROM hm_edges
             WHERE tenant_id=$1 AND relation_kind='SUPERSEDES' AND to_id=$2
             ORDER BY from_id",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let from_id: String = row.get(0);
            GraphRow::from([("id".to_owned(), GraphValue::String(from_id))])
        })
        .collect())
}

fn query_neighbor_pairs(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let relation = parse_relation(cypher)?;
    let id = required_string_param(params, "id")?;
    let incoming = cypher.contains("<-[r:");
    let rows = if incoming {
        client
            .query(
                "SELECT from_id, event_origin FROM hm_edges
                 WHERE tenant_id=$1 AND relation_kind=$2 AND to_id=$3
                 ORDER BY from_id",
                &[&tenant_id, &relation.table_name(), &id],
            )
            .map_err(pg_error)?
    } else {
        client
            .query(
                "SELECT to_id, event_origin FROM hm_edges
                 WHERE tenant_id=$1 AND relation_kind=$2 AND from_id=$3
                 ORDER BY to_id",
                &[&tenant_id, &relation.table_name(), &id],
            )
            .map_err(pg_error)?
    };
    Ok(rows
        .into_iter()
        .map(|row| {
            let neighbor_id: String = row.get(0);
            let event_origin: Option<i64> = row.get(1);
            GraphRow::from([
                ("id".to_owned(), GraphValue::String(neighbor_id)),
                (
                    "event_origin".to_owned(),
                    event_origin
                        .map(GraphValue::Int)
                        .unwrap_or(GraphValue::Null),
                ),
            ])
        })
        .collect())
}

fn query_neighbor_ids(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let relation = parse_relation(cypher)?;
    let id = required_string_param(params, "id")?;
    let alias = if cypher.contains("AS option_id") {
        "option_id"
    } else if cypher.contains("AS evidence_id") {
        "evidence_id"
    } else if cypher.contains("AS hypothesis_id") {
        "hypothesis_id"
    } else {
        return Err(projection_error(format!("unknown neighbor alias in: {cypher}")).into());
    };
    let rows = client
        .query(
            "SELECT to_id FROM hm_edges
             WHERE tenant_id=$1 AND relation_kind=$2 AND from_id=$3
             ORDER BY to_id",
            &[&tenant_id, &relation.table_name(), &id],
        )
        .map_err(pg_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let to_id: String = row.get(0);
            GraphRow::from([(alias.to_owned(), GraphValue::String(to_id))])
        })
        .collect())
}

fn query_edge_list(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let _ = params;
    let relation = parse_relation(cypher)?;
    let rows = client
        .query(
            "SELECT from_id, to_id FROM hm_edges
             WHERE tenant_id=$1 AND relation_kind=$2
             ORDER BY from_id, to_id",
            &[&tenant_id, &relation.table_name()],
        )
        .map_err(pg_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let from_id: String = row.get(0);
            let to_id: String = row.get(1);
            GraphRow::from([
                ("from_id".to_owned(), GraphValue::String(from_id)),
                ("to_id".to_owned(), GraphValue::String(to_id)),
            ])
        })
        .collect())
}

fn query_topic_decisions(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let topic = required_string_param(params, "topic")?;
    let rows = client
        .query(
            "SELECT node_id, properties FROM hm_nodes
             WHERE tenant_id=$1 AND node_kind='Decision'
               AND properties->'topic_keys' @> jsonb_build_array($2::text)
             ORDER BY node_id
             LIMIT 1000",
            &[&tenant_id, &topic],
        )
        .map_err(pg_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let node_id: String = row.get(0);
            let props: JsonValue = row.get(1);
            json_props_to_row(&node_id, &props)
        })
        .collect())
}

fn query_all_nodes(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let _ = params;
    let kind = parse_node_kind(cypher)?;
    let rows = client
        .query(
            "SELECT node_id, properties FROM hm_nodes
             WHERE tenant_id=$1 AND node_kind=$2
             ORDER BY node_id",
            &[&tenant_id, &kind.table_name()],
        )
        .map_err(pg_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let node_id: String = row.get(0);
            let props: JsonValue = row.get(1);
            json_props_to_row(&node_id, &props)
        })
        .collect())
}

// ── Cypher parsing helpers ────────────────────────────────────────────────────

fn parse_relation(cypher: &str) -> Result<RelationKind> {
    for rel in RelationKind::ALL {
        if backtick_quoted(cypher, rel.table_name()) {
            return Ok(rel);
        }
    }
    Err(projection_error(format!("no relation found in: {cypher}")).into())
}

fn parse_node_kind(cypher: &str) -> Result<NodeKind> {
    for kind in NodeKind::ALL {
        if backtick_quoted(cypher, kind.table_name()) {
            return Ok(kind);
        }
    }
    Err(projection_error(format!("no node kind found in: {cypher}")).into())
}

fn backtick_quoted(cypher: &str, identifier: &str) -> bool {
    cypher
        .split('`')
        .skip(1)
        .step_by(2)
        .any(|s| s == identifier)
}

fn required_string_param<'a>(params: &'a GraphParams, key: &str) -> Result<&'a str> {
    match params.get(key) {
        Some(GraphValue::String(s)) => Ok(s.as_str()),
        _ => Err(projection_error(format!("missing string param: {key}")).into()),
    }
}

// ── JSON ↔ GraphValue conversion ─────────────────────────────────────────────

fn json_props_to_row(id: &str, props: &JsonValue) -> GraphRow {
    let mut row = GraphRow::from([("id".to_owned(), GraphValue::String(id.to_owned()))]);
    if let Some(obj) = props.as_object() {
        for (key, value) in obj {
            if let Some(gv) = json_to_graph_value(value) {
                row.insert(key.clone(), gv);
            }
        }
    }
    row
}

fn graph_properties_to_json(props: &GraphProperties) -> JsonValue {
    let mut map = serde_json::Map::with_capacity(props.len());
    for (k, v) in props {
        map.insert(k.clone(), graph_value_to_json(v));
    }
    JsonValue::Object(map)
}

fn graph_value_to_json(value: &GraphValue) -> JsonValue {
    match value {
        GraphValue::Null => JsonValue::Null,
        GraphValue::Bool(b) => JsonValue::Bool(*b),
        GraphValue::Int(n) => JsonValue::Number((*n).into()),
        GraphValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        GraphValue::String(s) => JsonValue::String(s.clone()),
        GraphValue::StringList(v) => {
            JsonValue::Array(v.iter().map(|s| JsonValue::String(s.clone())).collect())
        }
    }
}

fn json_to_graph_value(value: &JsonValue) -> Option<GraphValue> {
    match value {
        JsonValue::Null => Some(GraphValue::Null),
        JsonValue::Bool(b) => Some(GraphValue::Bool(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(GraphValue::Int(i))
            } else {
                n.as_f64().map(GraphValue::Float)
            }
        }
        JsonValue::String(s) => Some(GraphValue::String(s.clone())),
        JsonValue::Array(arr) => {
            let strings: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            Some(GraphValue::StringList(strings))
        }
        JsonValue::Object(_) => None,
    }
}

// ── Error helpers ─────────────────────────────────────────────────────────────

fn projection_error(message: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(message.to_string())
}

fn pg_error(error: impl std::fmt::Display) -> crate::HivemindError {
    projection_error(error).into()
}

fn validate_tenant_id(tenant_id: String) -> Result<String> {
    if tenant_id.trim().is_empty() {
        return Err(projection_error("tenant_id is required").into());
    }
    Ok(tenant_id)
}

#[cfg(test)]
mod tests;
