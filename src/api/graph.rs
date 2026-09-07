//! `GET /v1/graph` — full decision graph for a tenant, plus the projected
//! [`MemoryGraph`] cache shared by every read handler across the API
//! (avoids replaying the whole ledger on each request).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::events::TenantId;
use crate::ledger::EventLedger;
use crate::projector::{
    memory::MemoryGraph, project_from_ledger_for_tenant, GraphParams, GraphRow, GraphValue,
    GraphView, NodeKind, RelationKind,
};

use super::auth::extract_ctx;
use super::{
    respond, ApiBackend, ApiError, ApiLedger, ApiRequestCtx, ApiResult, AppState, GraphCache,
};

/// A single node in the decision graph, as returned by GET /v1/graph.
#[derive(Debug, serde::Serialize)]
struct GraphNode {
    id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

/// A directed edge in the decision graph.
#[derive(Clone, Debug, serde::Serialize)]
struct GraphEdge {
    from: String,
    to: String,
    relation: String,
}

/// Full decision graph for a tenant, in the shape expected by the SPA.
#[derive(Debug, serde::Serialize)]
struct GraphData {
    decisions: Vec<serde_json::Value>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

// ---------------------------------------------------------------------------
// Graph handler
// ---------------------------------------------------------------------------

pub(super) async fn graph_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || graph_blocking(&backend, &ctx, &cache)).await;
    respond(result, StatusCode::OK)
}

fn graph_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}

fn graph_blocking(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    cache: &GraphCache,
) -> ApiResult<serde_json::Value> {
    let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
    let graph = get_cached_graph(&ledger, &ctx.tenant_id, cache)?;

    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut decisions: Vec<serde_json::Value> = Vec::new();

    for kind in NodeKind::ALL {
        let kind_name = kind.table_name();
        let rows = graph
            .query(&graph_node_query(kind), &GraphParams::new())
            .map_err(graph_err)?;
        for row in rows {
            let id = row_string(&row, "id").unwrap_or_default();
            let label = row
                .get("title")
                .or_else(|| row.get("label"))
                .or_else(|| row.get("statement"))
                .or_else(|| row.get("content"))
                .and_then(|v| match v {
                    GraphValue::String(s) => Some(s.into()),
                    _ => None,
                });
            if matches!(kind, NodeKind::Decision) {
                let obj: serde_json::Map<String, serde_json::Value> = row
                    .iter()
                    .map(|(k, v)| (k.into(), graph_value_to_json(v)))
                    .collect();
                decisions.push(serde_json::Value::Object(obj));
            }
            nodes.push(GraphNode {
                id: [kind_name, ":", &id].concat(),
                kind: kind_name.into(),
                label,
            });
        }
    }

    let mut edges: Vec<GraphEdge> = Vec::new();

    for relation in RelationKind::ALL {
        let (from_kind, to_kind) = relation.endpoints();
        let from_name = from_kind.table_name();
        let to_name = to_kind.table_name();
        let rel_name = relation.table_name();
        let q = [
            "MATCH (from:`",
            from_name,
            "`)-[rel:`",
            rel_name,
            "`]->(to:`",
            to_name,
            "`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        ]
        .concat();
        let rows = graph.query(&q, &GraphParams::new()).map_err(graph_err)?;
        for row in rows {
            edges.push(GraphEdge {
                from: [
                    from_name,
                    ":",
                    &row_string(&row, "from_id").unwrap_or_default(),
                ]
                .concat(),
                to: [to_name, ":", &row_string(&row, "to_id").unwrap_or_default()].concat(),
                relation: rel_name.into(),
            });
        }
    }

    let data = GraphData {
        decisions,
        nodes,
        edges,
    };
    serde_json::to_value(data).map_err(graph_err)
}

fn row_string(row: &GraphRow, key: &str) -> Option<String> {
    row.get(key).and_then(|v| match v {
        GraphValue::String(s) => Some(s.to_owned()),
        _ => None,
    })
}

fn graph_node_query(kind: NodeKind) -> String {
    let projection = match kind {
        NodeKind::Decision => {
            "node.id AS id, node.title AS title, node.rationale AS rationale, node.topic_keys AS topic_keys"
        }
        NodeKind::DecisionRequest => {
            "node.id AS id, node.topic_keys AS topic_keys, node.reason AS reason"
        }
        NodeKind::Actor => "node.id AS id",
        NodeKind::Blocker => "node.id AS id, node.reason AS reason",
        NodeKind::Evidence => "node.id AS id, node.content AS content",
        NodeKind::Notification => "node.id AS id",
        NodeKind::Option => "node.id AS id, node.label AS label, node.description AS description",
        NodeKind::Hypothesis => "node.id AS id, node.statement AS statement",
    };
    format!(
        "MATCH (node:`{}`) RETURN {projection} ORDER BY node.id;",
        kind.table_name()
    )
}

fn graph_value_to_json(v: &GraphValue) -> serde_json::Value {
    match v {
        GraphValue::Null => serde_json::Value::Null,
        GraphValue::Bool(b) => serde_json::Value::Bool(*b),
        GraphValue::Int(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
        GraphValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        GraphValue::String(s) => serde_json::Value::String(s.to_owned()),
        GraphValue::StringList(list) => serde_json::Value::Array(
            list.iter()
                .map(|s| serde_json::Value::String(s.to_owned()))
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Graph cache — shared by every handler that needs a projected MemoryGraph.
// ---------------------------------------------------------------------------

/// Return a projected graph for `tenant_id`, served from the in-memory cache
/// when the ledger offset is unchanged, or rebuilt incrementally otherwise.
pub(super) fn get_cached_graph(
    ledger: &ApiLedger,
    tenant_id: &TenantId,
    cache: &GraphCache,
) -> ApiResult<Arc<MemoryGraph>> {
    let latest = ledger
        .latest_offset_for_tenant(tenant_id)
        .map_err(graph_err)?;

    // Fast path: read lock — no allocation when the offset matches.
    {
        let guard = cache
            .read()
            .map_err(|_| graph_err("graph cache poisoned"))?;
        if let Some((cached_offset, graph)) = guard.get(tenant_id) {
            if *cached_offset >= latest {
                return Ok(Arc::clone(graph));
            }
        }
    }

    // Slow path: acquire write lock, double-check, then project.
    let mut guard = cache
        .write()
        .map_err(|_| graph_err("graph cache poisoned"))?;
    if let Some((cached_offset, graph)) = guard.get(tenant_id) {
        if *cached_offset >= latest {
            return Ok(Arc::clone(graph));
        }
    }

    let (base_graph, base_offset) = match guard.get(tenant_id) {
        Some((cached_offset, cached_graph)) => (cached_graph.as_ref().clone(), *cached_offset),
        None => (MemoryGraph::default(), 0),
    };
    project_from_ledger_for_tenant(ledger, tenant_id, &base_graph, base_offset)
        .map_err(graph_err)?;
    let new_graph = Arc::new(base_graph);
    guard.insert(tenant_id.clone(), (latest, Arc::clone(&new_graph)));
    Ok(new_graph)
}

pub(super) fn open_graph_from_ledger(
    ledger: &ApiLedger,
    tenant_id: &TenantId,
    cache: &GraphCache,
) -> ApiResult<Arc<MemoryGraph>> {
    get_cached_graph(ledger, tenant_id, cache)
}
