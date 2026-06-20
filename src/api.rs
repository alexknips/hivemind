//! HTTP API server — third transport over the same commands and queries layers.
//!
//! Transport: HTTP/1.1 JSON REST (axum). Every endpoint is a thin wrapper over
//! the same [`crate::commands::Commands`] and [`crate::queries`] layers that
//! CLI and MCP use. No layer-3 "smart" behaviour happens here.
//!
//! ## Auth
//!
//! **SQLite dev mode** (`HIVEMIND_DATABASE_URL` unset): bearer token compared
//! in constant time against `HIVEMIND_API_KEY`. Tenant identity comes from
//! `X-HiveMind-Tenant` header.
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
//! - `POST /auth/revoke`                           — revoke WorkOS sub (Postgres, admin only)
//!
//! Read:
//! - `GET  /v1/decisions/{id}`                     — get single decision
//! - `GET  /v1/decisions/{id}/supersession-chain`  — supersession chain
//! - `GET  /v1/decisions/search`                   — full-text search (SQLite only)
//! - `GET  /v1/decisions/relevant`                 — decisions by topic
//! - `GET  /v1/health`                             — liveness probe

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use jsonwebtoken::jwk::JwkSet;
#[cfg(feature = "shared-backend-postgres")]
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::commands::{CommandContext, Commands};
use crate::error::{CliError, CommandError, HivemindError};
use crate::events::{EventProvenance, IngestTurn, TenantId};
use crate::ledger::{EventLedger, SqliteEventLedger};
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::{PostgresEventLedger, TenantStore};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::{
    derive_decision_status, get_compact_view, get_decision, get_relevant_decisions,
    get_supersession_chain, search_decisions_fts_with_context, DecisionStatus, QueryContext,
    SearchDecisionRequest,
};

type ApiResult<T> = std::result::Result<T, ApiError>;

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
    /// Expected JWT audience claim. When set, token `aud` is validated.
    /// With DCR the client_id is dynamic; omitting WORKOS_AUDIENCE disables
    /// audience validation — set this to the client_id for stricter enforcement.
    pub workos_audience: Option<String>,
}

