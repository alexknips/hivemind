//! Postgres-backed graph projector for the shared-backend deployment.

use std::str::FromStr;

use postgres::{Client, Config};
use postgres_native_tls::MakeTlsConnector;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use serde_json::Value as JsonValue;

use crate::error::ProjectorError;
use crate::Result;

use super::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};

type PgManager = PostgresConnectionManager<MakeTlsConnector>;
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
        let tls = MakeTlsConnector::new(native_tls::TlsConnector::new().map_err(pg_error)?);
        let manager = PostgresConnectionManager::new(config, tls);
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

    // ── Decision by id (title/rationale/topic_keys, or existence+event_origin
    //    check — outcome.rs's get_decision_outcome existence probe reads only
    //    id/event_origin but the row carries every stored property either way) ──
    if cypher.contains("LIMIT 1")
        && (cypher.contains("d.title AS title")
            || cypher.contains("d.event_origin AS event_origin"))
    {
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

    // ── outcome.rs's get_decision_outcome query shapes ────────────────────────
    // Not covered by the handlers above; see hivemind-kj0i.

    // query_superseder: newest SUPERSEDES edge pointing at this decision.
    if cypher.contains("RETURN newer.id AS superseder_id, r.event_origin AS edge_origin") {
        return query_superseder(client, tenant_id, params);
    }

    // query_refuted_premises: hypotheses this decision premises on (via CHOSE->PREMISED_ON
    // or PREMISED_ON_DIRECT) that have also been REFUTES-ed by some Evidence.
    if cypher.contains("REFUTES") && cypher.contains("hypothesis_id") {
        return query_refuted_premises(client, tenant_id, params);
    }

    // query_contested: ACCEPTED_BY / REJECTED_BY edge counts in one call.
    if cypher.contains("AS accepted_count") {
        return query_contested(client, tenant_id, params);
    }

    // query_has_options / query_has_evidence: outgoing edge count of one relation kind.
    if cypher.contains("RETURN count(*) AS cnt") {
        return query_has_relation(client, tenant_id, cypher, params);
    }

    // get_decision_quality_candidates / get_decision_context_candidates: bulk decision dump,
    // optionally floored by $since. Both context.rs's and outcome.rs's bulk queries share this
    // ORDER BY tail (differing only in which columns they RETURN); this handler returns every
    // stored property either way, same as query_decision_by_id below.
    if cypher.contains("ORDER BY d.event_origin, d.id") {
        return query_decisions_with_origin(client, tenant_id, params);
    }

    // ── context.rs's get_decision_context and brief.rs's get_decision_brief query shapes ──
    // Not covered by the handlers above; see hivemind-ookw.

    // brief.rs's resolve_option_label: single Option node lookup by id, returning its label
    // (or Null if the node has no `label` property — the row still exists as long as the
    // Option node itself exists, matching memory.rs's `graph_property_or_default`).
    if cypher.contains("RETURN o.label AS label") {
        return query_option_label(client, tenant_id, params);
    }

    // query_proposer / query_actor_ids_by_edge: decision -[relation]-> Actor, returning the
    // actor id and its "kind" (human/agent/unknown, stamped by upsert_actor). The two callers
    // differ only in whether the cypher ends "LIMIT 1;" (proposer) or "ORDER BY a.id;"
    // (acceptors) — both are handled by the same query, truncated when LIMIT 1 is present.
    if cypher.contains("RETURN a.id AS actor_id, a.kind AS kind") {
        return query_actor_kind_pairs(client, tenant_id, cypher, params);
    }

    // query_hypothesis_count: hypotheses this decision premises on (via CHOSE->PREMISED_ON or
    // PREMISED_ON_DIRECT), unfiltered by REFUTES — distinct from query_refuted_premises above,
    // which requires "REFUTES" in the cypher text; this one never contains that substring.
    if cypher.contains("UNION") && cypher.contains(" AS hid") {
        return query_hypothesis_ids(client, tenant_id, params, "hid");
    }

    // shared.rs's premised_on_hypothesis_ids: same premised-on-hypothesis computation as
    // query_hypothesis_count above, but under the "hypothesis_id" alias — consumed as a list
    // (not just a count) by get_decision, get_decision_brief, and situational.rs. Distinct
    // cypher text from query_hypothesis_count's (" AS hid" vs "hypothesis_id"), so the two
    // conditions never both match; REFUTES-filtered callers already returned above.
    if cypher.contains("UNION") && cypher.contains("hypothesis_id") {
        return query_hypothesis_ids(client, tenant_id, params, "hypothesis_id");
    }

    // get_decision_context's single-decision fetch: any remaining
    // "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, ..." shape returns the id plus every
    // stored property, same as query_decision_by_id above — the caller reads only the specific
    // keys it asked for. Placed last since it's the most general remaining single-id shape.
    if cypher.contains("MATCH (d:`Decision` {id: $id})") && cypher.contains("RETURN d.id AS id") {
        return query_decision_by_id(client, tenant_id, params);
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

// ── outcome.rs query implementations (hivemind-kj0i) ──────────────────────────

fn query_superseder(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;
    let row = client
        .query_opt(
            "SELECT from_id, event_origin FROM hm_edges
             WHERE tenant_id=$1 AND relation_kind='SUPERSEDES' AND to_id=$2
             ORDER BY event_origin DESC NULLS LAST
             LIMIT 1",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    Ok(row
        .map(|row| {
            let superseder_id: String = row.get(0);
            let edge_origin: Option<i64> = row.get(1);
            GraphRow::from([
                (
                    "superseder_id".to_owned(),
                    GraphValue::String(superseder_id),
                ),
                (
                    "edge_origin".to_owned(),
                    edge_origin.map_or(GraphValue::Null, GraphValue::Int),
                ),
            ])
        })
        .into_iter()
        .collect())
}

/// Hypotheses this decision premises on (via CHOSE->PREMISED_ON or PREMISED_ON_DIRECT)
/// that have also been REFUTES-ed by some Evidence. Mirrors memory.rs's in-process set
/// logic rather than a single SQL join — tenant-scoped edge volumes are small and this
/// keeps the two backends' selection logic easy to compare line-for-line.
fn query_refuted_premises(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;

    let refuted_hypotheses: std::collections::BTreeSet<String> = client
        .query(
            "SELECT DISTINCT to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='REFUTES'",
            &[&tenant_id],
        )
        .map_err(pg_error)?
        .into_iter()
        .map(|row| row.get::<_, String>(0))
        .collect();

    let chosen_options: std::collections::BTreeSet<String> = client
        .query(
            "SELECT to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='CHOSE' AND from_id=$2",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?
        .into_iter()
        .map(|row| row.get::<_, String>(0))
        .collect();

    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    if !chosen_options.is_empty() {
        let premised_rows = client
            .query(
                "SELECT from_id, to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='PREMISED_ON'",
                &[&tenant_id],
            )
            .map_err(pg_error)?;
        ids.extend(premised_rows.into_iter().filter_map(|row| {
            let from_id: String = row.get(0);
            let to_id: String = row.get(1);
            (chosen_options.contains(&from_id) && refuted_hypotheses.contains(&to_id))
                .then_some(to_id)
        }));
    }

    let direct_rows = client
        .query(
            "SELECT to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='PREMISED_ON_DIRECT' AND from_id=$2",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    ids.extend(direct_rows.into_iter().filter_map(|row| {
        let to_id: String = row.get(0);
        refuted_hypotheses.contains(&to_id).then_some(to_id)
    }));

    Ok(ids
        .into_iter()
        .map(|hyp_id| GraphRow::from([("hypothesis_id".to_owned(), GraphValue::String(hyp_id))]))
        .collect())
}

fn query_contested(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;
    let accepted = count_outgoing(client, tenant_id, "ACCEPTED_BY", id)?;
    let rejected = count_outgoing(client, tenant_id, "REJECTED_BY", id)?;
    Ok(vec![GraphRow::from([
        ("accepted_count".to_owned(), GraphValue::Int(accepted)),
        ("rejected_count".to_owned(), GraphValue::Int(rejected)),
    ])])
}

fn query_has_relation(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let relation = parse_relation(cypher)?;
    let id = required_string_param(params, "id")?;
    let count = count_outgoing(client, tenant_id, relation.table_name(), id)?;
    Ok(vec![GraphRow::from([(
        "cnt".to_owned(),
        GraphValue::Int(count),
    )])])
}

fn count_outgoing(
    client: &mut Client,
    tenant_id: &str,
    relation: &str,
    from_id: &str,
) -> Result<i64> {
    client
        .query_one(
            "SELECT COUNT(*)::bigint FROM hm_edges
             WHERE tenant_id=$1 AND relation_kind=$2 AND from_id=$3",
            &[&tenant_id, &relation, &from_id],
        )
        .map_err(pg_error)
        .map(|row| row.get::<_, i64>(0))
}

fn query_decisions_with_origin(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let since = match params.get("since") {
        Some(GraphValue::Int(value)) => Some(*value),
        _ => None,
    };
    let rows = if let Some(since) = since {
        client
            .query(
                "SELECT node_id, properties FROM hm_nodes
                 WHERE tenant_id=$1 AND node_kind='Decision'
                   AND COALESCE((properties->>'event_origin')::bigint, 0) >= $2
                 ORDER BY COALESCE((properties->>'event_origin')::bigint, 0), node_id",
                &[&tenant_id, &since],
            )
            .map_err(pg_error)?
    } else {
        client
            .query(
                "SELECT node_id, properties FROM hm_nodes
                 WHERE tenant_id=$1 AND node_kind='Decision'
                 ORDER BY COALESCE((properties->>'event_origin')::bigint, 0), node_id",
                &[&tenant_id],
            )
            .map_err(pg_error)?
    };
    Ok(rows
        .into_iter()
        .map(|row| {
            let node_id: String = row.get(0);
            let props: JsonValue = row.get(1);
            json_props_to_row(&node_id, &props)
        })
        .collect())
}

// ── context.rs / brief.rs query implementations (hivemind-ookw) ────────────────

/// Label of a single `Option` node by id, or `None` if the node doesn't exist. When the node
/// exists but has no `label` property, returns a row with `label: Null` — same distinction as
/// memory.rs's `graph_property_or_default` (missing node vs. missing property are different).
fn query_option_label(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;
    let row = client
        .query_opt(
            "SELECT properties FROM hm_nodes
             WHERE tenant_id=$1 AND node_kind='Option' AND node_id=$2
             LIMIT 1",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    Ok(row
        .map(|row| {
            let props: JsonValue = row.get(0);
            let label = props
                .get("label")
                .and_then(json_to_graph_value)
                .unwrap_or(GraphValue::Null);
            GraphRow::from([("label".to_owned(), label)])
        })
        .into_iter()
        .collect())
}

/// `(actor_id, kind)` pairs for actors connected via one relationship label (PROPOSED_BY or
/// ACCEPTED_BY), sourced from `hm_edges` joined against `hm_nodes` for the actor's stored
/// `kind` property. `kind` is `Null` when the actor node is missing or has no `kind` key,
/// matching memory.rs's `.unwrap_or(GraphValue::Null)` fallback. Truncated to one row when the
/// cypher carries `LIMIT 1` (the single-proposer lookup); otherwise every match is returned,
/// ordered by actor id (the multi-acceptor lookup).
fn query_actor_kind_pairs(
    client: &mut Client,
    tenant_id: &str,
    cypher: &str,
    params: &GraphParams,
) -> Result<Vec<GraphRow>> {
    let relation = parse_relation(cypher)?;
    let id = required_string_param(params, "id")?;
    let rows = client
        .query(
            "SELECT e.to_id, n.properties->>'kind' FROM hm_edges e
             LEFT JOIN hm_nodes n
               ON n.tenant_id = e.tenant_id AND n.node_kind = 'Actor' AND n.node_id = e.to_id
             WHERE e.tenant_id = $1 AND e.relation_kind = $2 AND e.from_id = $3
             ORDER BY e.to_id",
            &[&tenant_id, &relation.table_name(), &id],
        )
        .map_err(pg_error)?;
    let mut result: Vec<GraphRow> = rows
        .into_iter()
        .map(|row| {
            let actor_id: String = row.get(0);
            let kind: Option<String> = row.get(1);
            GraphRow::from([
                ("actor_id".to_owned(), GraphValue::String(actor_id)),
                (
                    "kind".to_owned(),
                    kind.map_or(GraphValue::Null, GraphValue::String),
                ),
            ])
        })
        .collect();
    if cypher.contains("LIMIT 1") {
        result.truncate(1);
    }
    Ok(result)
}

/// Hypotheses this decision premises on (via CHOSE->PREMISED_ON or PREMISED_ON_DIRECT),
/// unfiltered by REFUTES. Mirrors `query_refuted_premises` above minus the refuted-hypothesis
/// filter — kept as a separate function (rather than a shared helper with a filter flag) so
/// each stays a direct, line-for-line match against its own memory.rs counterpart. `alias`
/// selects the returned column name ("hid" for context.rs's count-only caller, "hypothesis_id"
/// for shared.rs's list caller) — same rows, different callers read different keys.
fn query_hypothesis_ids(
    client: &mut Client,
    tenant_id: &str,
    params: &GraphParams,
    alias: &str,
) -> Result<Vec<GraphRow>> {
    let id = required_string_param(params, "id")?;

    let chosen_options: std::collections::BTreeSet<String> = client
        .query(
            "SELECT to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='CHOSE' AND from_id=$2",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?
        .into_iter()
        .map(|row| row.get::<_, String>(0))
        .collect();

    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    if !chosen_options.is_empty() {
        let premised_rows = client
            .query(
                "SELECT from_id, to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='PREMISED_ON'",
                &[&tenant_id],
            )
            .map_err(pg_error)?;
        ids.extend(premised_rows.into_iter().filter_map(|row| {
            let from_id: String = row.get(0);
            let to_id: String = row.get(1);
            chosen_options.contains(&from_id).then_some(to_id)
        }));
    }

    let direct_rows = client
        .query(
            "SELECT to_id FROM hm_edges WHERE tenant_id=$1 AND relation_kind='PREMISED_ON_DIRECT' AND from_id=$2",
            &[&tenant_id, &id],
        )
        .map_err(pg_error)?;
    ids.extend(direct_rows.into_iter().map(|row| row.get::<_, String>(0)));

    Ok(ids
        .into_iter()
        .map(|hid| GraphRow::from([(alias.to_owned(), GraphValue::String(hid))]))
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
