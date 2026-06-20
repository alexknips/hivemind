use std::str::FromStr;

use postgres::Config;
use postgres_native_tls::MakeTlsConnector;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::LedgerError;
use crate::Result;

use super::super::backend_error::storage_error;

type PgPool = Pool<PostgresConnectionManager<MakeTlsConnector>>;

/// Token prefix for all HiveMind bearer tokens.
/// Uses `hm_tk_` (not `sk_live_`) to avoid false-positive matches with
/// third-party secret-scanning patterns (e.g. Highnote).
const TOKEN_PREFIX: &str = "hm_tk_";

/// Result of provisioning a new tenant.
pub struct ProvisionedTenant {
    pub tenant_id: String,
    pub token_id: Uuid,
    /// Full bearer secret — `hm_tk_<64-hex>`. Returned ONCE, never stored.
    pub token_secret: String,
}

/// Provisioning and token resolution backed by Postgres.
///
/// Manages `hm_tenants` and `hm_tokens` tables, enables RLS on the event
/// and projection tables, and resolves bearer tokens to `tenant_id`.
#[derive(Clone)]
pub struct TenantStore {
    pool: PgPool,
}

impl TenantStore {
    /// Connect and initialize the provisioning schema.
    ///
    /// Must be called AFTER `PostgresEventLedger` and `PostgresGraphView`
    /// have created their tables so that RLS can be enabled on them.
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

    /// Create provisioning tables and enable RLS on the event ledger.
    pub fn initialize_schema(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(storage_error)?;

        // Create provisioning tables (all idempotent).
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS hm_tenants (
                    tenant_id    text PRIMARY KEY,
                    display_name text NOT NULL,
                    created_at   timestamptz NOT NULL DEFAULT now()
                );
                CREATE TABLE IF NOT EXISTS hm_tokens (
                    token_id   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                    token_hash text NOT NULL UNIQUE,
                    tenant_id  text NOT NULL REFERENCES hm_tenants(tenant_id),
                    label      text,
                    created_at timestamptz NOT NULL DEFAULT now()
                );
                CREATE INDEX IF NOT EXISTS hm_tokens_hash_idx ON hm_tokens (token_hash);",
            )
            .map_err(storage_error)?;

        // Enable RLS on events if it exists (separate query for clear error handling).
        // ALTER TABLE ENABLE/FORCE ROW LEVEL SECURITY is idempotent.
        // Graph projection tables (hm_nodes, hm_edges) are derived state; no RLS needed.
        let events_exists: bool = client
            .query_one(
                "SELECT EXISTS (
                    SELECT FROM information_schema.tables
                    WHERE table_schema = 'public' AND table_name = 'events'
                )",
                &[],
            )
            .map_err(storage_error)?
            .get(0);

        if events_exists {
            client
                .batch_execute(
                    "ALTER TABLE events ENABLE ROW LEVEL SECURITY;
                     ALTER TABLE events FORCE ROW LEVEL SECURITY;",
                )
                .map_err(storage_error)?;

            // Create the isolation policy if it does not exist yet.
            let policy_exists: bool = client
                .query_one(
                    "SELECT EXISTS (
                        SELECT FROM pg_policies
                        WHERE tablename = 'events' AND policyname = 'tenant_isolation'
                    )",
                    &[],
                )
                .map_err(storage_error)?
                .get(0);

            if !policy_exists {
                client
                    .execute(
                        "CREATE POLICY tenant_isolation ON events
                             USING (tenant_id = current_setting('app.tenant_id', true))",
                        &[],
                    )
                    .map_err(storage_error)?;
            }
        }

        Ok(())
    }

    /// Create a tenant and issue one bearer token for it.
    pub fn provision_tenant(
        &self,
        tenant_id: &str,
        display_name: &str,
    ) -> Result<ProvisionedTenant> {
        if tenant_id.trim().is_empty() {
            return Err(LedgerError::Storage("tenant_id must not be empty".to_owned()).into());
        }
        if display_name.trim().is_empty() {
            return Err(LedgerError::Storage("display_name must not be empty".to_owned()).into());
        }

        let secret_bytes: [u8; 32] = {
            let mut bytes = [0u8; 32];
            // Use UUID-based entropy for compatibility without external rand crate.
            let a = Uuid::new_v4();
            let b = Uuid::new_v4();
            bytes[..16].copy_from_slice(a.as_bytes());
            bytes[16..].copy_from_slice(b.as_bytes());
            bytes
        };
        let secret_hex = hex_encode(&secret_bytes);
        let token_secret = format!("{TOKEN_PREFIX}{secret_hex}");
        let token_hash = sha256_hex(secret_hex.as_bytes());

        let mut client = self.pool.get().map_err(storage_error)?;
        let mut tx = client.transaction().map_err(storage_error)?;

        tx.execute(
            "INSERT INTO hm_tenants (tenant_id, display_name)
             VALUES ($1, $2)
             ON CONFLICT (tenant_id) DO NOTHING",
            &[&tenant_id, &display_name],
        )
        .map_err(storage_error)?;

        let token_id: Uuid = tx
            .query_one(
                "INSERT INTO hm_tokens (token_hash, tenant_id, label)
                 VALUES ($1, $2, 'default')
                 RETURNING token_id",
                &[&token_hash, &tenant_id],
            )
            .map_err(storage_error)?
            .get(0);

        tx.commit().map_err(storage_error)?;

        Ok(ProvisionedTenant {
            tenant_id: tenant_id.to_owned(),
            token_id,
            token_secret,
        })
    }

    /// Resolve a bearer token to a tenant_id, or `None` if not found.
    pub fn resolve_token(&self, token: &str) -> Result<Option<String>> {
        let secret = token.strip_prefix(TOKEN_PREFIX).unwrap_or(token);
        let token_hash = sha256_hex(secret.as_bytes());

        let mut client = self.pool.get().map_err(storage_error)?;
        let row = client
            .query_opt(
                "SELECT tenant_id FROM hm_tokens WHERE token_hash = $1",
                &[&token_hash],
            )
            .map_err(storage_error)?;

        Ok(row.map(|r| r.get::<_, String>(0)))
    }

    /// Ensure a tenant exists for an OAuth user identified by their email address.
    ///
    /// Uses `user:<email>` as the stable tenant_id.  Creates the tenant row if
    /// it does not yet exist (idempotent via `ON CONFLICT DO NOTHING`).  No
    /// bearer token is issued — OAuth users authenticate via Better Auth.
    ///
    /// Returns the resolved `tenant_id` string.
    pub fn ensure_oauth_tenant(&self, email: &str) -> Result<String> {
        let tenant_id = format!("user:{email}");
        let mut client = self.pool.get().map_err(storage_error)?;
        client
            .execute(
                "INSERT INTO hm_tenants (tenant_id, display_name)
                 VALUES ($1, $2)
                 ON CONFLICT (tenant_id) DO NOTHING",
                &[&tenant_id, &email],
            )
            .map_err(storage_error)?;
        Ok(tenant_id)
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