impl ApiConfig {
    pub fn new(hivemind_dir: impl Into<PathBuf>) -> Self {
        Self {
            hivemind_dir: hivemind_dir.into(),
            port: 8080,
            api_key: std::env::var("HIVEMIND_API_KEY").ok(),
            database_url: std::env::var("HIVEMIND_DATABASE_URL").ok(),
            admin_key: std::env::var("HIVEMIND_ADMIN_KEY").ok(),
            workos_domain: std::env::var("WORKOS_DOMAIN").ok(),
            workos_issuer: std::env::var("WORKOS_ISSUER").ok(),
            workos_jwks_url: std::env::var("WORKOS_JWKS_URL").ok(),
            workos_audience: std::env::var("WORKOS_AUDIENCE").ok(),
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

/// WorkOS OAuth resource-server configuration (populated from env vars at startup).
#[derive(Clone)]
struct WorkosConfig {
    /// AuthKit domain, e.g. `https://your-tenant.authkit.app`.
    domain: String,
    /// OIDC issuer for JWT `iss` claim validation.
    #[cfg_attr(not(feature = "shared-backend-postgres"), allow(dead_code))]
    issuer: String,
    /// JWKS endpoint URL, stored for background refresh.
    jwks_url: String,
    /// Expected JWT `aud` claim. `None` disables audience validation (DCR mode).
    #[cfg_attr(not(feature = "shared-backend-postgres"), allow(dead_code))]
    audience: Option<String>,
}

/// Claims extracted from a validated WorkOS access token.
#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, serde::Deserialize)]
struct WorkosClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    backend: Arc<ApiBackend>,
    /// Single-token dev auth (SQLite mode).
    api_key: Option<String>,
    /// Admin key for `POST /v1/tenants`.
    #[cfg(feature = "shared-backend-postgres")]
    admin_key: Option<String>,
    /// Token store for per-tenant bearer token resolution (Postgres mode).
    #[cfg(feature = "shared-backend-postgres")]
    tenant_store: Option<Arc<TenantStore>>,
    /// WorkOS OAuth resource-server config (set when WORKOS_DOMAIN is configured).
    workos_config: Option<WorkosConfig>,
    /// Cached WorkOS JWKS. Refreshed in the background every WORKOS_JWKS_REFRESH_SECS.
    workos_jwks: Arc<RwLock<JwkSet>>,
}

impl AppState {
    pub fn from_config(config: &ApiConfig) -> crate::Result<Self> {
        // Fetch WorkOS JWKS once at startup (called before tokio runtime).
        let (workos_config, workos_jwks) = build_workos_state(config);

        #[cfg(feature = "shared-backend-postgres")]
        if let Some(ref url) = config.database_url {
            let ledger = PostgresEventLedger::connect(url, "provisioning")?;
            let store = TenantStore::connect(url)?;
            return Ok(Self {
                backend: Arc::new(ApiBackend::Postgres(Arc::new(ledger))),
                api_key: None,
                admin_key: config.admin_key.clone(),
                tenant_store: Some(Arc::new(store)),
                workos_config,
                workos_jwks,
            });
        }

        Ok(Self {
            backend: Arc::new(ApiBackend::Sqlite(Arc::new(config.hivemind_dir.clone()))),
            api_key: config.api_key.clone(),
            #[cfg(feature = "shared-backend-postgres")]
            admin_key: config.admin_key.clone(),
            #[cfg(feature = "shared-backend-postgres")]
            tenant_store: None,
            workos_config,
            workos_jwks,
        })
    }
}

/// Build WorkOS resource-server config and pre-fetch JWKS.
/// Called before the tokio runtime starts, so blocking I/O is safe.
fn build_workos_state(config: &ApiConfig) -> (Option<WorkosConfig>, Arc<RwLock<JwkSet>>) {
    let (Some(domain), Some(issuer), Some(jwks_url)) = (
        config.workos_domain.as_deref(),
        config
            .workos_issuer
            .as_deref()
            .or(config.workos_domain.as_deref()),
        config.workos_jwks_url.as_deref(),
    ) else {
        return (None, Arc::new(RwLock::new(JwkSet { keys: vec![] })));
    };

    let workos_cfg = WorkosConfig {
        domain: domain.to_owned(),
        issuer: issuer.to_owned(),
        jwks_url: jwks_url.to_owned(),
        audience: config.workos_audience.clone(),
    };

    if workos_cfg.audience.is_none() {
        tracing::warn!(
            target: "hivemind::api",
            "WORKOS_AUDIENCE not set — aud claim unchecked; \
             set WORKOS_AUDIENCE to your WorkOS client_id to prevent cross-resource token reuse"
        );
    }

    match reqwest::blocking::get(jwks_url).and_then(|r| r.json::<JwkSet>()) {
        Ok(jwks) => {
            tracing::info!(target: "hivemind::api", keys = jwks.keys.len(), "WorkOS JWKS loaded");
            (Some(workos_cfg), Arc::new(RwLock::new(jwks)))
        }
        Err(e) => {
            tracing::warn!(target: "hivemind::api", error = %e, "WorkOS JWKS fetch failed — JWT auth disabled");
            (
                Some(workos_cfg),
                Arc::new(RwLock::new(JwkSet { keys: vec![] })),
            )
        }
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

fn map_err(error: HivemindError) -> ApiError {
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

#[derive(Clone)]
struct ApiRequestCtx {
    tenant_id: TenantId,
    actor_id: String,
}

fn extract_ctx(state: &AppState, headers: &HeaderMap) -> ApiResult<ApiRequestCtx> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    // WorkOS JWT path: bearer that looks like a JWT ("ey...") + WorkOS configured.
    // Try this first so WorkOS tokens don't fall through to opaque-token resolution.
    #[cfg(feature = "shared-backend-postgres")]
    if bearer.starts_with("ey") {
        if let (Some(ref workos), Some(ref store)) = (&state.workos_config, &state.tenant_store) {
            let jwks_guard = state.workos_jwks.read().expect("JWKS lock poisoned");
            if jwks_guard.keys.is_empty() {
                return Err(ApiError::unauthorized(
                    "WorkOS JWKS not loaded — cannot validate JWT",
                ));
            }
            let claims = validate_workos_jwt(
                bearer,
                &jwks_guard,
                &workos.issuer,
                workos.audience.as_deref(),
            )
            .map_err(ApiError::unauthorized)?;
            drop(jwks_guard);
            if store
                .is_sub_revoked(&claims.sub)
                .map_err(|e| ApiError::internal(e.to_string()))?
            {
                return Err(ApiError::unauthorized("session revoked"));
            }

            let tenant_id_str = store
                .resolve_or_create_oidc_user(
                    &claims.sub,
                    claims.email.as_deref().unwrap_or(&claims.sub),
                )
                .map_err(|e| ApiError::internal(e.to_string()))?;

            let tenant_id = TenantId::new(&tenant_id_str)
                .map_err(|_| ApiError::internal("invalid tenant_id from OIDC mapping"))?;
            let actor_id = format!("human:{}", claims.email.as_deref().unwrap_or(&claims.sub));
            return Ok(ApiRequestCtx {
                tenant_id,
                actor_id,
            });
        }
    }

    // Postgres mode: when WorkOS is configured, browser-OAuth JWT is the only
    // accepted credential — there is no static-token fallback. Interactive
    // agents authenticate via WorkOS; headless flows use the device-approval
    // path (future), not a pre-shared secret.
    #[cfg(feature = "shared-backend-postgres")]
    if let Some(ref store) = state.tenant_store {
        if state.workos_config.is_some() {
            return Err(ApiError::unauthorized(
                "WorkOS browser-OAuth JWT required (use GitHub or Google login)",
            ));
        }
        // Postgres without WorkOS: existing opaque-token resolution for tenants
        // provisioned via POST /v1/tenants (admin-gated provisioning path).
        if bearer.is_empty() {
            return Err(ApiError::unauthorized("bearer token required"));
        }
        let tenant_id_str = store
            .resolve_token(bearer)
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::unauthorized("invalid or missing bearer token"))?;
        let tenant_id = TenantId::new(&tenant_id_str)
            .map_err(|_| ApiError::internal("invalid tenant_id from token store"))?;
        let actor_id = headers
            .get(HEADER_ACTOR)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(DEFAULT_ACTOR)
            .to_owned();
        return Ok(ApiRequestCtx {
            tenant_id,
            actor_id,
        });
    }

    // SQLite dev mode: single-key constant-time comparison.
    if let Some(expected) = &state.api_key {
        if !constant_time_eq(bearer, expected) {
            return Err(ApiError::unauthorized("invalid or missing bearer token"));
        }
    }

    let tenant_str = headers
        .get(HEADER_TENANT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(TenantId::LOCAL_VALUE);
    let tenant_id = TenantId::new(tenant_str)
        .map_err(|_| ApiError::validation("X-HiveMind-Tenant must not be empty"))?;

    let actor_id = headers
        .get(HEADER_ACTOR)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_ACTOR)
        .to_owned();

    Ok(ApiRequestCtx {
        tenant_id,
        actor_id,
    })
}

/// Validate a WorkOS JWT access token against the cached JWKS.
///
/// Validates signature (RS256/ES256), issuer, and expiry. When `audience` is
/// `Some`, the JWT `aud` claim is also validated — set `WORKOS_AUDIENCE` to
/// the WorkOS client_id for production to prevent cross-resource token reuse.
#[cfg(feature = "shared-backend-postgres")]
fn validate_workos_jwt(
    token: &str,
    jwks: &JwkSet,
    issuer: &str,
    audience: Option<&str>,
) -> Result<WorkosClaims, String> {
    let header = decode_header(token).map_err(|_| "invalid token")?;

    // Find the JWK by kid, or fall back to the first key if kid is absent.
    let kid = header.kid.as_deref().unwrap_or("");
    let jwk = if kid.is_empty() {
        jwks.keys.first()
    } else {
        jwks.find(kid)
    }
    .ok_or("no matching signing key")?;

    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|_| "invalid signing key")?;

    let alg = match header.alg {
        Algorithm::RS256 => Algorithm::RS256,
        Algorithm::RS384 => Algorithm::RS384,
        Algorithm::RS512 => Algorithm::RS512,
        Algorithm::ES256 => Algorithm::ES256,
        Algorithm::ES384 => Algorithm::ES384,
        _ => return Err("unsupported token algorithm".to_owned()),
    };

    let mut validation = Validation::new(alg);
    validation.set_issuer(&[issuer]);
    validation.validate_exp = true;
    match audience {
        Some(aud) => validation.set_audience(&[aud]),
        // ubs:ignore: DCR clients have dynamic client_ids; set WORKOS_AUDIENCE for strict aud enforcement
        None => validation.validate_aud = false,
    }

    // ubs:ignore: sig+iss+exp always validated; aud validated when WORKOS_AUDIENCE set
    decode::<WorkosClaims>(token, &decoding_key, &validation)
        .map(|d| d.claims)
        .map_err(|_| "token validation failed".to_owned())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OptionInput {
    label: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CaptureDecisionRequest {
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
struct CaptureEvidenceRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
struct CaptureHypothesisRequest {
    statement: String,
}

#[derive(Debug, Deserialize)]
struct DisagreeRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct SupersedeRequest {
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
struct IngestBatchRequest {
    batch_id: String,
    agent_tool: String,
    session_id: String,
    #[serde(default)]
    turns: Vec<IngestTurnRequest>,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
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
struct RelevantParams {
    topic: String,
    status: Option<String>,
}

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Deserialize)]
struct ProvisionTenantRequest {
    tenant_id: String,
    display_name: String,
}

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Deserialize)]
struct RevokeUserRequest {
    sub: String,
    reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Router — public so tests can call create_router without binding a port
// ---------------------------------------------------------------------------

pub fn create_router(config: &ApiConfig) -> Router {
    let state = AppState::from_config(config)
        .expect("failed to initialize API backend; check database URL");
    build_router(state)
}

fn build_router(state: AppState) -> Router {
    let router = Router::new()
        .route("/v1/health", get(health_handler))
        // Static routes before dynamic /:id to avoid ambiguity
        .route("/v1/decisions/search", get(search_handler))
        .route("/v1/decisions/relevant", get(relevant_handler))
        // Decision resource routes
        .route("/v1/decisions", post(post_decisions_handler))
        .route("/v1/decisions/{id}", get(get_decision_handler))
        .route(
            "/v1/decisions/{id}/supersession-chain",
            get(supersession_chain_handler),
        )
        .route("/v1/decisions/{id}/compact-view", get(compact_view_handler))
        .route("/v1/decisions/{id}/disagreements", post(disagree_handler))
        .route("/v1/decisions/{id}/supersessions", post(supersede_handler))
        // Evidence and hypotheses
        .route("/v1/evidence", post(post_evidence_handler))
        .route("/v1/hypotheses", post(post_hypotheses_handler))
        // Transcript ingest (capture client → server)
        .route("/v1/ingest", post(post_ingest_handler))
        // MCP Streamable HTTP transport (2025-03-26)
        .route("/mcp", post(mcp_http_handler))
        // OAuth resource/authorization server metadata (MCP auth spec)
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth_protected_resource_handler),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_authorization_server_handler),
        );

    #[cfg(feature = "shared-backend-postgres")]
    let router = router.route("/v1/tenants", post(provision_tenant_handler));

    #[cfg(feature = "shared-backend-postgres")]
    let router = router.route("/auth/revoke", post(revoke_user_handler));

    router.with_state(state)
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

    // Extract JWKS refresh params before state is consumed by build_router.
    let jwks_refresh = state
        .workos_config
        .as_ref()
        .map(|cfg| (cfg.jwks_url.clone(), Arc::clone(&state.workos_jwks)));

    crate::classifier::try_spawn(
        Arc::new(config.hivemind_dir.clone()),
        crate::events::TenantId::local(),
    );

    let app = build_router(state);

    // Spawn background JWKS refresh so WorkOS key rotation does not require restart.
    if let Some((jwks_url, jwks_lock)) = jwks_refresh {
        let refresh_secs: u64 = std::env::var("WORKOS_JWKS_REFRESH_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1800);
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(refresh_secs);
            loop {
                tokio::time::sleep(interval).await;
                match reqwest::get(&jwks_url).await {
                    Ok(resp) => match resp.json::<JwkSet>().await {
                        Ok(new_jwks) => {
                            *jwks_lock.write().expect("JWKS lock poisoned") = new_jwks;
                            tracing::info!(target: "hivemind::api", "WorkOS JWKS refreshed");
                        }
                        Err(e) => tracing::warn!(
                            target: "hivemind::api",
                            error = %e,
                            "WorkOS JWKS refresh: JSON parse failed"
                        ),
                    },
                    Err(e) => tracing::warn!(
                        target: "hivemind::api",
                        error = %e,
                        "WorkOS JWKS refresh: fetch failed"
                    ),
                }
            }
        });
    }

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
// Handlers
// ---------------------------------------------------------------------------

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
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

async fn post_decisions_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CaptureDecisionRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
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

