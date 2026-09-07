//! `/v1/*` REST handlers for decisions, evidence, hypotheses, search, and
//! transcript ingest — the CRUD surface of the HTTP API. Auth (`extract_ctx`)
//! lives in [`super::auth`]; graph projection/caching lives in [`super::graph`].

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::commands::{CommandContext, Commands, DecisionProposalInput, SupersedeInput};
use crate::events::{EventProvenance, IngestTurn};
use crate::ledger::{EventLedger, SqliteEventLedger};
#[cfg(feature = "shared-backend-postgres")]
use crate::queries::search_decisions_with_ledger;
use crate::queries::{
    derive_decision_status, get_compact_view, get_decision, get_relevant_decisions,
    get_supersession_chain, search_decisions_fts_with_context, QueryContext, SearchDecisionRequest,
};

use super::auth::extract_ctx;
use super::graph::{get_cached_graph, open_graph_from_ledger};
use super::{
    parse_datetime, parse_status, respond, respond_envelope, to_api_error, ApiBackend, ApiError,
    ApiLedger, ApiRequestCtx, ApiResult, AppState,
};

#[derive(Debug, Deserialize)]
struct OptionInput {
    label: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CaptureDecisionRequest {
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    options: Vec<OptionInput>,
    chosen_option_label: Option<String>,
    #[serde(default)]
    hypothesis_ids: Vec<String>,
    #[serde(default)]
    evidence_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CaptureEvidenceRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CaptureHypothesisRequest {
    statement: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct DisagreeRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct SupersedeRequest {
    title: String,
    rationale: String,
    #[serde(default)]
    topic_keys: Vec<String>,
    #[serde(default)]
    options: Vec<String>,
    chosen_option_label: Option<String>,
    #[serde(default)]
    hypothesis_ids: Vec<String>,
    #[serde(default)]
    evidence_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IngestTurnRequest {
    turn_id: String,
    role: String,
    text: String,
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct IngestBatchRequest {
    batch_id: String,
    agent_tool: String,
    session_id: String,
    #[serde(default)]
    turns: Vec<IngestTurnRequest>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SearchParams {
    q: Option<String>,
    topic: Option<String>,
    status: Option<String>,
    actor_id: Option<String>,
    source: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RelevantParams {
    topic: String,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MapParams {
    /// Blend weight: 0.0 = pure semantic, 1.0 = pure structural. Comma-separated
    /// list (e.g. "0.0,0.5") returns two result sets for side-by-side comparison.
    alpha: Option<String>,
}

pub(super) async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let backend = Arc::clone(&state.backend);
    let healthy = tokio::task::spawn_blocking(move || backend_healthy(&backend)).await;

    match healthy {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "error" })),
        )
            .into_response(),
    }
}

fn backend_healthy(backend: &ApiBackend) -> bool {
    match backend {
        ApiBackend::Sqlite(dir) => SqliteEventLedger::open(dir.as_ref()).is_ok(),
        #[cfg(feature = "shared-backend-postgres")]
        ApiBackend::Postgres(ledger) => ledger.pool().get().is_ok(),
    }
}

pub(super) async fn post_decisions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CaptureDecisionRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result =
        tokio::task::spawn_blocking(move || capture_decision_blocking(&backend, &ctx, req)).await;

    respond(result, StatusCode::OK)
}

fn capture_decision_blocking(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    req: CaptureDecisionRequest,
) -> ApiResult<serde_json::Value> {
    if req.title.trim().is_empty() {
        return Err(ApiError::validation("title must not be empty"));
    }
    if req.rationale.trim().is_empty() {
        return Err(ApiError::validation("rationale must not be empty"));
    }
    if req.topic_keys.is_empty() {
        return Err(ApiError::validation("topic_keys must not be empty"));
    }
    if req.options.is_empty() {
        return Err(ApiError::validation("options must not be empty"));
    }

    let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(
            ctx.tenant_id.clone(),
            EventProvenance::api(Some(ctx.actor_id.clone())),
        ),
    );

    let mut option_ids: Vec<String> = Vec::with_capacity(req.options.len());
    let mut chosen_option_id: Option<String> = None;
    for (index, option) in req.options.into_iter().enumerate() {
        let label = option.label.trim().to_owned();
        if label.is_empty() {
            return Err(ApiError::validation(format!(
                "options[{index}].label must not be empty"
            )));
        }
        let description = option
            .description
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| format!("Option '{label}'"));

        let option_id = commands
            .record_option(&ctx.actor_id, &label, &description)
            .map_err(to_api_error)?;

        // ubs:ignore: == compares option labels (user-visible strings), not secrets
        if req.chosen_option_label.as_deref() == Some(label.as_str()) {
            chosen_option_id = Some(option_id.clone());
        }
        option_ids.push(option_id);
    }

