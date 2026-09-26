//! MCP Streamable HTTP transport (`POST /mcp`, 2025-03-26 spec).
//!
//! Each POST carries a single JSON-RPC 2.0 request and receives a single
//! JSON-RPC 2.0 response (no SSE in this implementation — MCP allows plain
//! JSON for non-streaming tools). Auth uses the same bearer-token path as
//! the REST API ([`super::auth::extract_ctx`]). The `Mcp-Session-Id` header
//! is issued on `initialize` and accepted (but not enforced) on subsequent
//! requests; it seeds the default actor_id for write operations.

use std::sync::Arc;

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::commands::{CommandContext, Commands};
use crate::events::{EventProvenance, TenantId};
use crate::mcp::args::{
    optional_string as mcp_opt_str, optional_string_array as mcp_opt_str_array,
    optional_usize as mcp_opt_usize, require_string as mcp_req_str,
    require_string_array as mcp_req_str_array,
};
use crate::mcp::core::{
    CaptureDecisionArgs, CompactViewArgs, CoreError, DisagreeArgs, GetDecisionNeighborhoodArgs,
    GetDecisionOutcomeArgs, GetSituationalDecisionsArgs, GetSuggestionsArgs,
    GetSupersessionChainArgs, GroundDecisionArgs, LedgerHandle, LedgerProvider, MoveDecisionArgs,
    RecallDecisionsArgs, ScanDecisionQualityArgs, ScoreDecisionArgs, SupersedeDecisionArgs,
};
use crate::projector::memory::MemoryGraph;
use crate::queries::{
    get_decision, get_relevant_decisions, search_decisions_any, DecisionStatus, QueryContext,
    SearchDecisionRequest,
};

use super::auth::extract_ctx;
use super::graph::get_cached_graph;
use super::{
    parse_datetime, parse_status, query_envelope, ApiBackend, ApiLedger, ApiRequestCtx, ApiResult,
    AppState, GraphCache, HEADER_FORWARDED_PROTO,
};

pub(super) async fn mcp_http_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => {
            let mut resp = e.into_response();
            // MCP clients use WWW-Authenticate to discover the PRM endpoint (RFC 9728 §5).
            if resp.status() == StatusCode::UNAUTHORIZED {
                let scheme = headers
                    .get(HEADER_FORWARDED_PROTO)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("https");
                let host = headers
                    .get("host")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("localhost");
                let prm_url = format!("{scheme}://{host}/.well-known/oauth-protected-resource");
                let www_auth = format!("Bearer resource_metadata=\"{prm_url}\"");
                if let Ok(v) = www_auth.parse::<axum::http::HeaderValue>() {
                    resp.headers_mut().insert("www-authenticate", v);
                }
            }
            return resp;
        }
    };

    let session_id = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_default();

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return mcp_error_response(
                serde_json::Value::Null,
                -32700,
                format!("invalid JSON: {e}"),
            )
        }
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => {
            return mcp_error_response(
                serde_json::Value::Null,
                -32600,
                "request must be a JSON object".into(),
            )
        }
    };

    // MCP notifications carry no `id` — acknowledge with 202 and no body.
    if !obj.contains_key("id") {
        return (StatusCode::ACCEPTED, "").into_response();
    }

    let id = obj.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = match obj.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_owned(),
        None => return mcp_error_response(id, -32600, "missing `method`".into()),
    };
    let params = obj
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    match method.as_str() {
        "initialize" => {
            let new_session_id = uuid::Uuid::new_v4().to_string();
            let mut resp = mcp_success_response(id, crate::mcp::initialize_result());
            if let Ok(v) = new_session_id.parse::<axum::http::HeaderValue>() {
                resp.headers_mut().insert("mcp-session-id", v);
            }
            resp
        }
        "ping" => mcp_success_response(id, serde_json::json!({})),
        "tools/list" => mcp_success_response(id, crate::mcp::tools_list_result()),
        "tools/call" => {
            let backend = Arc::clone(&state.backend);
            let cache = Arc::clone(&state.graph_cache);
            let result = tokio::task::spawn_blocking(move || {
                mcp_tools_call_blocking(&backend, &ctx, &session_id, params, &cache)
            })
            .await;
            match result {
                Ok(Ok(v)) => mcp_success_response(id, v),
                Ok(Err((code, msg))) => mcp_error_response(id, code, msg),
                Err(e) => mcp_error_response(id, -32603, e.to_string()),
            }
        }
        other => mcp_error_response(id, -32601, format!("unknown method: {other}")),
    }
}

fn mcp_success_response(id: serde_json::Value, result: serde_json::Value) -> Response {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    (StatusCode::OK, Json(body)).into_response()
}

fn mcp_error_response(id: serde_json::Value, code: i32, message: String) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    });
    (StatusCode::OK, Json(body)).into_response()
}

