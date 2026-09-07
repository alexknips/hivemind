//! Bearer-token / WorkOS-JWT auth (`extract_ctx`), per-user token
//! management, tenant provisioning, and OAuth discovery metadata.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::events::TenantId;
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::ResolvedToken;

use super::{
    respond, ApiConfig, ApiError, ApiRequestCtx, ApiResult, AppState, DEFAULT_ACTOR, HEADER_ACTOR,
    HEADER_AUTHORIZATION, HEADER_FORWARDED_PROTO, HEADER_TENANT,
};

/// WorkOS OAuth resource-server configuration (populated from env vars at startup).
#[derive(Clone)]
pub(super) struct WorkosConfig {
    /// AuthKit domain, e.g. `https://your-tenant.authkit.app`.
    domain: String,
    /// OIDC issuer for JWT `iss` claim validation.
    issuer: String,
    /// Accepted JWT `aud` values. `None` disables audience validation (DCR mode).
    audience: Option<Vec<String>>,
    /// JWKS endpoint — stored for refresh-on-unknown-kid.
    jwks_url: Option<String>,
}

/// Claims extracted from a validated WorkOS access token.
#[derive(Debug, serde::Deserialize)]
struct WorkosClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    /// WorkOS organization ID — used as tenant key in SQLite mode.
    #[serde(default)]
    org_id: Option<String>,
}

/// Pre-computed JWKS state: one DecodingKey per kid, built from the raw JwkSet
/// at startup and rebuilt on refresh-on-unknown-kid.
pub(super) struct CachedJwks {
    keys: HashMap<String, DecodingKey>,
}

impl CachedJwks {
    fn build(jwks: &JwkSet) -> Self {
        let keys = jwks
            .keys
            .iter()
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone().unwrap_or_default();
                DecodingKey::from_jwk(jwk).ok().map(|k| (kid, k))
            })
            .collect();
        CachedJwks { keys }
    }
}

/// Build WorkOS resource-server config and pre-fetch JWKS.
/// Called before the tokio runtime starts, so blocking I/O is safe.
pub(super) fn build_workos_state(config: &ApiConfig) -> (Option<WorkosConfig>, CachedJwks) {
    let (Some(domain), Some(issuer), Some(jwks_url)) = (
        config.workos_domain.as_deref(),
        config
            .workos_issuer
            .as_deref()
            .or(config.workos_domain.as_deref()),
        config.workos_jwks_url.as_deref(),
    ) else {
        // WORKOS_DOMAIN/WORKOS_JWKS_URL not set — WorkOS JWT auth disabled.
        return (
            config.workos_domain.as_deref().map(|domain| WorkosConfig {
                domain: domain.to_owned(),
                issuer: config
                    .workos_issuer
                    .clone()
                    .unwrap_or_else(|| domain.to_owned()),
                audience: config.workos_audience.clone(),
                jwks_url: config.workos_jwks_url.clone(),
            }),
            CachedJwks::build(&JwkSet { keys: vec![] }),
        );
    };

    let workos_cfg = WorkosConfig {
        domain: domain.to_owned(),
        issuer: issuer.to_owned(),
        audience: config.workos_audience.clone(),
        jwks_url: Some(jwks_url.to_owned()),
    };

    match reqwest::blocking::get(jwks_url).and_then(|r| r.json::<JwkSet>()) {
        Ok(jwks) => {
            tracing::info!(target: "hivemind::api", keys = jwks.keys.len(), "WorkOS JWKS loaded");
            (Some(workos_cfg), CachedJwks::build(&jwks))
        }
        Err(e) => {
            tracing::warn!(target: "hivemind::api", error = %e, "WorkOS JWKS fetch failed — JWT auth disabled");
            (
                Some(workos_cfg),
                CachedJwks::build(&JwkSet { keys: vec![] }),
            )
        }
    }
}