    if req.chosen_option_label.is_some() && chosen_option_id.is_none() {
        return Err(ApiError::validation(
            "chosen_option_label must match one of the supplied option labels",
        ));
    }

    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: &ctx.actor_id,
            title: &req.title,
            rationale: &req.rationale,
            topic_keys: &req.topic_keys,
            option_ids: &option_ids,
            chosen_option_id: chosen_option_id.as_deref(),
            hypothesis_ids: &req.hypothesis_ids,
            evidence_ids: &req.evidence_ids,
        })
        .map_err(to_api_error)?;

    Ok(serde_json::json!({
        "decision_id": decision_id,
        "option_ids": option_ids,
        "chosen_option_id": chosen_option_id,
    }))
}

pub(super) async fn post_evidence_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CaptureEvidenceRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || {
        if req.content.trim().is_empty() {
            return Err(ApiError::validation("content must not be empty"));
        }
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let commands = Commands::new_with_context(
            &ledger,
            CommandContext::new(
                ctx.tenant_id,
                EventProvenance::api(Some(ctx.actor_id.clone())),
            ),
        );
        let evidence_id = commands
            .record_evidence(&ctx.actor_id, &req.content)
            .map_err(to_api_error)?;
        Ok(serde_json::json!({ "evidence_id": evidence_id }))
    })
    .await;

    respond(result, StatusCode::OK)
}

pub(super) async fn post_hypotheses_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CaptureHypothesisRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || {
        if req.statement.trim().is_empty() {
            return Err(ApiError::validation("statement must not be empty"));
        }
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let commands = Commands::new_with_context(
            &ledger,
            CommandContext::new(
                ctx.tenant_id,
                EventProvenance::api(Some(ctx.actor_id.clone())),
            ),
        );
        let hypothesis_id = commands
            .record_hypothesis(&ctx.actor_id, &req.statement)
            .map_err(to_api_error)?;
        Ok(serde_json::json!({ "hypothesis_id": hypothesis_id }))
    })
    .await;

    respond(result, StatusCode::OK)
}

pub(super) async fn disagree_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
    payload: std::result::Result<Json<DisagreeRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || {
        if req.reason.trim().is_empty() {
            return Err(ApiError::validation("reason must not be empty"));
        }
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let commands = Commands::new_with_context(
            &ledger,
            CommandContext::new(
                ctx.tenant_id.clone(),
                EventProvenance::api(Some(ctx.actor_id.clone())),
            ),
        );
        let event_id = commands
            .disagree(&ctx.actor_id, &decision_id, &req.reason)
            .map_err(to_api_error)?;

        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let graph = &*graph;
        let decision_status = derive_decision_status(graph, &decision_id).map_err(to_api_error)?;

        Ok(serde_json::json!({
            "decision_id": decision_id,
            "event_id": event_id,
            "decision_status": decision_status,
        }))
    })
    .await;

    respond(result, StatusCode::OK)
}

pub(super) async fn supersede_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(old_decision_id): Path<String>,
    payload: std::result::Result<Json<SupersedeRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || {
        if req.title.trim().is_empty() {
            return Err(ApiError::validation("title must not be empty"));
        }
        if req.rationale.trim().is_empty() {
            return Err(ApiError::validation("rationale must not be empty"));
        }
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let commands = Commands::new_with_context(
            &ledger,
            CommandContext::new(
                ctx.tenant_id.clone(),
                EventProvenance::api(Some(ctx.actor_id.clone())),
            ),
        );
        let outcome = commands
            .supersede(SupersedeInput {
                actor_id: &ctx.actor_id,
                old_decision_id: &old_decision_id,
                new_title: &req.title,
                new_rationale: &req.rationale,
                topic_keys: &req.topic_keys,
                option_labels: &req.options,
                chosen_option_label: req.chosen_option_label.as_deref(),
                hypothesis_ids: &req.hypothesis_ids,
                evidence_ids: &req.evidence_ids,
            })
            .map_err(to_api_error)?;

        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let graph = &*graph;
        let old_status = derive_decision_status(graph, &old_decision_id).map_err(to_api_error)?;
        let new_status =
            derive_decision_status(graph, &outcome.new_decision_id).map_err(to_api_error)?;

        Ok(serde_json::json!({
            "old_decision_id": old_decision_id,
            "new_decision_id": outcome.new_decision_id,
            "proposal_event_id": outcome.proposal_event_id,
            "relation_event_ids": outcome.relation_event_ids,
            "superseded_event_id": outcome.superseded_event_id,
            "old_decision_status": old_status,
            "new_decision_status": new_status,
        }))
    })
    .await;

    respond(result, StatusCode::OK)
}

