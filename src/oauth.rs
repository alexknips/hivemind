//! Custom all-Rust OAuth 2.1 Authorization Server for HiveMind MCP access.
//!
//! Implements:
//! - RFC7591 Dynamic Client Registration (DCR) at POST /oauth/register
//! - OAuth 2.1 authorization-code + PKCE at GET /oauth/authorize and GET /oauth/callback
//! - Token endpoint at POST /oauth/token
//! - Token resolution integrated into the /mcp bearer-auth path
//!
//! Only active when the `shared-backend-postgres` feature is enabled and
//! `HIVEMIND_DATABASE_URL` is set. All endpoints return 501 in SQLite dev mode.
//!
//! Providers supported: GitHub (`provider=github`) and Google (`provider=google`).
//! Headless callers keep using `HIVEMIND_API_KEY` bearer unchanged.

use std::str::FromStr;

use base64ct::{Base64UrlUnpadded, Encoding};
use chrono::{DateTime, Utc};
use postgres::{types::ToSql, Config};
use postgres_native_tls::MakeTlsConnector;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::LedgerError;
use crate::Result;

fn storage_error<E: std::fmt::Display>(error: E) -> LedgerError {
    LedgerError::Storage(error.to_string())
}

type PgPool = Pool<PostgresConnectionManager<MakeTlsConnector>>;

/// Prefix for OAuth-issued bearer tokens. Distinct from `hm_tk_` (TenantStore tokens).
pub const OAUTH_TOKEN_PREFIX: &str = "hm_oa_";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Provider-credential and server-base-URL config for the OAuth AS.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    /// Public base URL of this server (e.g. "https://hivemind.fly.dev").
    pub base_url: String,
}

impl OAuthConfig {
    pub fn from_env(base_url: String) -> Self {
        Self {
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok(),
            google_client_id: std::env::var("GOOGLE_CLIENT_ID").ok(),
            google_client_secret: std::env::var("GOOGLE_CLIENT_SECRET").ok(),
            base_url,
        }
    }

    pub fn has_any_provider(&self) -> bool {
        self.github_client_id.is_some() || self.google_client_id.is_some()
    }

    pub fn callback_url(&self) -> String {
        format!("{}/oauth/callback", self.base_url)
    }

    pub fn github_authorize_url(&self, provider_state: &str) -> Option<String> {
        let client_id = self.github_client_id.as_deref()?;
        Some(format!(
            "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=user%3Aemail&state={}",
            urlencoded(client_id),
            urlencoded(&self.callback_url()),
            urlencoded(provider_state),
        ))
    }

    pub fn google_authorize_url(&self, provider_state: &str) -> Option<String> {
        let client_id = self.google_client_id.as_deref()?;
        Some(format!(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=openid%20email&state={}",
            urlencoded(client_id),
            urlencoded(&self.callback_url()),
            urlencoded(provider_state),
        ))
    }
}

// ---------------------------------------------------------------------------
// Session / code / token value types
// ---------------------------------------------------------------------------

pub struct OAuthSession {
    pub session_id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub client_state: String,
    pub provider: String,
    pub expires_at: DateTime<Utc>,
}

pub struct ExchangeResult {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub tenant_id: String,
}

// ---------------------------------------------------------------------------
// OAuthStore
// ---------------------------------------------------------------------------

/// Postgres-backed store for all OAuth AS state: clients, sessions, codes, tokens.
#[derive(Clone)]
pub struct OAuthStore {
    pool: PgPool,
}

impl OAuthStore {
    pub fn connect(database_url: &str) -> Result<Self> {
        let config = Config::from_str(database_url).map_err(storage_error)?;
        let tls = MakeTlsConnector::new(native_tls::TlsConnector::new().map_err(storage_error)?);
        let manager = PostgresConnectionManager::new(config, tls);
        let pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .map_err(storage_error)?;
        let store = Self { pool };
        store.initialize_schema()?;
        Ok(store)
    }

