use std::str::FromStr;

use postgres::Config;
use postgres_native_tls::MakeTlsConnector;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::LedgerError;
use crate::Result;

use super::super::backend_error::{storage_error, unknown_tenant_error};

type PgPool = Pool<PostgresConnectionManager<MakeTlsConnector>>;

pub(crate) const TOKEN_PREFIX: &str = "hm_tk_";

/// Result of provisioning a new tenant.
pub struct ProvisionedTenant {
    pub tenant_id: String,
    pub token_id: Uuid,
    /// Full bearer secret — `hm_tk_<64-hex>`. Returned ONCE, never stored.
    pub token_secret: String,
}

/// Auth resolution result for an opaque bearer token.
pub struct ResolvedToken {
    pub tenant_id: String,
    pub user_id: Option<Uuid>,
    pub actor_id: String,
}

/// A newly created user with their first bearer token.
pub struct ProvisionedUser {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub token_id: Uuid,
    pub token_secret: String,
}

/// Summary of a user for listing.
pub struct UserInfo {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
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
        Self::from_pool(pool)
    }

    /// Build a store sharing an existing pool (e.g. `PostgresEventLedger::
    /// pool()`) instead of opening a second one — see `pool()`'s doc comment.
    /// Schema init still runs (idempotent, cheap), so this is safe to call
    /// on every ledger open. Same ordering requirement as `connect`: the
    /// pool's tables must already exist.
    pub fn from_pool(pool: PgPool) -> Result<Self> {
        let store = Self { pool };
        store.initialize_schema()?;
        Ok(store)
    }

    /// Create provisioning tables and enable RLS on the event ledger.
    ///
    /// The whole sequence (table creation, column migrations, RLS
    /// enablement, policy creation) runs inside one transaction, opened
    /// with `pg_advisory_xact_lock(hashtext('hivemind_schema_init'))` as
    /// its first statement — the SAME key `PostgresEventLedger::
    /// initialize_schema` locks on before creating `events`. Two
    /// invariants depend on this:
    ///
    /// 1. `CREATE TABLE IF NOT EXISTS` is not race-free across concurrent
    ///    sessions creating the same not-yet-existing table for the first
    ///    time (a documented Postgres caveat); the shared lock serializes
    ///    every caller of either `initialize_schema` so only one session
    ///    ever runs first-time creation at once.
    /// 2. `ALTER TABLE events ENABLE/FORCE ROW LEVEL SECURITY` takes an
    ///    ACCESS EXCLUSIVE lock; running the policy check-and-create in
    ///    the SAME transaction keeps that lock held until the policy also
    ///    exists, so a concurrent reader/writer never observes RLS
    ///    enabled with no policy (which denies every row, even to the
    ///    owner, since FORCE is set).
    ///
    /// Violating either one reproduces hivemind-g9kv: on a brand-new
    /// database, ledger tests intermittently failed with a raw db error
    /// — whichever test's `initialize_schema` lost the race saw a
    /// duplicate-object error from `CREATE TABLE IF NOT EXISTS`, or (for
    /// the RLS gap specifically) a `next_event_id` lookup that
    /// momentarily saw zero rows for a tenant that already had some,
    /// producing a PRIMARY KEY collision that `ON CONFLICT (tenant_id,
    /// event_uuid)` doesn't cover.
    pub fn initialize_schema(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(storage_error)?;
        let mut tx = client.transaction().map_err(storage_error)?;

        tx.batch_execute("SELECT pg_advisory_xact_lock(hashtext('hivemind_schema_init'));")
            .map_err(storage_error)?;

        tx.batch_execute(
            "CREATE TABLE IF NOT EXISTS hm_tenants (
                    tenant_id    text PRIMARY KEY,
                    display_name text NOT NULL,
                    created_at   timestamptz NOT NULL DEFAULT now()
                );
                CREATE TABLE IF NOT EXISTS hm_users (
                    user_id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                    tenant_id    text NOT NULL REFERENCES hm_tenants(tenant_id),
                    email        text NOT NULL,
                    display_name text NOT NULL,
                    role         text NOT NULL DEFAULT 'member',
                    created_at   timestamptz NOT NULL DEFAULT now(),
                    UNIQUE (tenant_id, email)
                );
                CREATE TABLE IF NOT EXISTS hm_tokens (
                    token_id    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                    token_hash  text NOT NULL UNIQUE,
                    tenant_id   text NOT NULL REFERENCES hm_tenants(tenant_id),
                    user_id     uuid REFERENCES hm_users(user_id),
                    actor_id    text NOT NULL DEFAULT 'service:api',
                    label       text,
                    revoked_at  timestamptz,
                    created_at  timestamptz NOT NULL DEFAULT now()
                );
                CREATE INDEX IF NOT EXISTS hm_tokens_hash_idx ON hm_tokens (token_hash);
                CREATE TABLE IF NOT EXISTS hm_oidc_users (
                    sub        text PRIMARY KEY,
                    tenant_id  text NOT NULL REFERENCES hm_tenants(tenant_id),
                    email      text,
                    created_at timestamptz NOT NULL DEFAULT now()
                );",
        )
        .map_err(storage_error)?;

        // Idempotent column additions for deployments that predate this schema version.
        tx.batch_execute(
            "ALTER TABLE hm_tokens ADD COLUMN IF NOT EXISTS user_id    uuid REFERENCES hm_users(user_id);
                 ALTER TABLE hm_tokens ADD COLUMN IF NOT EXISTS actor_id   text NOT NULL DEFAULT 'service:api';
                 ALTER TABLE hm_tokens ADD COLUMN IF NOT EXISTS revoked_at timestamptz;",
        )
        .map_err(storage_error)?;

        // Enable RLS on events if it exists.
        // Graph projection tables (hm_nodes, hm_edges) are derived state; no RLS needed.
        let events_exists: bool = tx
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
            tx.batch_execute(
                "ALTER TABLE events ENABLE ROW LEVEL SECURITY;
                 ALTER TABLE events FORCE ROW LEVEL SECURITY;",
            )
            .map_err(storage_error)?;

            let policy_exists: bool = tx
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
                tx.execute(
                    "CREATE POLICY tenant_isolation ON events
                         USING (tenant_id = current_setting('app.tenant_id', true))",
                    &[],
                )
                .map_err(storage_error)?;
            }
        }

        tx.commit().map_err(storage_error)?;

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

        let (token_secret, token_hash) = generate_token_secret();

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

    /// Resolve an OIDC user (WorkOS `sub`) to a tenant_id, auto-provisioning
    /// a tenant on first login. The tenant is keyed by `sub` so each WorkOS
    /// user gets an isolated tenant in the existing multi-tenant model.
    pub fn resolve_or_create_oidc_user(&self, sub: &str, email: &str) -> Result<String> {
        let mut client = self.pool.get().map_err(storage_error)?;

        // Fast path: already mapped.
        if let Some(row) = client
            .query_opt(
                "SELECT tenant_id FROM hm_oidc_users WHERE sub = $1",
                &[&sub],
            )
            .map_err(storage_error)?
        {
            return Ok(row.get::<_, String>(0));
        }

        // Auto-provision a tenant for this OIDC user. The tenant_id is stable
        // across logins so the user's decision history persists.
        let tenant_id = format!("oidc:{sub}");
        let display_name = if email.is_empty() { sub } else { email };

        let mut tx = client.transaction().map_err(storage_error)?;
        tx.execute(
            "INSERT INTO hm_tenants (tenant_id, display_name)
             VALUES ($1, $2)
             ON CONFLICT (tenant_id) DO NOTHING",
            &[&tenant_id, &display_name],
        )
        .map_err(storage_error)?;
        tx.execute(
            "INSERT INTO hm_oidc_users (sub, tenant_id, email)
             VALUES ($1, $2, $3)
             ON CONFLICT (sub) DO UPDATE SET email = EXCLUDED.email",
            &[&sub, &tenant_id, &email],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;

        Ok(tenant_id)
    }

    /// Create a user and issue their first bearer token.
    pub fn create_user(
        &self,
        tenant_id: &str,
        email: &str,
        display_name: &str,
        role: &str,
    ) -> Result<ProvisionedUser> {
        if email.trim().is_empty() {
            return Err(LedgerError::Storage("email must not be empty".to_owned()).into());
        }
        if !matches!(role, "admin" | "member") {
            return Err(LedgerError::Storage("role must be 'admin' or 'member'".to_owned()).into());
        }

        let (token_secret, token_hash) = generate_token_secret();
        let actor_id = format!("human:{email}");

        let mut client = self.pool.get().map_err(storage_error)?;
        let mut tx = client.transaction().map_err(storage_error)?;

        let user_id: Uuid = tx
            .query_one(
                "INSERT INTO hm_users (tenant_id, email, display_name, role)
                 VALUES ($1, $2, $3, $4)
                 RETURNING user_id",
                &[&tenant_id, &email, &display_name, &role],
            )
            .map_err(storage_error)?
            .get(0);

        let token_id: Uuid = tx
            .query_one(
                "INSERT INTO hm_tokens (token_hash, tenant_id, user_id, actor_id, label)
                 VALUES ($1, $2, $3, $4, 'default')
                 RETURNING token_id",
                &[&token_hash, &tenant_id, &user_id, &actor_id],
            )
            .map_err(storage_error)?
            .get(0);

        tx.commit().map_err(storage_error)?;

        Ok(ProvisionedUser {
            user_id,
            email: email.to_owned(),
            display_name: display_name.to_owned(),
            role: role.to_owned(),
            token_id,
            token_secret,
        })
    }

    /// Mint an additional bearer token for an existing user.
    pub fn mint_user_token(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        label: Option<&str>,
    ) -> Result<ProvisionedUser> {
        let (token_secret, token_hash) = generate_token_secret();

        let mut client = self.pool.get().map_err(storage_error)?;

        let row = client
            .query_opt(
                "SELECT email, display_name, role FROM hm_users WHERE user_id = $1 AND tenant_id = $2",
                &[&user_id, &tenant_id],
            )
            .map_err(storage_error)?
            .ok_or_else(|| LedgerError::Storage("user not found".to_owned()))?;

        let email: String = row.get(0);
        let display_name: String = row.get(1);
        let role: String = row.get(2);
        let actor_id = format!("human:{email}");

        let token_id: Uuid = client
            .query_one(
                "INSERT INTO hm_tokens (token_hash, tenant_id, user_id, actor_id, label)
                 VALUES ($1, $2, $3, $4, $5)
                 RETURNING token_id",
                &[&token_hash, &tenant_id, &user_id, &actor_id, &label],
            )
            .map_err(storage_error)?
            .get(0);

        Ok(ProvisionedUser {
            user_id,
            email,
            display_name,
            role,
            token_id,
            token_secret,
        })
    }

    /// Whether `tenant_id` has an `hm_tenants` row.
    pub fn tenant_exists(&self, tenant_id: &str) -> Result<bool> {
        let mut client = self.pool.get().map_err(storage_error)?;
        let exists: bool = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM hm_tenants WHERE tenant_id = $1)",
                &[&tenant_id],
            )
            .map_err(storage_error)?
            .get(0);
        Ok(exists)
    }

    /// Errors unless `tenant_id` has an `hm_tenants` row. Called once at
    /// ledger-open time (the CLI/stdio-MCP seam in
    /// [`crate::ledger::AnyLedger::open`] and the HTTP API's per-request
    /// seam) so a typo'd `--tenant`/token-resolved tenant cannot silently
    /// open a fresh, empty tenant scope. Tenant creation stays the server's
    /// provisioning route (`POST /v1/tenants`), never the CLI.
    pub fn ensure_known_tenant(&self, tenant_id: &str) -> Result<()> {
        if !self.tenant_exists(tenant_id)? {
            return Err(unknown_tenant_error(
                tenant_id,
                "tenants are created through the server's provisioning route \
                 (POST /v1/tenants), not the CLI",
            )
            .into());
        }
        Ok(())
    }

    /// Revoke a token. Returns false if not found or already revoked.
    pub fn revoke_token(&self, tenant_id: &str, token_id: Uuid) -> Result<bool> {
        let mut client = self.pool.get().map_err(storage_error)?;
        let n = client
            .execute(
                "UPDATE hm_tokens SET revoked_at = now()
                 WHERE token_id = $1 AND tenant_id = $2 AND revoked_at IS NULL",
                &[&token_id, &tenant_id],
            )
            .map_err(storage_error)?;
        Ok(n > 0)
    }

    /// List all users for a tenant.
    pub fn list_users(&self, tenant_id: &str) -> Result<Vec<UserInfo>> {
        let mut client = self.pool.get().map_err(storage_error)?;
        let rows = client
            .query(
                "SELECT user_id, email, display_name, role FROM hm_users
                 WHERE tenant_id = $1 ORDER BY created_at",
                &[&tenant_id],
            )
            .map_err(storage_error)?;
        Ok(rows
            .into_iter()
            .map(|r| UserInfo {
                user_id: r.get(0),
                email: r.get(1),
                display_name: r.get(2),
                role: r.get(3),
            })
            .collect())
    }

    /// Resolve a bearer token to auth context, or `None` if not found/revoked.
    pub fn resolve_token(&self, token: &str) -> Result<Option<ResolvedToken>> {
        let secret = token.strip_prefix(TOKEN_PREFIX).unwrap_or(token);
        let token_hash = sha256_hex(secret.as_bytes());

        let mut client = self.pool.get().map_err(storage_error)?;
        let row = client
            .query_opt(
                "SELECT tenant_id, user_id, actor_id FROM hm_tokens
                 WHERE token_hash = $1 AND revoked_at IS NULL",
                &[&token_hash],
            )
            .map_err(storage_error)?;

        Ok(row.map(|r| ResolvedToken {
            tenant_id: r.get(0),
            user_id: r.get(1),
            actor_id: r.get(2),
        }))
    }
}

fn generate_token_secret() -> (String, String) {
    let secret_bytes: [u8; 32] = {
        let mut bytes = [0u8; 32];
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        bytes[..16].copy_from_slice(a.as_bytes());
        bytes[16..].copy_from_slice(b.as_bytes());
        bytes
    };
    let secret_hex = hex_encode(&secret_bytes);
    let token_secret = format!("{TOKEN_PREFIX}{secret_hex}");
    let token_hash = sha256_hex(secret_hex.as_bytes());
    (token_secret, token_hash)
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