type McpToolResult = std::result::Result<serde_json::Value, (i32, String)>;

fn mcp_tools_call_blocking(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    session_id: &str,
    params: serde_json::Value,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let mut obj = match params {
        serde_json::Value::Object(map) => map,
        _ => return Err((-32602, "params must be an object".into())),
    };
    let name = match obj.remove("name") {
        Some(serde_json::Value::String(s)) => s,
        _ => return Err((-32602, "missing `name`".into())),
    };
    let arguments = obj
        .remove("arguments")
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let args = match arguments {
        serde_json::Value::Object(map) => map,
        _ => return Err((-32602, "`arguments` must be an object".into())),
    };

    let actor_id = mcp_resolve_actor(&args, &ctx.actor_id, session_id);

    let outcome: McpToolResult = match name.as_str() {
        "capture_decision" => mcp_capture_decision(backend, ctx, &actor_id, args),
        "capture_evidence" => mcp_capture_evidence(backend, ctx, &actor_id, args),
        "capture_hypothesis" => mcp_capture_hypothesis(backend, ctx, &actor_id, args),
        "disagree_decision" => mcp_disagree(backend, ctx, &actor_id, args),
        "supersede_decision" => mcp_supersede(backend, ctx, &actor_id, args),
        "move_decision" => mcp_move(backend, ctx, &actor_id, args),
        "ground_decision" => mcp_ground(backend, ctx, &actor_id, args),
        "get_decision" => mcp_get_decision(backend, ctx, args, cache),
        "get_decision_outcome" => mcp_get_decision_outcome(backend, ctx, args),
        "get_relevant_decisions" => mcp_get_relevant_decisions(backend, ctx, args, cache),
        "get_situational_decisions" => mcp_get_situational_decisions(backend, ctx, args, cache),
        "get_supersession_chain" => mcp_get_supersession_chain(backend, ctx, args),
        "get_decision_neighborhood" => mcp_get_decision_neighborhood(backend, ctx, args),
        "recall_decisions" => mcp_recall_decisions(backend, ctx, args, cache),
        "search_decisions" => mcp_search_decisions(backend, ctx, args, cache),
        "score_decision" => mcp_score_decision(backend, ctx, args, cache),
        "scan_decision_quality" => mcp_scan_decision_quality(backend, ctx, args, cache),
        "get_suggestions" => mcp_get_suggestions(backend, ctx, args, cache),
        "dump_graph" => mcp_dump_graph(backend, ctx, cache),
        "hivemind_compact_view" => mcp_compact_view(backend, ctx, args),
        "summarize_decisions" => mcp_summarize(backend, ctx, args, cache),
        "classify_queue_list" => mcp_classify_queue_list(backend, ctx, args),
        "classify_queue_submit" => mcp_classify_queue_submit(backend, ctx, args),
        other => return Err((-32602, format!("unknown tool: {other}"))),
    };

    // Per MCP spec: tool-level errors are returned as success responses with
    // `isError: true` rather than as JSON-RPC error objects.
    match outcome {
        Ok(payload) => Ok(mcp_tool_ok(payload)),
        Err((_, msg)) => Ok(mcp_tool_err(msg)),
    }
}

fn mcp_resolve_actor(
    args: &serde_json::Map<String, serde_json::Value>,
    auth_actor: &str,
    session_id: &str,
) -> String {
    args.get("actor_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| {
            if session_id.is_empty() {
                auth_actor.to_owned()
            } else {
                format!("agent:mcp-http:{session_id}")
            }
        })
}

fn mcp_open_graph(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    cache: &Arc<GraphCache>,
) -> std::result::Result<Arc<MemoryGraph>, (i32, String)> {
    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    get_cached_graph(&ledger, &ctx.tenant_id, cache).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_tool_ok(payload: serde_json::Value) -> serde_json::Value {
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
        "structuredContent": payload,
    })
}

fn mcp_tool_err(message: String) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

// ---------------------------------------------------------------------------
// Per-tool implementations
// ---------------------------------------------------------------------------

/// HTTP's [`LedgerProvider`]: resolves a tenant-scoped ledger from the
/// tenant already established by [`extract_ctx`]'s WorkOS/bearer-token
/// validation. Auth happened before this is ever called — the core this
/// feeds never sees the tenant negotiation, only its result.
struct HttpLedgerProvider<'a> {
    backend: &'a ApiBackend,
    tenant_id: &'a TenantId,
}

impl LedgerProvider for HttpLedgerProvider<'_> {
    type Ledger = ApiLedger;

    fn ledger(&self) -> std::result::Result<LedgerHandle<Self::Ledger>, CoreError> {
        let ledger = self
            .backend
            .open_ledger_for_tenant(self.tenant_id)
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        Ok(LedgerHandle {
            ledger,
            tenant_id: self.tenant_id.clone(),
        })
    }
}