    fn initialize_schema(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(storage_error)?;
        client
            .batch_execute(
                // redirect_uris stored as a JSON text string to avoid postgres array types.
                "CREATE TABLE IF NOT EXISTS hm_oauth_clients (
                    client_id     text PRIMARY KEY,
                    redirect_uris text NOT NULL DEFAULT '[]',
                    scope         text NOT NULL DEFAULT 'read',
                    created_at    timestamptz NOT NULL DEFAULT now()
                );
                CREATE TABLE IF NOT EXISTS hm_oauth_sessions (
                    session_id            text PRIMARY KEY,
                    client_id             text NOT NULL,
                    redirect_uri          text NOT NULL,
                    code_challenge        text NOT NULL,
                    code_challenge_method text NOT NULL DEFAULT 'S256',
                    client_state          text NOT NULL,
                    provider              text NOT NULL,
                    provider_state        text NOT NULL UNIQUE,
                    expires_at            timestamptz NOT NULL,
                    created_at            timestamptz NOT NULL DEFAULT now()
                );
                CREATE TABLE IF NOT EXISTS hm_oauth_codes (
                    code        text PRIMARY KEY,
                    session_id  text NOT NULL,
                    user_sub    text NOT NULL,
                    tenant_id   text NOT NULL,
                    expires_at  timestamptz NOT NULL,
                    created_at  timestamptz NOT NULL DEFAULT now()
                );
                CREATE TABLE IF NOT EXISTS hm_oauth_tokens (
                    token       text PRIMARY KEY,
                    client_id   text NOT NULL,
                    user_sub    text NOT NULL,
                    tenant_id   text NOT NULL,
                    scope       text NOT NULL DEFAULT 'read',
                    expires_at  timestamptz,
                    created_at  timestamptz NOT NULL DEFAULT now()
                );
                CREATE TABLE IF NOT EXISTS hm_oauth_user_tenants (
                    user_sub    text PRIMARY KEY,
                    tenant_id   text NOT NULL,
                    email       text NOT NULL DEFAULT '',
                    created_at  timestamptz NOT NULL DEFAULT now()
                );",
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// DCR: register a new client. Returns the `client_id`.
    pub fn register_client(&self, redirect_uris: &[String], scope: &str) -> Result<String> {
        let client_id = format!("hm_cl_{}", Uuid::new_v4().simple());
        let uris_json = serde_json::to_string(redirect_uris).unwrap_or_else(|_| "[]".to_owned());
        let mut pg = self.pool.get().map_err(storage_error)?;
        let params: &[&(dyn ToSql + Sync)] = &[&client_id, &uris_json, &scope];
        pg.execute(
            "INSERT INTO hm_oauth_clients (client_id, redirect_uris, scope) VALUES ($1, $2, $3)",
            params,
        )
        .map_err(storage_error)?;
        Ok(client_id)
    }

    /// Start an authorization session. Returns `(session_id, provider_state)`.
    pub fn start_session(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code_challenge: &str,
        code_challenge_method: &str,
        client_state: &str,
        provider: &str,
    ) -> Result<(String, String)> {
        let session_id = Uuid::new_v4().to_string();
        let provider_state = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::try_minutes(10).unwrap_or_default();

        let mut pg = self.pool.get().map_err(storage_error)?;
        let params: &[&(dyn ToSql + Sync)] = &[
            &session_id,
            &client_id,
            &redirect_uri,
            &code_challenge,
            &code_challenge_method,
            &client_state,
            &provider,
            &provider_state,
            &expires_at,
        ];
        pg.execute(
            "INSERT INTO hm_oauth_sessions
                (session_id, client_id, redirect_uri, code_challenge, code_challenge_method,
                 client_state, provider, provider_state, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            params,
        )
        .map_err(storage_error)?;

        Ok((session_id, provider_state))
    }

    /// Fetch a session by the state value sent to the upstream provider.
    pub fn get_session_by_provider_state(
        &self,
        provider_state: &str,
    ) -> Result<Option<OAuthSession>> {
        let mut pg = self.pool.get().map_err(storage_error)?;
        let row = pg
            .query_opt(
                "SELECT session_id, client_id, redirect_uri, code_challenge,
                        client_state, provider, expires_at
                 FROM hm_oauth_sessions
                 WHERE provider_state = $1 AND expires_at > now()",
                &[&provider_state],
            )
            .map_err(storage_error)?;

        Ok(row.map(|r| OAuthSession {
            session_id: r.get(0),
            client_id: r.get(1),
            redirect_uri: r.get(2),
            code_challenge: r.get(3),
            client_state: r.get(4),
            provider: r.get(5),
            expires_at: r.get(6),
        }))
    }

    /// Map a provider identity to a HiveMind tenant, provisioning on first login.
    ///
    /// Uses `user_sub` as the stable key. Inserts into `hm_oauth_user_tenants`
    /// and (on first login) also into `hm_tenants` with `display_name = email`.
    pub fn ensure_tenant(&self, user_sub: &str, email: &str) -> Result<String> {
        let mut pg = self.pool.get().map_err(storage_error)?;

        if let Some(row) = pg
            .query_opt(
                "SELECT tenant_id FROM hm_oauth_user_tenants WHERE user_sub = $1",
                &[&user_sub],
            )
            .map_err(storage_error)?
        {
            return Ok(row.get(0));
        }

        // First login — provision a new tenant.
        let tenant_id = format!("oauth:{user_sub}");
        let display_name = if email.is_empty() {
            user_sub.to_owned()
        } else {
            email.to_owned()
        };

        let mut tx = pg.transaction().map_err(storage_error)?;
        {
            let params: &[&(dyn ToSql + Sync)] = &[&tenant_id, &display_name];
            tx.execute(
                "INSERT INTO hm_tenants (tenant_id, display_name)
                 VALUES ($1, $2)
                 ON CONFLICT (tenant_id) DO NOTHING",
                params,
            )
            .map_err(storage_error)?;
        }
        {
            let params: &[&(dyn ToSql + Sync)] = &[&user_sub, &tenant_id, &email];
            tx.execute(
                "INSERT INTO hm_oauth_user_tenants (user_sub, tenant_id, email)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (user_sub) DO NOTHING",
                params,
            )
            .map_err(storage_error)?;
        }
        tx.commit().map_err(storage_error)?;

        Ok(tenant_id)
    }

    /// Issue an authorization code after the provider callback succeeds.
    pub fn issue_code(&self, session_id: &str, user_sub: &str, tenant_id: &str) -> Result<String> {
        let code = format!("hm_co_{}", hex_encode(&random_bytes_32()));
        let expires_at = Utc::now() + chrono::Duration::try_minutes(5).unwrap_or_default();

        let mut pg = self.pool.get().map_err(storage_error)?;
        let params: &[&(dyn ToSql + Sync)] =
            &[&code, &session_id, &user_sub, &tenant_id, &expires_at];
        pg.execute(
            "INSERT INTO hm_oauth_codes (code, session_id, user_sub, tenant_id, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
            params,
        )
        .map_err(storage_error)?;

        Ok(code)
    }

    /// Exchange an authorization code for a bearer token, validating PKCE S256.
    ///
    /// Returns `None` when the code is expired, not found, the client_id mismatches,
    /// or PKCE verification fails.
    pub fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        client_id: &str,
    ) -> Result<Option<ExchangeResult>> {
        let mut pg = self.pool.get().map_err(storage_error)?;

        let row = pg
            .query_opt(
                "SELECT c.user_sub, c.tenant_id, s.code_challenge, s.client_id
                 FROM hm_oauth_codes c
                 JOIN hm_oauth_sessions s ON s.session_id = c.session_id
                 WHERE c.code = $1 AND c.expires_at > now()",
                &[&code],
            )
            .map_err(storage_error)?;

        let row = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let user_sub: String = row.get(0);
        let tenant_id: String = row.get(1);
        let code_challenge: String = row.get(2);
        let session_client_id: String = row.get(3);

        if !constant_time_eq(&session_client_id, client_id) {
            return Ok(None);
        }
        if !pkce_verify(code_verifier, &code_challenge) {
            return Ok(None);
        }

        let token = format!("{OAUTH_TOKEN_PREFIX}{}", hex_encode(&random_bytes_32()));
        {
            let params: &[&(dyn ToSql + Sync)] = &[&token, &client_id, &user_sub, &tenant_id];
            pg.execute(
                "INSERT INTO hm_oauth_tokens (token, client_id, user_sub, tenant_id)
                 VALUES ($1, $2, $3, $4)",
                params,
            )
            .map_err(storage_error)?;
        }

        // Authorization codes are single-use.
        pg.execute("DELETE FROM hm_oauth_codes WHERE code = $1", &[&code])
            .map_err(storage_error)?;

        Ok(Some(ExchangeResult {
            access_token: token,
            token_type: "bearer".to_owned(),
            scope: "read".to_owned(),
            tenant_id,
        }))
    }

    /// Resolve an OAuth-issued bearer token to a `tenant_id`. Returns `None` when
    /// the token is not found, expired, or does not have the `hm_oa_` prefix.
    pub fn resolve_token(&self, bearer: &str) -> Result<Option<String>> {
        if !bearer.starts_with(OAUTH_TOKEN_PREFIX) {
            return Ok(None);
        }
        let mut pg = self.pool.get().map_err(storage_error)?;
        let row = pg
            .query_opt(
                "SELECT tenant_id FROM hm_oauth_tokens
                 WHERE token = $1 AND (expires_at IS NULL OR expires_at > now())",
                &[&bearer],
            )
            .map_err(storage_error)?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    }
}

// ---------------------------------------------------------------------------
// PKCE helpers (RFC 7636, S256)
// ---------------------------------------------------------------------------

/// Generate a cryptographically random PKCE verifier (43-char base64url, no padding).
pub fn generate_pkce_verifier() -> String {
    Base64UrlUnpadded::encode_string(&random_bytes_32())
}

/// Compute the S256 code challenge from a verifier: BASE64URL(SHA256(verifier)).
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    Base64UrlUnpadded::encode_string(&digest)
}