pub(super) async fn extract_ctx(state: &AppState, headers: &HeaderMap) -> ApiResult<ApiRequestCtx> {
    let bearer = headers
        .get(HEADER_AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    // WorkOS JWT path: bearer that looks like a JWT ("ey...") + WorkOS configured.
    // Try this first so WorkOS tokens don't fall through to opaque-token resolution.
    #[cfg(feature = "shared-backend-postgres")]
    if bearer.starts_with("ey") {
        if let (Some(ref workos), Some(ref store)) = (&state.workos_config, &state.tenant_store) {
            let claims = workos_validate_token(state, workos, bearer).await?;

            // resolve_or_create_oidc_user calls r2d2 pool.get() which blocks.
            // Must run on a blocking thread — calling it directly on the async
            // executor causes "Cannot start a runtime from within a runtime" panic.
            let store = Arc::clone(store);
            let sub = claims.sub.clone();
            let email = claims.email.as_deref().unwrap_or(&claims.sub).to_owned();
            let tenant_id_str = tokio::task::spawn_blocking(move || {
                store.resolve_or_create_oidc_user(&sub, &email)
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
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
        // Postgres without WorkOS: per-user or legacy tenant opaque token.
        // actor_id comes from the token record — never from the caller header.
        if bearer.is_empty() {
            return Err(ApiError::unauthorized("bearer token required"));
        }
        let store = Arc::clone(store);
        let bearer_owned = bearer.to_owned();
        let resolved: ResolvedToken =
            tokio::task::spawn_blocking(move || store.resolve_token(&bearer_owned))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .map_err(|e| ApiError::internal(e.to_string()))?
                .ok_or_else(|| ApiError::unauthorized("invalid or missing bearer token"))?;
        let tenant_id = TenantId::new(&resolved.tenant_id)
            .map_err(|_| ApiError::internal("invalid tenant_id from token store"))?;
        return Ok(ApiRequestCtx {
            tenant_id,
            actor_id: resolved.actor_id,
        });
    }

    // SQLite WorkOS JWT path: WorkOS configured, no postgres store, bearer looks like JWT.
    // org_id claim → tenant key; falls back to sub when org_id absent.
    if bearer.starts_with("ey") {
        if let Some(ref workos) = state.workos_config {
            #[cfg(not(feature = "shared-backend-postgres"))]
            {
                let claims = workos_validate_token(state, workos, bearer).await?;

                let tenant_raw = claims.org_id.as_deref().unwrap_or(claims.sub.as_str());
                let tenant_id = TenantId::new(tenant_raw)
                    .map_err(|_| ApiError::internal("invalid tenant from WorkOS org_id"))?;
                let actor_id = format!("human:{}", claims.email.as_deref().unwrap_or(&claims.sub));
                return Ok(ApiRequestCtx {
                    tenant_id,
                    actor_id,
                });
            }
        }
    }

    // SQLite mode: try per-user token resolution for hm_tk_ prefixed tokens.
    // If the user store is initialized and the token is not found/revoked, reject immediately.
    if bearer.starts_with(crate::ledger::SQLITE_TOKEN_PREFIX) {
        if let Some(ref user_store) = state.sqlite_user_store {
            let bearer_owned = bearer.to_owned();
            let user_store = Arc::clone(user_store);
            let resolved =
                tokio::task::spawn_blocking(move || user_store.resolve_token(&bearer_owned))
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?
                    .map_err(|e| ApiError::internal(e.to_string()))?;
            return match resolved {
                Some(r) => {
                    let tenant_id = TenantId::new(&r.tenant_id)
                        .map_err(|_| ApiError::internal("invalid tenant_id from user token"))?;
                    Ok(ApiRequestCtx {
                        tenant_id,
                        actor_id: r.actor_id,
                    })
                }
                None => Err(ApiError::unauthorized("invalid or revoked bearer token")),
            };
        }
    }

    // SQLite dev mode: single static key comparison.
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

/// Check the JWKS cache for the kid in `bearer`, refresh from `workos.jwks_url`
/// on cache miss (key rotation), then validate the JWT against the current keys.
async fn workos_validate_token(
    state: &AppState,
    workos: &WorkosConfig,
    bearer: &str,
) -> ApiResult<WorkosClaims> {
    let kid = decode_header(bearer)
        .ok()
        .and_then(|h| h.kid)
        .unwrap_or_default();

    let need_refresh = {
        let guard = state.workos_jwks.read().unwrap(); // ubs:ignore: RwLock poisoned iff a prior thread panicked — propagate the panic
        guard.keys.is_empty() || (!kid.is_empty() && !guard.keys.contains_key(&kid))
    };

    if need_refresh {
        if let Some(ref url) = workos.jwks_url {
            let url = url.clone();
            let result =
                tokio::task::spawn_blocking(move || reqwest::blocking::get(&url)?.json::<JwkSet>())
                    .await;
            match result {
                Ok(Ok(jwks)) => {
                    tracing::info!(target: "hivemind::api", keys = jwks.keys.len(), "WorkOS JWKS refreshed on unknown kid");
                    let mut w = state.workos_jwks.write().unwrap(); // ubs:ignore: RwLock poisoned iff a prior thread panicked — propagate the panic
                    *w = CachedJwks::build(&jwks);
                }
                Ok(Err(e)) => {
                    tracing::warn!(target: "hivemind::api", error = %e, "WorkOS JWKS refresh failed");
                }
                Err(e) => {
                    tracing::warn!(target: "hivemind::api", error = %e, "WorkOS JWKS refresh task panicked");
                }
            }
        }
    }

    let snapshot = {
        let guard = state.workos_jwks.read().unwrap(); // ubs:ignore: RwLock poisoned iff a prior thread panicked — propagate the panic
        if guard.keys.is_empty() {
            return Err(ApiError::unauthorized(
                "WorkOS JWKS not loaded — cannot validate JWT",
            ));
        }
        guard.keys.clone()
    };

    validate_workos_jwt(
        bearer,
        &snapshot,
        &workos.issuer,
        workos.audience.as_deref(),
    )
    .map_err(ApiError::unauthorized)
}

/// Validate a WorkOS JWT access token against the pre-computed key map.
///
/// Validates signature (RS256/ES256), issuer, and expiry. When `audience` is
/// `Some`, the JWT `aud` claim must match at least one value — set
/// `WORKOS_AUDIENCE` to a comma-separated list of client_ids (SPA public app
/// + MCP confidential app) for strict enforcement. Omit for DCR environments.
fn validate_workos_jwt(
    token: &str,
    keys: &HashMap<String, DecodingKey>,
    issuer: &str,
    audience: Option<&[String]>,
) -> Result<WorkosClaims, String> {
    let header = decode_header(token).map_err(|_| "invalid token")?;

    // Look up the pre-computed DecodingKey by kid; fall back to any key when kid absent.
    let kid = header.kid.as_deref().unwrap_or("");
    let decoding_key = if kid.is_empty() {
        keys.values().next()
    } else {
        keys.get(kid)
    }
    .ok_or("no matching signing key")?;

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
        Some(auds) => validation.set_audience(auds),
        // ubs:ignore: DCR clients have dynamic client_ids; set WORKOS_AUDIENCE for strict aud enforcement
        None => validation.validate_aud = false,
    }

    // ubs:ignore: sig+iss+exp always validated; aud validated when WORKOS_AUDIENCE set
    decode::<WorkosClaims>(token, decoding_key, &validation)
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
// Tenant provisioning (Postgres, admin-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Deserialize)]
pub(super) struct ProvisionTenantRequest {
    tenant_id: String,
    display_name: String,
}

/// Provision a new tenant and issue its initial bearer token.
/// Requires the `Authorization: Bearer <HIVEMIND_ADMIN_KEY>` header.
/// Only available when the `shared-backend-postgres` feature is enabled.
#[cfg(feature = "shared-backend-postgres")]
pub(super) async fn provision_tenant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<ProvisionTenantRequest>, JsonRejection>,
) -> Response {
    // Admin key gate (separate from per-tenant bearer tokens).
    let provided = headers
        .get(HEADER_AUTHORIZATION)
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

    respond(result, StatusCode::CREATED)
}

// ---------------------------------------------------------------------------
// User management endpoints (SQLite + Postgres, admin-gated)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct CreateUserRequest {
    email: String,
    display_name: String,
    #[serde(default = "default_role")]
    role: String,
    tenant_id: Option<String>,
}

fn default_role() -> String {
    "member".to_owned()
}

#[allow(clippy::result_large_err)]
fn check_admin_key(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let provided = headers
        .get(HEADER_AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    match &state.admin_key {
        None => Err(ApiError::internal("HIVEMIND_ADMIN_KEY not configured").into_response()),
        Some(expected) => {
            if constant_time_eq(provided, expected) {
                Ok(())
            } else {
                Err(ApiError::unauthorized("invalid admin key").into_response())
            }
        }
    }
}

pub(super) async fn create_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CreateUserRequest>, JsonRejection>,
) -> Response {
    if let Err(r) = check_admin_key(&state, &headers) {
        return r;
    }
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    // Postgres path
    #[cfg(feature = "shared-backend-postgres")]
    if let Some(ref store) = state.tenant_store {
        let tenant_id = req.tenant_id.clone().unwrap_or_else(|| "local".to_owned());
        let store = Arc::clone(store);
        let result = tokio::task::spawn_blocking(move || {
            let u = store
                .create_user(&tenant_id, &req.email, &req.display_name, &req.role)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok::<_, ApiError>(serde_json::json!({
                "user_id": u.user_id,
                "email": u.email,
                "display_name": u.display_name,
                "role": u.role,
                "token_id": u.token_id,
                "token_secret": u.token_secret,
            }))
        })
        .await;
        return respond(result, StatusCode::CREATED);
    }

    // SQLite path
    if let Some(ref user_store) = state.sqlite_user_store {
        let tenant_id = req
            .tenant_id
            .clone()
            .unwrap_or_else(|| crate::events::TenantId::LOCAL_VALUE.to_owned());
        let user_store = Arc::clone(user_store);
        let result = tokio::task::spawn_blocking(move || {
            let u = user_store
                .create_user(&tenant_id, &req.email, &req.display_name, &req.role)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok::<_, ApiError>(serde_json::json!({
                "user_id": u.user_id,
                "email": u.email,
                "display_name": u.display_name,
                "role": u.role,
                "token_id": u.token_id,
                "token_secret": u.token_secret,
            }))
        })
        .await;
        return respond(result, StatusCode::CREATED);
    }

    ApiError::internal("user store not initialized").into_response()
}

pub(super) async fn list_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Err(r) = check_admin_key(&state, &headers) {
        return r;
    }
    let tenant_id = params
        .get("tenant_id")
        .map(|s| s.as_str())
        .unwrap_or("local")
        .to_owned();

    #[cfg(feature = "shared-backend-postgres")]
    if let Some(ref store) = state.tenant_store {
        let store = Arc::clone(store);
        let result = tokio::task::spawn_blocking(move || {
            store
                .list_users(&tenant_id)
                .map_err(|e| ApiError::internal(e.to_string()))
        })
        .await;
        return match result {
            Ok(Ok(users)) => {
                let body: Vec<_> = users
                    .into_iter()
                    .map(|u| {
                        serde_json::json!({
                            "user_id": u.user_id,
                            "email": u.email,
                            "display_name": u.display_name,
                            "role": u.role,
                        })
                    })
                    .collect();
                Json(serde_json::json!({ "users": body })).into_response()
            }
            Ok(Err(e)) => e.into_response(),
            Err(e) => ApiError::internal(e.to_string()).into_response(),
        };
    }

    if let Some(ref user_store) = state.sqlite_user_store {
        let user_store = Arc::clone(user_store);
        let result = tokio::task::spawn_blocking(move || {
            user_store
                .list_users(&tenant_id)
                .map_err(|e| ApiError::internal(e.to_string()))
        })
        .await;
        return match result {
            Ok(Ok(users)) => {
                let body: Vec<_> = users
                    .into_iter()
                    .map(|u| {
                        serde_json::json!({
                            "user_id": u.user_id,
                            "email": u.email,
                            "display_name": u.display_name,
                            "role": u.role,
                        })
                    })
                    .collect();
                Json(serde_json::json!({ "users": body })).into_response()
            }
            Ok(Err(e)) => e.into_response(),
            Err(e) => ApiError::internal(e.to_string()).into_response(),
        };
    }

    ApiError::internal("user store not initialized").into_response()
}

pub(super) async fn mint_user_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id_str): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Err(r) = check_admin_key(&state, &headers) {
        return r;
    }
    let user_id = match uuid::Uuid::parse_str(&user_id_str) {
        Ok(u) => u,
        Err(_) => return ApiError::validation("invalid user_id").into_response(),
    };
    let tenant_id = params
        .get("tenant_id")
        .map(|s| s.as_str())
        .unwrap_or("local")
        .to_owned();
    let label = params.get("label").cloned();

    #[cfg(feature = "shared-backend-postgres")]
    if let Some(ref store) = state.tenant_store {
        let store = Arc::clone(store);
        let label2 = label.clone();
        let result = tokio::task::spawn_blocking(move || {
            let u = store
                .mint_user_token(&tenant_id, user_id, label2.as_deref())
                .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok::<_, ApiError>(serde_json::json!({
                "user_id": u.user_id,
                "token_id": u.token_id,
                "token_secret": u.token_secret,
            }))
        })
        .await;
        return respond(result, StatusCode::CREATED);
    }

    if let Some(ref user_store) = state.sqlite_user_store {
        let user_store = Arc::clone(user_store);
        let result = tokio::task::spawn_blocking(move || {
            user_store
                .mint_user_token(&tenant_id, user_id, label.as_deref())
                .map_err(|e| ApiError::internal(e.to_string()))
        })
        .await;
        return match result {
            Ok(Ok((token_id, token_secret))) => (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "user_id": user_id,
                    "token_id": token_id,
                    "token_secret": token_secret,
                })),
            )
                .into_response(),
            Ok(Err(e)) => e.into_response(),
            Err(e) => ApiError::internal(e.to_string()).into_response(),
        };
    }

    ApiError::internal("user store not initialized").into_response()
}

