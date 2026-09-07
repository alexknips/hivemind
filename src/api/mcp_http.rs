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

use crate::commands::{CommandContext, Commands, DecisionProposalInput, SupersedeInput};
use crate::events::EventProvenance;
use crate::mcp::args::{
    default_option_description, optional_option_labels as mcp_opt_option_labels,
    optional_string as mcp_opt_str, optional_string_array as mcp_opt_str_array,
    optional_usize as mcp_opt_usize, require_string as mcp_req_str,
    require_string_array as mcp_req_str_array,
};
use crate::projector::memory::MemoryGraph;
#[cfg(feature = "shared-backend-postgres")]
use crate::queries::search_decisions_with_ledger;
use crate::queries::{
    derive_decision_status, get_compact_view, get_decision, get_decision_quality_score,
    get_relevant_decisions, get_supersession_chain, scan_decision_quality, scorer_next_cursor,
    search_decisions_fts_with_context, DecisionStatus, QualityTier, QueryContext,
    ScanQualityRequest, ScorerConfig, SearchDecisionRequest,
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
        "disagree_decision" => mcp_disagree(backend, ctx, &actor_id, args, cache),
        "supersede_decision" => mcp_supersede(backend, ctx, &actor_id, args, cache),
        "get_decision" => mcp_get_decision(backend, ctx, args, cache),
        "get_relevant_decisions" => mcp_get_relevant_decisions(backend, ctx, args, cache),
        "get_supersession_chain" => mcp_get_supersession_chain(backend, ctx, args, cache),
        "search_decisions" => mcp_search_decisions(backend, ctx, args, cache),
        "score_decision" => mcp_score_decision(backend, ctx, args, cache),
        "scan_decision_quality" => mcp_scan_decision_quality(backend, ctx, args, cache),
        "dump_graph" => mcp_dump_graph(backend, ctx, cache),
        "hivemind_compact_view" => mcp_compact_view(backend, ctx, args, cache),
        "summarize_decisions" => mcp_summarize(backend, ctx, args, cache),
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

fn mcp_capture_decision(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let title = mcp_req_str(&args, "title")?;
    let rationale = mcp_req_str(&args, "rationale")?;
    let topic_keys = mcp_req_str_array(&args, "topic_keys")?;
    if topic_keys.is_empty() {
        return Err((-32602, "topic_keys must not be empty".into()));
    }
    let options_val = args
        .get("options")
        .cloned()
        .ok_or_else(|| (-32602i32, "missing `options`".to_owned()))?;
    let options = match options_val {
        serde_json::Value::Array(v) => v,
        _ => return Err((-32602, "`options` must be an array".into())),
    };
    if options.is_empty() {
        return Err((-32602, "options must not be empty".into()));
    }
    let chosen_label = mcp_opt_str(&args, "chosen_option_label")?;
    let hypothesis_ids = mcp_opt_str_array(&args, "hypothesis_ids")?;
    let evidence_ids = mcp_opt_str_array(&args, "evidence_ids")?;

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

    let mut option_ids: Vec<String> = Vec::with_capacity(options.len());
    let mut chosen_option_id: Option<String> = None;
    for (i, opt) in options.into_iter().enumerate() {
        let obj = match opt {
            serde_json::Value::Object(map) => map,
            _ => return Err((-32602, format!("options[{i}] must be an object"))),
        };
        let label = obj
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| (-32602i32, format!("options[{i}].label must be non-empty")))?
            .to_owned();
        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_owned())
            .unwrap_or_else(|| default_option_description(&label));
        let oid = commands
            .record_option(actor_id, &label, &description)
            .map_err(|e| (-32603i32, e.to_string()))?;
        // ubs:ignore: == compares option labels (user-visible strings), not secrets
        if chosen_label.as_deref() == Some(label.as_str()) {
            chosen_option_id = Some(oid.clone());
        }
        option_ids.push(oid);
    }
    if chosen_label.is_some() && chosen_option_id.is_none() {
        return Err((
            -32602,
            "chosen_option_label must match one of the supplied option labels".into(),
        ));
    }
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id,
            title: &title,
            rationale: &rationale,
            topic_keys: &topic_keys,
            option_ids: &option_ids,
            chosen_option_id: chosen_option_id.as_deref(),
            hypothesis_ids: &hypothesis_ids,
            evidence_ids: &evidence_ids,
        })
        .map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({
        "decision_id": decision_id,
        "option_ids": option_ids,
        "chosen_option_id": chosen_option_id,
    }))
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
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let reason = mcp_req_str(&args, "reason")?;
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
    let event_id = commands
        .disagree(actor_id, &decision_id, &reason)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let graph =
        get_cached_graph(&ledger, &ctx.tenant_id, cache).map_err(|e| (-32603i32, e.to_string()))?;
    let status =
        derive_decision_status(&*graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({
        "decision_id": decision_id,
        "event_id": event_id,
        "decision_status": status,
    }))
}