/// Constant-time PKCE verification.
fn pkce_verify(verifier: &str, challenge: &str) -> bool {
    let computed = pkce_challenge(verifier);
    if computed.len() != challenge.len() {
        return false;
    }
    computed
        .bytes()
        .zip(challenge.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

// ---------------------------------------------------------------------------
// Provider code-exchange helpers (async — called from async axum handlers)
// ---------------------------------------------------------------------------

/// Exchange a GitHub authorization code for (user_sub, primary_email).
pub async fn exchange_github_code(code: &str, config: &OAuthConfig) -> Result<(String, String)> {
    let client_id = config.github_client_id.as_deref().ok_or_else(|| {
        crate::error::LedgerError::Storage("GITHUB_CLIENT_ID not configured".to_owned())
    })?;
    let client_secret = config.github_client_secret.as_deref().ok_or_else(|| {
        crate::error::LedgerError::Storage("GITHUB_CLIENT_SECRET not configured".to_owned())
    })?;

    let http = reqwest::Client::new();
    let token_resp: serde_json::Value = http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": config.callback_url(),
        }))
        .send()
        .await
        .map_err(|e| crate::error::LedgerError::Storage(format!("GitHub token exchange: {e}")))?
        .json()
        .await
        .map_err(|e| {
            crate::error::LedgerError::Storage(format!("GitHub token response parse: {e}"))
        })?;

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| {
            crate::error::LedgerError::Storage(format!(
                "GitHub token missing in response: {token_resp}"
            ))
        })?
        .to_owned();

    let user_resp: serde_json::Value = http
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "hivemind/1.0")
        .send()
        .await
        .map_err(|e| crate::error::LedgerError::Storage(format!("GitHub user info: {e}")))?
        .json()
        .await
        .map_err(|e| {
            crate::error::LedgerError::Storage(format!("GitHub user response parse: {e}"))
        })?;

    let sub = user_resp["id"]
        .as_i64()
        .map(|id| format!("github:{id}"))
        .ok_or_else(|| crate::error::LedgerError::Storage("GitHub user id missing".to_owned()))?;

    // Email: prefer the direct field; fall back to GET /user/emails.
    let email = if let Some(email) = user_resp["email"].as_str().filter(|s| !s.is_empty()) {
        email.to_owned()
    } else {
        let emails: serde_json::Value = http
            .get("https://api.github.com/user/emails")
            .header("Authorization", format!("Bearer {access_token}"))
            .header("User-Agent", "hivemind/1.0")
            .send()
            .await
            .map_err(|e| crate::error::LedgerError::Storage(format!("GitHub emails: {e}")))?
            .json()
            .await
            .map_err(|e| crate::error::LedgerError::Storage(format!("GitHub emails parse: {e}")))?;

        emails
            .as_array()
            .and_then(|arr| arr.iter().find(|e| e["primary"].as_bool().unwrap_or(false)))
            .and_then(|e| e["email"].as_str())
            .unwrap_or("")
            .to_owned()
    };

    Ok((sub, email))
}