pub(super) async fn revoke_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((user_id_str, token_id_str)): Path<(String, String)>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Err(r) = check_admin_key(&state, &headers) {
        return r;
    }
    let token_id = match uuid::Uuid::parse_str(&token_id_str) {
        Ok(u) => u,
        Err(_) => return ApiError::validation("invalid token_id").into_response(),
    };
    if uuid::Uuid::parse_str(&user_id_str).is_err() {
        return ApiError::validation("invalid user_id").into_response();
    }
    let tenant_id = params
        .get("tenant_id")
        .map(|s| s.as_str())
        .unwrap_or("local")
        .to_owned();

    #[cfg(feature = "shared-backend-postgres")]
    if let Some(ref store) = state.tenant_store {
        let store = Arc::clone(store);
        let result = tokio::task::spawn_blocking(move || {
            store
                .revoke_token(&tenant_id, token_id)
                .map_err(|e| ApiError::internal(e.to_string()))
        })
        .await;
        return match result {
            Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
            Ok(Ok(false)) => {
                ApiError::not_found("token not found or already revoked").into_response()
            }
            Ok(Err(e)) => e.into_response(),
            Err(e) => ApiError::internal(e.to_string()).into_response(),
        };
    }

    if let Some(ref user_store) = state.sqlite_user_store {
        let user_store = Arc::clone(user_store);
        let result = tokio::task::spawn_blocking(move || {
            user_store
                .revoke_token(&tenant_id, token_id)
                .map_err(|e| ApiError::internal(e.to_string()))
        })
        .await;
        return match result {
            Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
            Ok(Ok(false)) => {
                ApiError::not_found("token not found or already revoked").into_response()
            }
            Ok(Err(e)) => e.into_response(),
            Err(e) => ApiError::internal(e.to_string()).into_response(),
        };
    }

    ApiError::internal("user store not initialized").into_response()
}