    unpack_blocking(result)
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
            .map_err(map_err)?;

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
        .propose_decision(
            &ctx.actor_id,
            &req.title,
            &req.rationale,
            &req.topic_keys,
            &option_ids,
            chosen_option_id.as_deref(),
            &req.hypothesis_ids,
            &req.evidence_ids,
        )
        .map_err(map_err)?;

    Ok(serde_json::json!({
        "decision_id": decision_id,
        "option_ids": option_ids,
        "chosen_option_id": chosen_option_id,
    }))
}

async fn post_evidence_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CaptureEvidenceRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
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
            .map_err(map_err)?;
        Ok(serde_json::json!({ "evidence_id": evidence_id }))
    })
    .await;

    unpack_blocking(result)
}

async fn post_hypotheses_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CaptureHypothesisRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
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
            .map_err(map_err)?;
        Ok(serde_json::json!({ "hypothesis_id": hypothesis_id }))
    })
    .await;

    unpack_blocking(result)
}

async fn disagree_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
    payload: std::result::Result<Json<DisagreeRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
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
            .map_err(map_err)?;

        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id)?;
        let decision_status = derive_decision_status(&graph, &decision_id).map_err(map_err)?;

        Ok(serde_json::json!({
            "decision_id": decision_id,
            "event_id": event_id,
            "decision_status": decision_status,
        }))
    })
    .await;

    unpack_blocking(result)
}

