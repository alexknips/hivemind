//! HTTP API server — third transport over the same commands and queries layers.
//!
//! Transport: HTTP/1.1 JSON REST (axum). Every endpoint is a thin wrapper over
//! the same [`crate::commands::Commands`] and [`crate::queries`] layers that
//! CLI and MCP use. No layer-3 "smart" behaviour happens here.
//!
//! ## Auth
//!
//! **SQLite dev mode** (`HIVEMIND_DATABASE_URL` unset): bearer token compared
//! in constant time against `HIVEMIND_API_KEY`. When `WORKOS_DOMAIN` is also
//! set, WorkOS JWTs are additionally accepted — the JWT `org_id` claim (or
//! `sub` if absent) is used as the tenant key.
//!
//! **Postgres multi-tenant mode** (`HIVEMIND_DATABASE_URL` set): bearer token
//! is resolved to a `tenant_id` via `TenantStore::resolve_token`. The token
//! encodes which tenant the client belongs to — clients send no tenant header.
//! Admin operations (e.g. provisioning) are guarded by `HIVEMIND_ADMIN_KEY`.
//!
//! ## Endpoints
//!
//! Write:
//! - `POST /v1/decisions`                          — capture decision
//! - `POST /v1/evidence`                           — capture evidence
//! - `POST /v1/hypotheses`                         — capture hypothesis
//! - `POST /v1/decisions/{id}/disagreements`        — disagree
//! - `POST /v1/decisions/{id}/supersessions`        — supersede
//! - `POST /v1/tenants`                            — provision tenant (Postgres, admin only)
//!
//! Read:
//! - `GET  /v1/decisions/{id}`                     — get single decision
//! - `GET  /v1/decisions/{id}/supersession-chain`  — supersession chain
//! - `GET  /v1/decisions/search`                   — full-text search (SQLite only)
//! - `GET  /v1/decisions/relevant`                 — decisions by topic
//! - `GET  /v1/decisions/map[?alpha=0.5]`          — 2-D spectral decision map
//! - `GET  /v1/graph`                              — full decision graph (JSON)
//! - `GET  /v1/health`                             — liveness probe
//!
//! ## SPA serving
//!
//! When `HIVEMIND_SPA_DIR` is set, the server statically serves the built SPA
//! from that directory at `/`. API paths (`/v1/*`, `/mcp`, `/.well-known/*`,
//! `/auth/*`) take priority. Unknown paths fall back to the SPA's `index.html`
//! for client-side routing (same-origin, no CORS needed).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::Json;
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use tower::ServiceBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

use crate::error::{CliError, CommandError, HivemindError};
use crate::events::{EventId, TenantId};
use crate::ledger::{EventLedger, SqliteEventLedger, SqliteUserStore};
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::{PostgresEventLedger, TenantStore};
use crate::projector::memory::MemoryGraph;
use crate::queries::{DecisionStatus, QueryResponse};

use self::auth::{CachedJwks, WorkosConfig};

mod auth;
mod graph;
mod handlers;
mod mcp_http;

type ApiResult<T> = std::result::Result<T, ApiError>;
/// Cache key: (tenant_id, ledger_offset, alpha.to_bits())
type MapCacheKey = (String, u64, u64);
type MapCache = Arc<Mutex<HashMap<MapCacheKey, crate::map::MapResult>>>;
type GraphCache = RwLock<HashMap<TenantId, (EventId, Arc<MemoryGraph>)>>;

const REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_CONCURRENT_REQUESTS: usize = 200;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration assembled at server startup.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub hivemind_dir: PathBuf,
    pub port: u16,
    /// Expected bearer token for SQLite dev mode. `None` = no auth check.
    pub api_key: Option<String>,
    /// Postgres database URL. When set, enables the multi-tenant Postgres
    /// backend with per-tenant bearer tokens and RLS enforcement.
    pub database_url: Option<String>,
    /// Admin token for the `POST /v1/tenants` provisioning endpoint.
    pub admin_key: Option<String>,
    /// WorkOS AuthKit domain (e.g. https://your-tenant.authkit.app).
    /// Set WORKOS_DOMAIN to enable WorkOS OAuth resource-server mode.
    pub workos_domain: Option<String>,
    /// WorkOS OIDC issuer URL (usually same as WORKOS_DOMAIN).
    pub workos_issuer: Option<String>,
    /// WorkOS JWKS endpoint (e.g. https://api.workos.com/sso/jwks).
    pub workos_jwks_url: Option<String>,
    /// Expected JWT audience claims. Comma-separated list of accepted client_ids.
    /// When set, token `aud` must match at least one value. Omit WORKOS_AUDIENCE
    /// to disable audience validation (DCR / mixed SPA+MCP environments).
    pub workos_audience: Option<Vec<String>>,
    /// Directory containing the pre-built SPA. When set, the server serves
    /// static files from this directory at `/`, falling back to `index.html`
    /// for SPA client-side routing. API paths always take precedence.
    pub spa_dir: Option<PathBuf>,
    /// Exact origins allowed to make cross-origin requests (CORS). Empty = no
    /// CORS headers (same-origin only). Populated from HIVEMIND_CORS_ORIGINS
    /// (comma-separated). Self-hosters are unaffected by default.
    pub cors_origins: Vec<String>,
}