impl From<CoreError> for (i32, String) {
    fn from(error: CoreError) -> Self {
        let code = match &error {
            CoreError::InvalidArgument(_) => -32602,
            CoreError::Internal(_) => -32603,
        };
        (code, error.into_message())
    }
}

fn mcp_capture_decision(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = CaptureDecisionArgs::from_json(&args, actor_id.to_owned())?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::capture_decision(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_capture_evidence(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let content = mcp_req_str(&args, "content")?;
    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(
            ctx.tenant_id.clone(),
            EventProvenance::agent(actor_id.to_owned()),
        ),
    );
    let evidence_id = commands
        .record_evidence(actor_id, &content)
        .map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({ "evidence_id": evidence_id }))
}

fn mcp_capture_hypothesis(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let statement = mcp_req_str(&args, "statement")?;
    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(
            ctx.tenant_id.clone(),
            EventProvenance::agent(actor_id.to_owned()),
        ),
    );
    let hypothesis_id = commands
        .record_hypothesis(actor_id, &statement)
        .map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({ "hypothesis_id": hypothesis_id }))
}

fn mcp_disagree(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = DisagreeArgs::from_json(&args, actor_id.to_owned())?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::disagree_decision(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_supersede(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = SupersedeDecisionArgs::from_json(&args, actor_id.to_owned())?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::supersede_decision(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_move(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = MoveDecisionArgs::from_json(&args, actor_id.to_owned())?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::move_decision(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_ground(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = GroundDecisionArgs::from_json(&args, actor_id.to_owned())?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::ground_decision(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_get_decision(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let response = get_decision(&*graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_get_decision_outcome(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = GetDecisionOutcomeArgs::from_json(&args)?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::get_decision_outcome(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_get_relevant_decisions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let topic = mcp_req_str(&args, "topic")?;
    let status_filter = match args
        .get("status")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        None => None,
        Some("proposed") => Some(DecisionStatus::Proposed),
        Some("accepted") => Some(DecisionStatus::Accepted),
        Some("rejected") => Some(DecisionStatus::Rejected),
        Some("contested") => Some(DecisionStatus::Contested),
        Some("superseded") => Some(DecisionStatus::Superseded),
        Some(other) => return Err((-32602, format!("unknown status `{other}`"))),
    };
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let response = get_relevant_decisions(&*graph, &topic, status_filter)
        .map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_get_situational_decisions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let core_args = GetSituationalDecisionsArgs::from_json(&args)?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::get_situational_decisions(&provider, &*graph, core_args)?;
    Ok(output.into_value())
}

fn mcp_recall_decisions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let core_args = RecallDecisionsArgs::from_json(&args)?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::recall_decisions(&provider, &*graph, core_args)?;
    Ok(output.into_value())
}

fn mcp_get_supersession_chain(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = GetSupersessionChainArgs::from_json(&args)?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::get_supersession_chain(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_get_decision_neighborhood(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = GetDecisionNeighborhoodArgs::from_json(&args)?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::get_decision_neighborhood(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_search_decisions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let graph =
        get_cached_graph(&ledger, &ctx.tenant_id, cache).map_err(|e| (-32603i32, e.to_string()))?;

    let query = mcp_opt_str(&args, "q")?;
    let topic_keys = mcp_opt_str_array(&args, "topic")?;
    let statuses = mcp_opt_str_array(&args, "status")?
        .into_iter()
        .map(|s| parse_status(&s))
        .collect::<ApiResult<Vec<_>>>()
        .map_err(|e| (-32602i32, e.to_string()))?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(25);
    let since = match mcp_opt_str(&args, "since")? {
        Some(s) => Some(
            parse_datetime(&s).map_err(|e| (-32602i32, format!("`since` must be RFC3339: {e}")))?,
        ),
        None => None,
    };
    let until = match mcp_opt_str(&args, "until")? {
        Some(s) => Some(
            parse_datetime(&s).map_err(|e| (-32602i32, format!("`until` must be RFC3339: {e}")))?,
        ),
        None => None,
    };
    let request = SearchDecisionRequest {
        query,
        topic_keys,
        statuses,
        actor_ids: mcp_opt_str_array(&args, "actor_id")?,
        sources: mcp_opt_str_array(&args, "source")?,
        since,
        until,
        limit,
        cursor: mcp_opt_str(&args, "cursor")?,
        project: None,
    };
    let query_ctx = QueryContext::new(ctx.tenant_id.clone());
    let response = search_decisions_any(&query_ctx, &ledger, &*graph, &request)
        .map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_score_decision(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let core_args = ScoreDecisionArgs::from_json(&args)?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let output = crate::mcp::core::score_decision(&*graph, core_args)?;
    Ok(output.into_value())
}

fn mcp_scan_decision_quality(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let core_args = ScanDecisionQualityArgs::from_json(&args)?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let output = crate::mcp::core::scan_decision_quality(&*graph, core_args)?;
    Ok(output.into_value())
}

fn mcp_get_suggestions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let core_args = GetSuggestionsArgs::from_json(&args)?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let output = crate::mcp::core::get_suggestions(&*graph, core_args)?;
    Ok(output.into_value())
}

fn mcp_dump_graph(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let dot = crate::cli::render_decision_dot(&*graph).map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({ "format": "dot", "content": dot }))
}

fn mcp_compact_view(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let core_args = CompactViewArgs::from_json(&args)?;
    let provider = HttpLedgerProvider {
        backend,
        tenant_id: &ctx.tenant_id,
    };
    let output = crate::mcp::core::compact_view(&provider, core_args)?;
    Ok(output.into_value())
}

fn mcp_summarize(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    use crate::summarize::{summarize_decisions, SummarizeMode, SummarizeRequest};

    let decision_ids = mcp_req_str_array(&args, "decision_ids")?;
    if decision_ids.is_empty() {
        return Err((-32602, "decision_ids must not be empty".into()));
    }
    let mode_str = mcp_opt_str(&args, "mode")?;
    let mode = match mode_str.as_deref() {
        None if decision_ids.len() == 1 => SummarizeMode::Single,
        None => SummarizeMode::Cluster,
        Some("single") if decision_ids.len() != 1 => {
            return Err((
                -32602,
                "mode=single requires exactly one decision_id".into(),
            ))
        }
        Some("single") => SummarizeMode::Single,
        Some("cluster") => SummarizeMode::Cluster,
        Some("chain") if decision_ids.len() != 1 => {
            return Err((-32602, "mode=chain requires exactly one decision_id".into()))
        }
        Some("chain") => SummarizeMode::Chain,
        Some(other) => {
            return Err((
                -32602,
                format!("unknown mode `{other}`; must be single, cluster, or chain"),
            ))
        }
    };
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let request = SummarizeRequest { decision_ids, mode };
    let response =
        summarize_decisions(&*graph, &request).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

/// Lists pending (unclassified) ingest batches with rendered turn text, plus
/// today's classification budget. MCP mirror of `GET /v1/classify-queue`
/// (hivemind-zdsh.18).
fn mcp_classify_queue_list(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let session_id = mcp_opt_str(&args, "session_id")?;
    let limit = mcp_opt_usize(&args, "limit")?.unwrap_or(20);

    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let mut batches = crate::classifier::list_pending_batches_for_ledger(&ledger, &ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    if let Some(ref session_id) = session_id {
        batches.retain(|b| &b.session_id == session_id);
    }
    batches.truncate(limit);
    let budget = crate::classifier::daily_cap_status(&ledger, &ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({ "batches": batches, "budget": budget }))
}

/// Submits captures for one or more pending batches from the same session in
/// a single classification event — the session-grouped cadence
/// (hivemind-zdsh.18): one model call per session end, not one per batch.
/// Refuses with an error once the daily classification cap is hit; batches
/// stay pending for the next day rather than being dropped. MCP mirror of
/// `POST /v1/classify-queue/submit`.
fn mcp_classify_queue_submit(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let batch_ids = mcp_req_str_array(&args, "batch_ids")?;
    if batch_ids.is_empty() {
        return Err((-32602, "batch_ids must not be empty".into()));
    }
    let captures_value = args
        .get("captures")
        .cloned()
        .ok_or_else(|| (-32602, "missing `captures`".to_owned()))?;
    let captures: Vec<crate::events::CaptureItem> = serde_json::from_value(captures_value)
        .map_err(|e| (-32602, format!("`captures` is invalid: {e}")))?;
    let model = mcp_opt_str(&args, "model")?.unwrap_or_else(|| "agent:worker-a".to_owned());

    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;

    let cap_status = crate::classifier::daily_cap_status(&ledger, &ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    if cap_status.remaining == 0 {
        return Err((
            -32000,
            format!(
                "daily classification cap ({}) reached; batches remain pending until tomorrow (UTC)",
                cap_status.cap
            ),
        ));
    }

    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(
            ctx.tenant_id.clone(),
            EventProvenance::agent(ctx.actor_id.clone()),
        ),
    );
    let capture_count = captures.len();
    let event_id = commands
        .record_ingest_batch_classified(
            &ctx.actor_id,
            &batch_ids,
            &model,
            crate::classifier::SCHEMA_VERSION,
            captures,
            None,
        )
        .map_err(|e| (-32603i32, e.to_string()))?;

    Ok(serde_json::json!({
        "batch_ids": batch_ids,
        "capture_count": capture_count,
        "event_id": event_id,
    }))
}