async fn supersede_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(old_decision_id): Path<String>,
    payload: std::result::Result<Json<SupersedeRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
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
            .supersede(
                &ctx.actor_id,
                &old_decision_id,
                &req.title,
                &req.rationale,
                &req.topic_keys,
                &req.options,
                req.chosen_option_label.as_deref(),
                &req.hypothesis_ids,
                &req.evidence_ids,
            )
            .map_err(map_err)?;

        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id)?;
        let old_status = derive_decision_status(&graph, &old_decision_id).map_err(map_err)?;
        let new_status =
            derive_decision_status(&graph, &outcome.new_decision_id).map_err(map_err)?;

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

    unpack_blocking(result)
}

async fn get_decision_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id)?;
        let view = get_decision(&graph, &decision_id).map_err(map_err)?;
        Ok(view)
    })
    .await;

    match result {
        Ok(Ok(view)) => (StatusCode::OK, Json(query_envelope(view))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

async fn supersession_chain_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id)?;
        let exists = get_decision(&graph, &decision_id).map_err(map_err)?;
        if exists.data.is_none() {
            return Err(ApiError::not_found(format!(
                "decision not found: {decision_id}"
            )));
        }
        let response = get_supersession_chain(&graph, &decision_id).map_err(map_err)?;
        Ok(response)
    })
    .await;

    match result {
        Ok(Ok(view)) => (StatusCode::OK, Json(query_envelope(view))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

async fn compact_view_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id)?;
        let response = get_compact_view(&graph, &decision_id).map_err(map_err)?;
        Ok(response)
    })
    .await;

    match result {
        Ok(Ok(view)) => (StatusCode::OK, Json(query_envelope(view))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

async fn search_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
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
            ApiBackend::Sqlite(dir) => {
                let ledger = SqliteEventLedger::open(dir.as_ref())
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                let graph = MemoryGraph::default();
                rebuild_graph_for_tenant(&ledger, &ctx.tenant_id, &graph)
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                let query_ctx = QueryContext::new(ctx.tenant_id);
                let response =
                    search_decisions_fts_with_context(&query_ctx, &ledger, &graph, &request)
                        .map_err(map_err)?;
                Ok(response)
            }
            #[cfg(feature = "shared-backend-postgres")]
            ApiBackend::Postgres(_) => Err(ApiError::validation(
                "full-text search is not available in shared-backend mode; \
                 use GET /v1/decisions/relevant for topic-based queries",
            )),
        }
    })
    .await;

    match result {
        Ok(Ok(view)) => (StatusCode::OK, Json(query_envelope(view))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

async fn relevant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<RelevantParams>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let status_filter = params.status.as_deref().map(parse_status).transpose()?;
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id)?;
        let response =
            get_relevant_decisions(&graph, &params.topic, status_filter).map_err(map_err)?;
        Ok(response)
    })
    .await;

    match result {
        Ok(Ok(view)) => (StatusCode::OK, Json(query_envelope(view))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

async fn post_ingest_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<IngestBatchRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
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

    match result {
        Ok(Ok(body)) => (StatusCode::ACCEPTED, Json(body)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
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
        .map_err(map_err)?;

    Ok(serde_json::json!({
        "batch_id": req.batch_id,
        "event_id": event_id,
        "queued": true,
    }))
}

/// Provision a new tenant and issue its initial bearer token.
/// Requires the `Authorization: Bearer <HIVEMIND_ADMIN_KEY>` header.
/// Only available when the `shared-backend-postgres` feature is enabled.
#[cfg(feature = "shared-backend-postgres")]
async fn provision_tenant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<ProvisionTenantRequest>, JsonRejection>,
) -> Response {
    // Admin key gate (separate from per-tenant bearer tokens).
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    match &state.admin_key {
        None => {
            return ApiError::internal("HIVEMIND_ADMIN_KEY not configured").into_response();
        }
        Some(expected) => {
            if !constant_time_eq(provided, expected) {
                return ApiError::unauthorized("invalid admin key").into_response();
            }
        }
    }

    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let store = match &state.tenant_store {
        Some(s) => Arc::clone(s),
        None => return ApiError::internal("tenant store not initialized").into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let provisioned = store
            .provision_tenant(&req.tenant_id, &req.display_name)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok::<_, ApiError>(serde_json::json!({
            "tenant_id": provisioned.tenant_id,
            "token_id": provisioned.token_id,
            "token_secret": provisioned.token_secret,
        }))
    })
    .await;

    match result {
        Ok(Ok(body)) => (StatusCode::CREATED, Json(body)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

/// Revoke a WorkOS user by `sub`. Future JWTs for this sub are rejected with
/// 401 even if they have not expired. Requires `HIVEMIND_ADMIN_KEY`.
#[cfg(feature = "shared-backend-postgres")]
async fn revoke_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<RevokeUserRequest>, JsonRejection>,
) -> Response {
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    match &state.admin_key {
        None => {
            return ApiError::internal("HIVEMIND_ADMIN_KEY not configured").into_response();
        }
        Some(expected) => {
            if !constant_time_eq(provided, expected) {
                return ApiError::unauthorized("invalid admin key").into_response();
            }
        }
    }

    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let store = match &state.tenant_store {
        Some(s) => Arc::clone(s),
        None => return ApiError::internal("tenant store not initialized").into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        store
            .revoke_sub(&req.sub, req.reason.as_deref())
            .map_err(|e| ApiError::internal(e.to_string()))
    })
    .await;

    match result {
        Ok(Ok(())) => {
            (StatusCode::OK, Json(serde_json::json!({ "revoked": true }))).into_response()
        }
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Helpers shared across blocking closures
// ---------------------------------------------------------------------------

fn open_graph_from_ledger(ledger: &ApiLedger, tenant_id: &TenantId) -> ApiResult<MemoryGraph> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(graph)
}

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

fn unpack_blocking(
    result: std::result::Result<ApiResult<serde_json::Value>, tokio::task::JoinError>,
) -> Response {
    match result {
        Ok(Ok(body)) => (StatusCode::OK, Json(body)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => ApiError::internal(e.to_string()).into_response(),
    }
}

// ---------------------------------------------------------------------------
// MCP Streamable HTTP transport (POST /mcp, 2025-03-26 spec)
// ---------------------------------------------------------------------------
//
// Each POST carries a single JSON-RPC 2.0 request and receives a single
// JSON-RPC 2.0 response (no SSE in this implementation — MCP allows plain
// JSON for non-streaming tools).  Auth uses the same bearer-token path as
// the REST API.  The `Mcp-Session-Id` header is issued on `initialize` and
// accepted (but not enforced) on subsequent requests; it seeds the default
// actor_id for write operations.

async fn mcp_http_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ctx = match extract_ctx(&state, &headers) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
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
            let result = tokio::task::spawn_blocking(move || {
                mcp_tools_call_blocking(&backend, &ctx, &session_id, params)
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
        "get_decision" => mcp_get_decision(backend, ctx, args),
        "get_relevant_decisions" => mcp_get_relevant_decisions(backend, ctx, args),
        "get_supersession_chain" => mcp_get_supersession_chain(backend, ctx, args),
        "search_decisions" => mcp_search_decisions(backend, ctx, args),
        "dump_graph" => mcp_dump_graph(backend, ctx),
        "hivemind_compact_view" => mcp_compact_view(backend, ctx, args),
        "summarize_decisions" => mcp_summarize(backend, ctx, args),
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
) -> std::result::Result<MemoryGraph, (i32, String)> {
    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &ctx.tenant_id, &graph)
        .map_err(|e| (-32603i32, e.to_string()))?;
    Ok(graph)
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
            .unwrap_or_else(|| format!("Option generated from MCP value '{label}'"));
        let oid = commands
            .record_option(actor_id, &label, &description)
            .map_err(|e| (-32603i32, e.to_string()))?;
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
        .propose_decision(
            actor_id,
            &title,
            &rationale,
            &topic_keys,
            &option_ids,
            chosen_option_id.as_deref(),
            &hypothesis_ids,
            &evidence_ids,
        )
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
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &ctx.tenant_id, &graph)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let status =
        derive_decision_status(&graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
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
        .supersede(
            actor_id,
            &old_id,
            &title,
            &rationale,
            &topic_keys,
            &option_labels,
            chosen_label.as_deref(),
            &hypothesis_ids,
            &evidence_ids,
        )
        .map_err(|e| (-32603i32, e.to_string()))?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &ctx.tenant_id, &graph)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let old_status =
        derive_decision_status(&graph, &old_id).map_err(|e| (-32603i32, e.to_string()))?;
    let new_status = derive_decision_status(&graph, &outcome.new_decision_id)
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
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx)?;
    let response = get_decision(&graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_get_relevant_decisions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
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
    let graph = mcp_open_graph(backend, ctx)?;
    let response = get_relevant_decisions(&graph, &topic, status_filter)
        .map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_get_supersession_chain(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx)?;
    let response =
        get_supersession_chain(&graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_search_decisions(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    // FTS is SQLite-only; Postgres returns a clear error.
    #[cfg(feature = "shared-backend-postgres")]
    if matches!(backend, ApiBackend::Postgres(_)) {
        return Err((
            -32602,
            "full-text search is not available in shared-backend mode; \
             use get_relevant_decisions instead"
                .into(),
        ));
    }

    let ledger = backend
        .open_ledger_for_tenant(&ctx.tenant_id)
        .map_err(|e| (-32603i32, e.to_string()))?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &ctx.tenant_id, &graph)
        .map_err(|e| (-32603i32, e.to_string()))?;
    // The cfg guard above guarantees Sqlite when Postgres feature is enabled;
    // without the feature there is only one variant (Sqlite), hence the allow.
    #[allow(clippy::infallible_destructuring_match)]
    let sqlite_ledger = match &ledger {
        ApiLedger::Sqlite(l) => l,
        #[cfg(feature = "shared-backend-postgres")]
        ApiLedger::Postgres(_) => {
            return Err((
                -32602,
                "full-text search not available in shared-backend mode".into(),
            ))
        }
    };

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
    let response = search_decisions_fts_with_context(&query_ctx, sqlite_ledger, &graph, &request)
        .map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_dump_graph(backend: &ApiBackend, ctx: &ApiRequestCtx) -> McpToolResult {
    let graph = mcp_open_graph(backend, ctx)?;
    let dot = crate::cli::render_decision_dot(&graph).map_err(|e| (-32603i32, e.to_string()))?;
    Ok(serde_json::json!({ "format": "dot", "content": dot }))
}

fn mcp_compact_view(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
) -> McpToolResult {
    let decision_id = mcp_req_str(&args, "decision_id")?;
    let graph = mcp_open_graph(backend, ctx)?;
    let response =
        get_compact_view(&graph, &decision_id).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(&response).map_err(|e| (-32603i32, e.to_string()))
}

fn mcp_summarize(
    backend: &ApiBackend,
    ctx: &ApiRequestCtx,
    args: serde_json::Map<String, serde_json::Value>,
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
    let graph = mcp_open_graph(backend, ctx)?;
    let request = SummarizeRequest { decision_ids, mode };
    let response = summarize_decisions(&graph, &request).map_err(|e| (-32603i32, e.to_string()))?;
    serde_json::to_value(query_envelope(response)).map_err(|e| (-32603i32, e.to_string()))
}

// ---------------------------------------------------------------------------
// Arg-parsing helpers for MCP tool arguments (JSON → typed values)
// ---------------------------------------------------------------------------

fn mcp_req_str(
    args: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> std::result::Result<String, (i32, String)> {
    match args.get(field) {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(serde_json::Value::String(_)) => {
            Err((-32602, format!("`{field}` must be a non-empty string")))
        }
        Some(_) => Err((-32602, format!("`{field}` must be a string"))),
        None => Err((-32602, format!("missing `{field}`"))),
    }
}

fn mcp_opt_str(
    args: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> std::result::Result<Option<String>, (i32, String)> {
    match args.get(field) {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(serde_json::Value::String(_)) | None | Some(serde_json::Value::Null) => Ok(None),
        Some(_) => Err((-32602, format!("`{field}` must be a string"))),
    }
}

fn mcp_req_str_array(
    args: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> std::result::Result<Vec<String>, (i32, String)> {
    match args.get(field) {
        Some(serde_json::Value::Array(items)) => mcp_collect_strings(items, field),
        Some(_) => Err((-32602, format!("`{field}` must be an array of strings"))),
        None => Err((-32602, format!("missing `{field}`"))),
    }
}

fn mcp_opt_str_array(
    args: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> std::result::Result<Vec<String>, (i32, String)> {
    match args.get(field) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => mcp_collect_strings(items, field),
        Some(_) => Err((-32602, format!("`{field}` must be an array of strings"))),
    }
}

fn mcp_opt_option_labels(
    args: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> std::result::Result<Vec<String>, (i32, String)> {
    match args.get(field) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                serde_json::Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
                serde_json::Value::Object(map) => map
                    .get("label")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| (-32602i32, format!("`{field}[{i}].label` must be non-empty"))),
                _ => Err((
                    -32602i32,
                    format!("`{field}[{i}]` must be a string or object with `label`"),
                )),
            })
            .collect(),
        Some(_) => Err((-32602, format!("`{field}` must be an array"))),
    }
}

fn mcp_collect_strings(
    items: &[serde_json::Value],
    field: &str,
) -> std::result::Result<Vec<String>, (i32, String)> {
    items
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            serde_json::Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
            _ => Err((
                -32602i32,
                format!("`{field}[{i}]` must be a non-empty string"),
            )),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// OAuth resource/authorization-server metadata stubs
// OAuth resource/authorization server metadata (MCP auth spec, Nov-2025)
// ---------------------------------------------------------------------------
//
// WorkOS is the Authorization Server. We serve only the Protected Resource
// metadata (RFC 9728) that tells MCP clients where to find the AS.
// WorkOS serves its own AS metadata at:
//   https://api.workos.com/.well-known/openid-configuration
// ---------------------------------------------------------------------------

async fn oauth_protected_resource_handler(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(ref cfg) = state.workos_config {
        // Point MCP clients at WorkOS as the Authorization Server.
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "resource": cfg.domain,
                "authorization_servers": [cfg.domain],
                "bearer_methods_supported": ["header"],
                "scopes_supported": ["openid", "profile", "email"],
            })),
        );
    }
    // No WorkOS configured — return a generic stub.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "resource": "/",
            "authorization_servers": [],
            "bearer_methods_supported": ["header"],
            "scopes_supported": [],
        })),
    )
}

/// WorkOS is the Authorization Server; redirect discovery to them.
async fn oauth_authorization_server_handler(State(state): State<AppState>) -> Response {
    if let Some(ref cfg) = state.workos_config {
        // Return WorkOS OIDC discovery document location to help debug flows,
        // but the canonical AS metadata lives at WorkOS, not us.
        let discovery_url = format!("{}/.well-known/openid-configuration", cfg.domain);
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "note": "WorkOS is the authorization server for this deployment.",
                "authorization_server_discovery": discovery_url,
            })),
        )
            .into_response();
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "no authorization server configured"})),
    )
        .into_response()
}