/// Exchange a Google authorization code for (user_sub, email).
///
/// Uses the `id_token` JWT returned by the token endpoint (no extra userinfo
/// round-trip) — the payload is base64url-encoded JSON with `sub` and `email`.
pub async fn exchange_google_code(code: &str, config: &OAuthConfig) -> Result<(String, String)> {
    let client_id = config.google_client_id.as_deref().ok_or_else(|| {
        crate::error::LedgerError::Storage("GOOGLE_CLIENT_ID not configured".to_owned())
    })?;
    let client_secret = config.google_client_secret.as_deref().ok_or_else(|| {
        crate::error::LedgerError::Storage("GOOGLE_CLIENT_SECRET not configured".to_owned())
    })?;

    let http = reqwest::Client::new();
    let token_resp: serde_json::Value = http
        .post("https://oauth2.googleapis.com/token")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "grant_type": "authorization_code",
            "redirect_uri": config.callback_url(),
        }))
        .send()
        .await
        .map_err(|e| crate::error::LedgerError::Storage(format!("Google token exchange: {e}")))?
        .json()
        .await
        .map_err(|e| {
            crate::error::LedgerError::Storage(format!("Google token response parse: {e}"))
        })?;

    let id_token = token_resp["id_token"].as_str().ok_or_else(|| {
        crate::error::LedgerError::Storage(format!("Google id_token missing: {token_resp}"))
    })?;

    // JWT: header.payload.sig — decode the payload to extract claims.
    let payload_b64 = id_token.split('.').nth(1).ok_or_else(|| {
        crate::error::LedgerError::Storage("Google id_token: invalid JWT format".to_owned())
    })?;
    let payload_bytes = Base64UrlUnpadded::decode_vec(payload_b64).map_err(|e| {
        crate::error::LedgerError::Storage(format!("Google JWT base64 decode: {e}"))
    })?;
    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| crate::error::LedgerError::Storage(format!("Google JWT parse: {e}")))?;

    let sub = claims["sub"]
        .as_str()
        .map(|s| format!("google:{s}"))
        .ok_or_else(|| crate::error::LedgerError::Storage("Google JWT sub missing".to_owned()))?;
    let email = claims["email"].as_str().unwrap_or("").to_owned();

    Ok((sub, email))
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn random_bytes_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    bytes
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub fn urlencoded(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}