pub(super) async fn get_decision_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let view = get_decision(&*graph, &decision_id).map_err(to_api_error)?;
        Ok(view)
    })
    .await;

    respond_envelope(result)
}

pub(super) async fn supersession_chain_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let graph = &*graph;
        let exists = get_decision(graph, &decision_id).map_err(to_api_error)?;
        if exists.data.is_none() {
            return Err(ApiError::not_found(format!(
                "decision not found: {decision_id}"
            )));
        }
        let response = get_supersession_chain(graph, &decision_id).map_err(to_api_error)?;
        Ok(response)
    })
    .await;

    respond_envelope(result)
}

pub(super) async fn compact_view_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let response = get_compact_view(&*graph, &decision_id).map_err(to_api_error)?;
        Ok(response)
    })
    .await;

    respond_envelope(result)
}

pub(super) async fn map_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<MapParams>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let map_cache = Arc::clone(&state.map_cache);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        // sqlite_dir() returns None for the Postgres backend; compute_map accepts Option<&Path>
        // and falls back to structural-only layout when no embedding store is available.
        let dir: Option<std::path::PathBuf> = backend.sqlite_dir().map(|p| p.to_path_buf());

        let alphas = parse_alpha_list(params.alpha.as_deref())?;
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;

        // Cheap offset read used as cache key — avoids rebuilding the graph on repeated calls.
        let offset = ledger
            .latest_offset_for_tenant(&ctx.tenant_id)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let tenant_key = ctx.tenant_id.to_string();

        // Build or reuse cached MemoryGraph (avoids full ledger replay on each request).
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;

        if alphas.len() == 1 {
            let alpha = alphas[0]; // ubs:ignore: alphas[0] guarded by len()==1 check above
            let cache_key = (tenant_key, offset, alpha.to_bits());
            // Lock is released immediately after .cloned() — graph build happens outside the lock.
            let hit = map_cache.lock().unwrap().get(&cache_key).cloned(); // ubs:ignore: Mutex::lock().unwrap() — non-panicking path; .cloned() releases lock before any compute
            if let Some(cached) = hit {
                return Ok(serde_json::to_value(&cached).unwrap_or_default());
            }
            let r = crate::map::compute_map(&*graph, dir.as_deref(), alpha)
                .map_err(|e| ApiError::internal(e.to_string()))?; // ubs:ignore: error conversion at handler boundary
            map_cache.lock().unwrap().insert(cache_key, r.clone()); // ubs:ignore: Mutex::lock().unwrap() — non-panicking path; clone necessary — r moved into cache, also returned
            Ok(serde_json::to_value(&r).unwrap_or_default())
        } else {
            let mut results = Vec::new();
            for &alpha in &alphas {
                let cache_key = (tenant_key.clone(), offset, alpha.to_bits()); // ubs:ignore: clone necessary — cache_key moved into map_cache per iteration
                let hit = map_cache.lock().unwrap().get(&cache_key).cloned(); // ubs:ignore: Mutex::lock().unwrap() — non-panicking path; .cloned() releases lock before compute
                if let Some(cached) = hit {
                    results.push(cached);
                    continue;
                }
                let r = crate::map::compute_map(&*graph, dir.as_deref(), alpha)
                    .map_err(|e| ApiError::internal(e.to_string()))?; // ubs:ignore: error conversion at handler boundary
                map_cache.lock().unwrap().insert(cache_key, r.clone()); // ubs:ignore: Mutex::lock().unwrap() — non-panicking path; clone necessary — r moved into cache, also pushed to results
                results.push(r);
            }
            Ok(serde_json::to_value(&results).unwrap_or_default())
        }
    })
    .await;

    respond(result, StatusCode::OK)
}

fn parse_alpha_list(raw: Option<&str>) -> ApiResult<Vec<f64>> {
    let raw = raw.unwrap_or("0.5");
    let alphas: Vec<f64> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<f64>()
                .map_err(|_| ApiError::validation(format!("invalid alpha value: {s}")))
            // ubs:ignore: format! for user-facing validation message
        })
        .collect::<ApiResult<Vec<f64>>>()?;
    if alphas.is_empty() {
        return Ok(vec![0.5]);
    }
    for &a in &alphas {
        if !(0.0..=1.0).contains(&a) {
            let msg = format!("alpha must be between 0.0 and 1.0, got {a}"); // ubs:ignore: error path
            return Err(ApiError::validation(msg));
        }
    }
    Ok(alphas)
}