fn mcp_supersede(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    actor_id: &str,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let old_id = mcp_req_str(&args, "old_decision_id")?;
    let title = mcp_req_str(&args, "title")?;
    let rationale = mcp_req_str(&args, "rationale")?;
    let topic_keys = mcp_opt_str_array(&args, "topic_keys")?;
    let option_labels = mcp_opt_option_labels(&args, "options")?;
    let chosen_label = mcp_opt_str(&args, "chosen_option_label")?;
    let hypothesis_ids = mcp_opt_str_array(&args, "hypothesis_ids")?;
    let evidence_ids = mcp_opt_str_array(&args, "evidence_ids")?;

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
    let outcome = commands
        .supersede(SupersedeInput {
            actor_id,
            old_decision_id: &old_id,
            new_title: &title,
            new_rationale: &rationale,
            topic_keys: &topic_keys,
            option_labels: &option_labels,
            chosen_option_label: chosen_label.as_deref(),
            hypothesis_ids: &hypothesis_ids,
            evidence_ids: &evidence_ids,
        })
        .map_err(|e| (-32603i32, e.to_string()))?;
    let graph =
        get_cached_graph(&ledger, &ctx.tenant_id, cache).map_err(|e| (-32603i32, e.to_string()))?;
    let old_status =
        derive_decision_status(&*graph, &old_id).map_err(|e| (-32603i32, e.to_string()))?;
    let new_status = derive_decision_status(&*graph, &outcome.new_decision_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({
        "old_decision_id": old_id,
        "new_decision_id": outcome.new_decision_id,
        "proposal_event_id": outcome.proposal_event_id,
        "relation_event_ids": outcome.relation_event_ids,
        "superseded_event_id": outcome.superseded_event_id,
        "old_decision_status": old_status,
        "new_decision_status": new_status,
    }))
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

fn mcp_get_supersession_chain(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let response =
        get_supersession_chain(&*graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
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
    };
    let query_ctx = QueryContext::new(ctx.tenant_id.clone());
    let response = match &ledger {
        ApiLedger::Sqlite(sqlite_ledger) => {
            search_decisions_fts_with_context(&query_ctx, sqlite_ledger, &*graph, &request)
                .map_err(|e| (-32603i32, e.to_string()))?
        }
        #[cfg(feature = "shared-backend-postgres")]
        ApiLedger::Postgres(postgres_ledger) => {
            search_decisions_with_ledger(&query_ctx, postgres_ledger, &*graph, &request)
                .map_err(|e| (-32603i32, e.to_string()))?
        }
    };
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_score_decision(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let config = ScorerConfig::default();
    let response = get_decision_quality_score(&*graph, &decision_id, &config)
        .map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_scan_decision_quality(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let since_event_origin = args.get("since_event_origin").and_then(|v| v.as_i64());
    let limit = mcp_opt_usize(&args, "limit")?.unwrap_or(25);
    let cursor = mcp_opt_str(&args, "cursor")?;
    let min_tier = match mcp_opt_str(&args, "min_tier")? {
        None => None,
        Some(s) => Some(parse_quality_tier_http(&s)?),
    };
    let request = ScanQualityRequest {
        since_event_origin,
        limit,
        cursor: cursor.clone(),
        min_tier,
    };
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let config = ScorerConfig::default();
    let response = scan_decision_quality(&*graph, &request, &config)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let skip: usize = cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(0);
    let next_cursor = if response.truncated {
        scorer_next_cursor(skip, response.result_count)
    } else {
        None
    };
    let mut value =
        serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))?;
    if let (Some(nc), Some(obj)) = (next_cursor, value.as_object_mut()) {
        obj.insert("next_cursor".to_owned(), serde_json::Value::String(nc));
    }
    Ok(value)
}

fn parse_quality_tier_http(s: &str) -> std::result::Result<QualityTier, (i32, String)> {
    match s {
        "clean" => Ok(QualityTier::Clean),
        "minor_concerns" => Ok(QualityTier::MinorConcerns),
        "significant_concerns" => Ok(QualityTier::SignificantConcerns),
        "high_concern" => Ok(QualityTier::HighConcern),
        other => Err((-32602, format!("unknown quality tier `{other}`; expected clean, minor_concerns, significant_concerns, or high_concern"))),
    }
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
    cache: &Arc<GraphCache>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx, cache)?;
    let response =
        get_compact_view(&*graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(&response).map_err(|e| (-32603i32, e.to_string()))
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