impl ApiConfig {
    pub fn new(hivemind_dir: impl Into<PathBuf>) -> Self {
        Self {
            hivemind_dir: hivemind_dir.into(),
            port: 8080,
            api_key: std::env::var("HIVEMIND_API_KEY").ok(),
            database_url: std::env::var("HIVEMIND_DATABASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            admin_key: std::env::var("HIVEMIND_ADMIN_KEY").ok(),
            workos_domain: std::env::var("WORKOS_DOMAIN").ok(),
            workos_issuer: std::env::var("WORKOS_ISSUER").ok(),
            workos_jwks_url: std::env::var("WORKOS_JWKS_URL").ok(),
            workos_audience: std::env::var("WORKOS_AUDIENCE").ok().map(|s| {
                s.split(',')
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .collect()
            }),
            spa_dir: std::env::var("HIVEMIND_SPA_DIR").ok().map(PathBuf::from),
            cors_origins: std::env::var("HIVEMIND_CORS_ORIGINS")
                .ok()
                .map(|s| {
                    s.split(',')
                        .map(|v| v.trim().to_owned())
                        .filter(|v| !v.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

// ---------------------------------------------------------------------------
// App state (shared across handlers via axum State)
// ---------------------------------------------------------------------------

/// Ledger backend — SQLite for local dev, Postgres for the shared service.
enum ApiBackend {
    Sqlite(Arc<PathBuf>),
    #[cfg(feature = "shared-backend-postgres")]
    Postgres(Arc<PostgresEventLedger>),
}

impl ApiBackend {
    /// Return the on-disk directory for SQLite backends; None for Postgres.
    fn sqlite_dir(&self) -> Option<&std::path::Path> {
        match self {
            ApiBackend::Sqlite(dir) => Some(dir.as_ref()),
            #[cfg(feature = "shared-backend-postgres")]
            ApiBackend::Postgres(_) => None,
        }
    }

    /// Open a tenant-scoped ledger for use within a blocking closure.
    fn open_ledger_for_tenant(&self, tenant_id: &TenantId) -> ApiResult<ApiLedger> {
        #[cfg(not(feature = "shared-backend-postgres"))]
        let _ = tenant_id;
        match self {
            ApiBackend::Sqlite(dir) => {
                let ledger = SqliteEventLedger::open(dir.as_ref())
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                Ok(ApiLedger::Sqlite(ledger))
            }
            #[cfg(feature = "shared-backend-postgres")]
            ApiBackend::Postgres(base) => {
                let ledger = base
                    .for_tenant(tenant_id.as_str())
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                Ok(ApiLedger::Postgres(ledger))
            }
        }
    }
}

/// Enum wrapper so handlers dispatch to either backend without monomorphisation.
enum ApiLedger {
    Sqlite(SqliteEventLedger),
    #[cfg(feature = "shared-backend-postgres")]
    Postgres(PostgresEventLedger),
}

impl EventLedger for ApiLedger {
    fn append_for_tenant(
        &self,
        tenant_id: &TenantId,
        event: crate::events::Event,
    ) -> crate::Result<crate::events::EventId> {
        match self {
            ApiLedger::Sqlite(l) => l.append_for_tenant(tenant_id, event),
            // Use explicit trait dispatch to avoid the inherent &str overload.
            #[cfg(feature = "shared-backend-postgres")]
            ApiLedger::Postgres(l) => EventLedger::append_for_tenant(l, tenant_id, event),
        }
    }

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: crate::events::EventId,
        limit: usize,
    ) -> crate::Result<Vec<crate::events::Event>> {
        match self {
            ApiLedger::Sqlite(l) => l.read_for_tenant(tenant_id, offset, limit),
            #[cfg(feature = "shared-backend-postgres")]
            ApiLedger::Postgres(l) => EventLedger::read_for_tenant(l, tenant_id, offset, limit),
        }
    }

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: crate::events::EventId,
        callback: &mut dyn FnMut(&crate::events::Event) -> crate::Result<()>,
    ) -> crate::Result<()> {
        match self {
            ApiLedger::Sqlite(l) => l.replay_from_for_tenant(tenant_id, offset, callback),
            #[cfg(feature = "shared-backend-postgres")]
            ApiLedger::Postgres(l) => {
                EventLedger::replay_from_for_tenant(l, tenant_id, offset, callback)
            }
        }
    }

    fn latest_offset_for_tenant(
        &self,
        tenant_id: &TenantId,
    ) -> crate::Result<crate::events::EventId> {
        match self {
            ApiLedger::Sqlite(l) => l.latest_offset_for_tenant(tenant_id),
            #[cfg(feature = "shared-backend-postgres")]
            ApiLedger::Postgres(l) => EventLedger::latest_offset_for_tenant(l, tenant_id),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    backend: Arc<ApiBackend>,
    /// Single-token dev auth (SQLite mode).
    api_key: Option<String>,
    /// Admin key for provisioning endpoints (`POST /v1/tenants`, `POST /v1/users`).
    admin_key: Option<String>,
    /// Per-user token store for SQLite mode.
    sqlite_user_store: Option<Arc<SqliteUserStore>>,
    /// Token store for per-tenant bearer token resolution (Postgres mode).
    #[cfg(feature = "shared-backend-postgres")]
    tenant_store: Option<Arc<TenantStore>>,
    /// WorkOS OAuth resource-server config (set when WORKOS_DOMAIN is configured).
    workos_config: Option<WorkosConfig>,
    /// Pre-computed WorkOS signing keys; refreshed on unknown-kid. Empty if WorkOS not configured.
    workos_jwks: Arc<RwLock<CachedJwks>>,
    /// Offset-keyed projected graph cache; avoids full ledger replay per request.
    graph_cache: Arc<GraphCache>,
    /// Pre-built SPA directory to serve at `/`. None = no SPA serving.
    spa_dir: Option<PathBuf>,
    /// Allowed CORS origins — empty means no CORS headers (same-origin only).
    cors_origins: Vec<String>,
    /// In-memory cache for GET /v1/decisions/map. Key: (tenant_id, ledger_offset, alpha.to_bits()).
    /// Invalidated automatically when the offset advances (new key → cache miss → recompute).
    map_cache: MapCache,
}

impl AppState {
    pub fn from_config(config: &ApiConfig) -> crate::Result<Self> {
        // Fetch WorkOS config and JWKS once at startup (blocking I/O, pre-tokio).
        let (workos_config, cached_jwks) = auth::build_workos_state(config);
        let workos_jwks = Arc::new(RwLock::new(cached_jwks));

        #[cfg(feature = "shared-backend-postgres")]
        if let Some(ref url) = config.database_url {
            let ledger = PostgresEventLedger::connect(url, "provisioning")?;
            let store = TenantStore::connect(url)?;
            return Ok(Self {
                backend: Arc::new(ApiBackend::Postgres(Arc::new(ledger))),
                api_key: None,
                admin_key: config.admin_key.clone(),
                sqlite_user_store: None,
                tenant_store: Some(Arc::new(store)),
                workos_config: workos_config.clone(),
                workos_jwks: Arc::clone(&workos_jwks),
                graph_cache: Arc::new(RwLock::new(HashMap::new())),
                spa_dir: config.spa_dir.clone(),
                cors_origins: config.cors_origins.clone(),
                map_cache: Arc::new(Mutex::new(HashMap::new())),
            });
        }

        let sqlite_user_store = SqliteUserStore::open(&config.hivemind_dir)
            .ok()
            .map(Arc::new);

        Ok(Self {
            backend: Arc::new(ApiBackend::Sqlite(Arc::new(config.hivemind_dir.clone()))),
            api_key: config.api_key.clone(),
            admin_key: config.admin_key.clone(),
            sqlite_user_store,
            #[cfg(feature = "shared-backend-postgres")]
            tenant_store: None,
            workos_config,
            workos_jwks,
            graph_cache: Arc::new(RwLock::new(HashMap::new())),
            spa_dir: config.spa_dir.clone(),
            cors_origins: config.cors_origins.clone(),
            map_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum ApiError {
    Unauthorized(String),
    NotFound(String),
    Validation(String),
    Internal(String),
}

impl ApiError {
    fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized(m)
            | ApiError::NotFound(m)
            | ApiError::Validation(m)
            | ApiError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, "unauthorized", msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg),
            ApiError::Validation(msg) => (StatusCode::BAD_REQUEST, "validation_error", msg),
            ApiError::Internal(msg) => {
                tracing::error!(target: "hivemind::api", error = %msg, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", msg)
            }
        };
        (
            status,
            Json(serde_json::json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

fn to_api_error(error: HivemindError) -> ApiError {
    match error {
        HivemindError::Command(CommandError::Validation(msg)) => ApiError::Validation(msg),
        HivemindError::Command(CommandError::Invariant(msg)) if msg.contains("does not exist") => {
            ApiError::not_found(msg)
        }
        HivemindError::Cli(CliError::InvalidInput(msg)) => ApiError::Validation(msg),
        other => ApiError::Internal(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Request context resolved from auth + routing headers
// ---------------------------------------------------------------------------

const DEFAULT_ACTOR: &str = "service:api";
const HEADER_TENANT: &str = "x-hivemind-tenant";
const HEADER_ACTOR: &str = "x-hivemind-actor";
const HEADER_AUTHORIZATION: &str = "authorization";
const HEADER_FORWARDED_PROTO: &str = "x-forwarded-proto";

#[derive(Clone)]
struct ApiRequestCtx {
    tenant_id: TenantId,
    actor_id: String,
}

// ---------------------------------------------------------------------------
// Router — public so tests can call create_router without binding a port
// ---------------------------------------------------------------------------

pub fn create_router(config: &ApiConfig) -> Router {
    let state = AppState::from_config(config)
        .expect("failed to initialize API backend; check database URL");
    build_router(state)
}

fn build_cors_layer(origins: &[String]) -> Option<CorsLayer> {
    let parsed: Vec<axum::http::HeaderValue> =
        origins.iter().filter_map(|o| o.parse().ok()).collect();
    if parsed.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(parsed))
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
            .max_age(std::time::Duration::from_secs(3600)),
    )
}

fn build_router(state: AppState) -> Router {
    let cors = build_cors_layer(&state.cors_origins);
    let router = Router::new()
        .route("/v1/health", get(handlers::health_handler))
        // Static routes before dynamic /:id to avoid ambiguity
        .route("/v1/decisions/search", get(handlers::search_handler))
        .route("/v1/decisions/relevant", get(handlers::relevant_handler))
        .route("/v1/decisions/map", get(handlers::map_handler))
        // Decision resource routes
        .route("/v1/decisions", post(handlers::post_decisions_handler))
        .route("/v1/decisions/{id}", get(handlers::get_decision_handler))
        .route(
            "/v1/decisions/{id}/supersession-chain",
            get(handlers::supersession_chain_handler),
        )
        .route(
            "/v1/decisions/{id}/compact-view",
            get(handlers::compact_view_handler),
        )
        .route(
            "/v1/decisions/{id}/disagreements",
            post(handlers::disagree_handler),
        )
        .route(
            "/v1/decisions/{id}/supersessions",
            post(handlers::supersede_handler),
        )
        // Evidence and hypotheses
        .route("/v1/evidence", post(handlers::post_evidence_handler))
        .route("/v1/hypotheses", post(handlers::post_hypotheses_handler))
        // Transcript ingest (capture client → server)
        .route("/v1/ingest", post(handlers::post_ingest_handler))
        // MCP Streamable HTTP transport (2025-03-26)
        .route("/mcp", post(mcp_http::mcp_http_handler))
        // OAuth resource/authorization server metadata (MCP auth spec)
        .route(
            "/.well-known/oauth-protected-resource",
            get(auth::oauth_protected_resource_handler),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(auth::oauth_authorization_server_handler),
        )
        // Full decision graph (read layer — no layer-3 inference)
        .route("/v1/graph", get(graph::graph_handler));

    #[cfg(feature = "shared-backend-postgres")]
    let router = router.route("/v1/tenants", post(auth::provision_tenant_handler));

    let router = router
        .route(
            "/v1/users",
            post(auth::create_user_handler).get(auth::list_users_handler),
        )
        .route(
            "/v1/users/{user_id}/tokens",
            post(auth::mint_user_token_handler),
        )
        .route(
            "/v1/users/{user_id}/tokens/{token_id}",
            delete(auth::revoke_token_handler),
        );

    // SPA static serving: API routes above take precedence via axum route order.
    // Non-API paths fall back to the SPA's index.html for client-side routing.
    let router = if let Some(ref spa_dir) = state.spa_dir {
        let index = spa_dir.join("index.html");
        let serve = ServeDir::new(spa_dir).fallback(ServeFile::new(index));
        router.with_state(state).fallback_service(serve)
    } else {
        router.with_state(state)
    };

    let router = if let Some(cors) = cors {
        router.layer(cors)
    } else {
        router
    };

    // HandleError (outermost) → Timeout (covers queue + processing) → ConcurrencyLimit (innermost).
    // ServiceBuilder stacks outermost-first so HandleError catches Elapsed from inner layers.
    router.layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(|err: tower::BoxError| async move {
                if err.is::<tower::timeout::error::Elapsed>() {
                    (StatusCode::REQUEST_TIMEOUT, "request timed out")
                } else {
                    (StatusCode::SERVICE_UNAVAILABLE, "service unavailable")
                }
            }))
            .layer(TimeoutLayer::new(Duration::from_secs(REQUEST_TIMEOUT_SECS)))
            .layer(GlobalConcurrencyLimitLayer::new(MAX_CONCURRENT_REQUESTS)),
    )
}

/// Bind to `config.port` and serve until SIGINT/SIGTERM.
///
/// `state` must be built before entering the tokio runtime (e.g. via
/// `AppState::from_config`) to avoid the "cannot start a runtime from within
/// a runtime" panic that r2d2/postgres triggers when pool construction runs
/// inside an existing async context.
pub async fn serve_http(state: AppState, config: &ApiConfig) -> crate::Result<()> {
    if config.api_key.is_none() && config.database_url.is_none() {
        warn!(
            target: "hivemind::api",
            "HIVEMIND_API_KEY and HIVEMIND_DATABASE_URL not set — running in development mode (no auth)"
        );
    }

    crate::classifier::try_spawn(
        Arc::new(config.hivemind_dir.clone()),
        crate::events::TenantId::local(),
    );
    crate::scorer::try_spawn(
        Arc::new(config.hivemind_dir.clone()),
        crate::events::TenantId::local(),
    );

    let app = build_router(state);
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| CliError::InvalidInput(format!("failed to bind {addr}: {e}")))?;

    tracing::info!(
        target: "hivemind::api",
        addr = %listener.local_addr().unwrap(),
        "HTTP API listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| CliError::InvalidInput(format!("server error: {e}")).into())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = sigterm => {},
    }
}

// ---------------------------------------------------------------------------
// Helpers shared across blocking closures
// ---------------------------------------------------------------------------

fn parse_status(value: &str) -> ApiResult<DecisionStatus> {
    match value {
        "proposed" => Ok(DecisionStatus::Proposed),
        "accepted" => Ok(DecisionStatus::Accepted),
        "rejected" => Ok(DecisionStatus::Rejected),
        "contested" => Ok(DecisionStatus::Contested),
        "superseded" => Ok(DecisionStatus::Superseded),
        other => Err(ApiError::validation(format!("unknown status `{other}`"))),
    }
}

fn parse_datetime(value: &str) -> std::result::Result<DateTime<Utc>, chrono::format::ParseError> {
    DateTime::parse_from_rfc3339(value).map(|dt| dt.with_timezone(&Utc))
}

#[derive(Serialize)]
struct QueryEnvelope<T: Serialize> {
    result_count: usize,
    truncated: bool,
    latency_ms: u128,
    data: T,
}

fn query_envelope<T: Serialize>(response: crate::queries::QueryResponse<T>) -> QueryEnvelope<T> {
    QueryEnvelope {
        result_count: response.result_count,
        truncated: response.truncated,
        latency_ms: response.latency_ms,
        data: response.data,
    }
}

fn respond<T: Serialize>(
    result: std::result::Result<ApiResult<T>, tokio::task::JoinError>,
    status: StatusCode,
) -> Response {
    match result {
        Ok(Ok(body)) => (status, Json(body)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

fn respond_envelope<T: Serialize>(
    result: std::result::Result<ApiResult<QueryResponse<T>>, tokio::task::JoinError>,
) -> Response {
    match result {
        Ok(Ok(view)) => (StatusCode::OK, Json(query_envelope(view))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}