pub(super) async fn search_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let statuses = match params.status.as_deref() {
            None => Vec::new(),
            Some(s) => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(parse_status)
                .collect::<ApiResult<Vec<_>>>()?,
        };
        let since = params
            .since
            .as_deref()
            .map(parse_datetime)
            .transpose()
            .map_err(|e| ApiError::validation(format!("invalid `since`: {e}")))?;
        let until = params
            .until
            .as_deref()
            .map(parse_datetime)
            .transpose()
            .map_err(|e| ApiError::validation(format!("invalid `until`: {e}")))?;

        let request = SearchDecisionRequest {
            query: params.q.filter(|s| !s.trim().is_empty()),
            topic_keys: params
                .topic
                .as_deref()
                .map(|t| {
                    t.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            statuses,
            actor_ids: params
                .actor_id
                .as_deref()
                .map(|a| {
                    a.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            sources: params
                .source
                .as_deref()
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            since,
            until,
            limit: params.limit.unwrap_or(25).min(1000),
            cursor: params.cursor,
        };

        match backend.as_ref() {
            ApiBackend::Sqlite(_dir) => {
                let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
                let graph = get_cached_graph(&ledger, &ctx.tenant_id, &cache)?;
                // FTS requires a concrete SqliteEventLedger.
                #[allow(clippy::infallible_destructuring_match)]
                let sqlite_ledger = match ledger {
                    ApiLedger::Sqlite(l) => l,
                    #[cfg(feature = "shared-backend-postgres")]
                    ApiLedger::Postgres(_) => unreachable!(), // ubs:ignore: cfg-gated arm; unreachable when only sqlite feature is active
                };
                let query_ctx = QueryContext::new(ctx.tenant_id);
                let response = search_decisions_fts_with_context(
                    &query_ctx,
                    &sqlite_ledger,
                    &*graph,
                    &request,
                )
                .map_err(to_api_error)?;
                Ok(response)
            }
            #[cfg(feature = "shared-backend-postgres")]
            ApiBackend::Postgres(_) => {
                let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
                let graph = get_cached_graph(&ledger, &ctx.tenant_id, &cache)?;
                #[allow(clippy::infallible_destructuring_match)]
                let postgres_ledger = match &ledger {
                    #[cfg(feature = "shared-backend-postgres")]
                    ApiLedger::Postgres(l) => l,
                    ApiLedger::Sqlite(_) => unreachable!(), // ubs:ignore: unreachable in Postgres mode
                };
                let query_ctx = QueryContext::new(ctx.tenant_id);
                let response =
                    search_decisions_with_ledger(&query_ctx, postgres_ledger, &*graph, &request)
                        .map_err(to_api_error)?;
                Ok(response)
            }
        }
    })
    .await;

    respond_envelope(result)
}

pub(super) async fn relevant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<RelevantParams>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let status_filter = params.status.as_deref().map(parse_status).transpose()?;
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let response =
            get_relevant_decisions(&*graph, &params.topic, status_filter).map_err(to_api_error)?;
        Ok(response)
    })
    .await;

    respond_envelope(result)
}

pub(super) async fn post_ingest_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<IngestBatchRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result =
        tokio::task::spawn_blocking(move || ingest_batch_blocking(&backend, &ctx, req)).await;

    respond(result, StatusCode::ACCEPTED)
}

const MAX_INGEST_TURNS: usize = 20;

fn ingest_batch_blocking(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    req: IngestBatchRequest,
) -> ApiResult<serde_json::Value> {
    if req.batch_id.trim().is_empty() {
        return Err(ApiError::validation("batch_id must not be empty"));
    }
    if req.agent_tool.trim().is_empty() {
        return Err(ApiError::validation("agent_tool must not be empty"));
    }
    if req.session_id.trim().is_empty() {
        return Err(ApiError::validation("session_id must not be empty"));
    }
    if req.turns.len() > MAX_INGEST_TURNS {
        return Err(ApiError::validation(format!(
            "turns exceeds maximum of {MAX_INGEST_TURNS}"
        )));
    }

    let turns: Vec<IngestTurn> = req
        .turns
        .into_iter()
        .map(|t| IngestTurn {
            turn_id: t.turn_id,
            role: t.role,
            text: t.text,
            truncated: t.truncated,
        })
        .collect();

    let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(
            ctx.tenant_id.clone(),
            EventProvenance::api(Some(ctx.actor_id.clone())),
        ),
    );

    let event_id = commands
        .record_ingest_batch(
            &ctx.actor_id,
            &req.batch_id,
            &req.agent_tool,
            &req.session_id,
            turns,
        )
        .map_err(to_api_error)?;

    Ok(serde_json::json!({
        "batch_id": req.batch_id,
        "event_id": event_id,
        "queued": true,
    }))
}