// ---------------------------------------------------------------------------
// OAuth resource/authorization server metadata (MCP auth spec, Nov-2025)
// ---------------------------------------------------------------------------
//
// WorkOS is the Authorization Server. We serve only the Protected Resource
// metadata (RFC 9728) that tells MCP clients where to find the AS.
// WorkOS serves its own AS metadata at:
//   https://api.workos.com/.well-known/openid-configuration
// ---------------------------------------------------------------------------

pub(super) async fn oauth_protected_resource_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Derive the MCP server's own URL from request headers.
    // Fly.io sets X-Forwarded-Proto; Host is standard HTTP/1.1.
    let scheme = headers
        .get(HEADER_FORWARDED_PROTO)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let resource = format!("{scheme}://{host}/mcp");

    if let Some(ref cfg) = state.workos_config {
        // `resource` = this MCP server's URL (RFC 9728 §2: the protected resource).
        // `authorization_servers` = WorkOS (the AS that issues tokens for this resource).
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "resource": resource,
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
            "resource": resource,
            "authorization_servers": [],
            "bearer_methods_supported": ["header"],
            "scopes_supported": [],
        })),
    )
}

/// WorkOS is the Authorization Server; redirect discovery to them.
pub(super) async fn oauth_authorization_server_handler(State(state): State<AppState>) -> Response {
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
