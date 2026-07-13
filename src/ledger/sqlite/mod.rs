mod row;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::LedgerError;
use crate::events::{Event, EventId, TenantId};
use crate::Result;

use super::backend_error::storage_error;
use super::EventLedger;

const LEDGER_DB_NAME: &str = "ledger.sqlite";
const SQLITE_BUSY_TIMEOUT_MS: u64 = 30_000;
const SQLITE_LOCK_RETRY_SLEEP_MS: u64 = 10;

#[derive(Debug)]
pub struct SqliteEventLedger {
    path: PathBuf,
    connection: Connection,
}

impl SqliteEventLedger {
    pub fn open(hivemind_dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(hivemind_dir.as_ref()).map_err(storage_error)?;
        let path = hivemind_dir.as_ref().join(LEDGER_DB_NAME);
        let connection = Connection::open(&path).map_err(storage_error)?;
        connection
            .busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(storage_error)?;

        initialize_schema(&connection)?;

        Ok(Self { path, connection })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventLedger for SqliteEventLedger {
    fn append_for_tenant(&self, tenant_id: &TenantId, mut event: Event) -> Result<EventId> {
        event.tenant_id = tenant_id.clone(); // ubs:ignore: per-append copy; false positive from impl EventLedger for.
        let stored = row::StoredEvent::from_event(event)?;

        let inserted = retry_sqlite_lock(|| {
            self.connection.execute(
                "INSERT OR IGNORE INTO events (
                    tenant_id,
                    event_uuid,
                    type,
                    actor_id,
                    source,
                    source_ref,
                    correlation_id,
                    causation_event_id,
                    payload,
                    ts
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    stored.tenant_id.as_str(),
                    stored.event_uuid.as_str(),
                    stored.event_type,
                    stored.actor_id.as_str(),
                    stored.source,
                    stored.source_ref.as_deref(),
                    stored.correlation_id.as_deref(),
                    stored.causation_event_id,
                    stored.payload.as_str(),
                    stored.ts.as_str(),
                ],
            )
        })
        .map_err(storage_error)?;

        if inserted == 1 {
            return rowid_to_event_id(self.connection.last_insert_rowid(), "sqlite rowid");
        }

        let existing: Option<i64> = retry_sqlite_lock(|| {
            self.connection
                .query_row(
                    "SELECT event_id FROM events WHERE tenant_id = ?1 AND event_uuid = ?2",
                    params![stored.tenant_id.as_str(), stored.event_uuid.as_str()],
                    |row| row.get(0),
                )
                .optional()
        })
        .map_err(storage_error)?;

        let existing = existing.ok_or_else(|| {
            LedgerError::Storage(String::from(
                "event dedup failed: duplicate event_uuid not found after INSERT OR IGNORE",
            ))
        })?;

        rowid_to_event_id(existing, "event_id")
    }

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let offset = i64::try_from(offset)
            .map_err(|error| storage_error(format!("offset out of range: {error}")))?;
        let limit = i64::try_from(limit)
            .map_err(|error| storage_error(format!("limit out of range: {error}")))?;

        let mut statement = self
            .connection
            .prepare(&read_events_sql())
            .map_err(storage_error)?;

        let mut rows = statement
            .query(params![tenant_id.as_str(), offset, limit])
            .map_err(storage_error)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(storage_error)? {
            events.push(row::event_from_row(row)?);
        }

        Ok(events)
    }

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        let offset = i64::try_from(offset)
            .map_err(|error| storage_error(format!("offset out of range: {error}")))?;

        let mut statement = self
            .connection
            .prepare(&replay_events_sql())
            .map_err(storage_error)?;

        let mut rows = statement
            .query(params![tenant_id.as_str(), offset])
            .map_err(storage_error)?;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let event = row::event_from_row(row)?;
            callback(&event)?;
        }

        Ok(())
    }

    fn latest_offset_for_tenant(&self, tenant_id: &TenantId) -> Result<EventId> {
        let offset: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE(MAX(event_id), 0) FROM events WHERE tenant_id = ?1",
                params![tenant_id.as_str()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;

        rowid_to_event_id(offset, "latest_offset")
    }
}

fn initialize_schema(connection: &Connection) -> Result<()> {
    retry_sqlite_lock(|| {
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS events (
                 event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 tenant_id TEXT NOT NULL DEFAULT 'local',
                 event_uuid TEXT NOT NULL,
                 type TEXT NOT NULL,
                 actor_id TEXT NOT NULL,
                 source TEXT NOT NULL DEFAULT 'cli',
                 source_ref TEXT,
                 correlation_id TEXT,
                 causation_event_id INTEGER,
                 payload TEXT NOT NULL,
                 ts TEXT NOT NULL,
                 UNIQUE(tenant_id, event_uuid)
             );",
        )?;
        ensure_column(
            connection,
            "tenant_id",
            "ALTER TABLE events ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'local'",
        )?;
        ensure_column(
            connection,
            "source",
            "ALTER TABLE events ADD COLUMN source TEXT NOT NULL DEFAULT 'cli'",
        )?;
        ensure_column(
            connection,
            "source_ref",
            "ALTER TABLE events ADD COLUMN source_ref TEXT",
        )?;
        ensure_tenant_scoped_event_uuid(connection)?;
        Ok(())
    })
    .map_err(storage_error)?;

    Ok(())
}

fn ensure_tenant_scoped_event_uuid(connection: &Connection) -> rusqlite::Result<()> {
    let create_sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'events'",
        [],
        |row| row.get(0),
    )?;

    if create_sql.contains("UNIQUE(tenant_id, event_uuid)")
        && !create_sql.contains("event_uuid TEXT NOT NULL UNIQUE")
    {
        return Ok(());
    }

    connection.execute_batch(
        "BEGIN IMMEDIATE;
             ALTER TABLE events RENAME TO events_pre_tenant_scope;
             CREATE TABLE events (
                 event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 tenant_id TEXT NOT NULL DEFAULT 'local',
                 event_uuid TEXT NOT NULL,
                 type TEXT NOT NULL,
                 actor_id TEXT NOT NULL,
                 source TEXT NOT NULL DEFAULT 'cli',
                 source_ref TEXT,
                 correlation_id TEXT,
                 causation_event_id INTEGER,
                 payload TEXT NOT NULL,
                 ts TEXT NOT NULL,
                 UNIQUE(tenant_id, event_uuid)
             );
             INSERT INTO events (
                 event_id,
                 tenant_id,
                 event_uuid,
                 type,
                 actor_id,
                 source,
                 source_ref,
                 correlation_id,
                 causation_event_id,
                 payload,
                 ts
             )
             SELECT
                 event_id,
                 COALESCE(NULLIF(tenant_id, ''), 'local'),
                 event_uuid,
                 type,
                 actor_id,
                 source,
                 source_ref,
                 correlation_id,
                 causation_event_id,
                 payload,
                 ts
             FROM events_pre_tenant_scope;
             DROP TABLE events_pre_tenant_scope;
             COMMIT;",
    )?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    column_name: &str,
    alter_sql: &str,
) -> rusqlite::Result<()> {
    let mut statement = connection.prepare("PRAGMA table_info(events)")?;
    let column_names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if !column_names.iter().any(|name| name == column_name) {
        connection.execute(alter_sql, [])?;
    }

    Ok(())
}

fn retry_sqlite_lock<T>(mut operation: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let deadline = Instant::now() + Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS);
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if is_retryable_sqlite_lock(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(SQLITE_LOCK_RETRY_SLEEP_MS));
            }
            Err(error) => return Err(error),
        }
    }
}

fn is_retryable_sqlite_lock(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn rowid_to_event_id(rowid: i64, label: &str) -> Result<EventId> {
    Ok(u64::try_from(rowid).map_err(|error| storage_error(format!("invalid {label}: {error}")))?)
}

fn event_columns_sql() -> &'static str {
    "tenant_id,
     event_id,
     event_uuid,
     type,
     actor_id,
     source,
     source_ref,
     correlation_id,
     causation_event_id,
     payload,
     ts"
}

fn read_events_sql() -> String {
    format!(
        "SELECT {} FROM events WHERE tenant_id = ?1 AND event_id > ?2 ORDER BY event_id ASC LIMIT ?3",
        event_columns_sql()
    )
}

fn replay_events_sql() -> String {
    format!(
        "SELECT {} FROM events WHERE tenant_id = ?1 AND event_id > ?2 ORDER BY event_id ASC",
        event_columns_sql()
    )
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// SQLite user / token store
// ---------------------------------------------------------------------------

pub const SQLITE_TOKEN_PREFIX: &str = "hm_tk_";

pub struct SqliteResolvedToken {
    pub user_id: Uuid,
    pub tenant_id: String,
    pub actor_id: String,
}

pub struct SqliteProvisionedUser {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub token_id: Uuid,
    pub token_secret: String,
}

pub struct SqliteUserInfo {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
}

/// Per-user token store backed by the same SQLite file as the event ledger.
pub struct SqliteUserStore {
    path: PathBuf,
}

impl SqliteUserStore {
    pub fn open(hivemind_dir: &Path) -> crate::Result<Self> {
        fs::create_dir_all(hivemind_dir).map_err(storage_error)?;
        let path = hivemind_dir.join(LEDGER_DB_NAME); // ubs:ignore: constant LEDGER_DB_NAME segment, not user input
        let conn = Connection::open(&path).map_err(storage_error)?;
        initialize_user_schema(&conn).map_err(storage_error)?;
        Ok(Self { path })
    }

    fn connect(&self) -> crate::Result<Connection> {
        let conn = Connection::open(&self.path).map_err(storage_error)?;
        conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(storage_error)?;
        Ok(conn)
    }

    pub fn create_user(
        &self,
        tenant_id: &str,
        email: &str,
        display_name: &str,
        role: &str,
    ) -> crate::Result<SqliteProvisionedUser> {
        if email.trim().is_empty() {
            return Err(LedgerError::Storage("email must not be empty".to_owned()).into());
        }
        if !matches!(role, "admin" | "member") {
            return Err(LedgerError::Storage("role must be 'admin' or 'member'".to_owned()).into());
        }

        let user_id = Uuid::new_v4();
        let (token_secret, token_hash) = sqlite_generate_token_secret();
        let token_id = Uuid::new_v4();
        let actor_id = format!("human:{email}");

        let conn = self.connect()?;
        retry_sqlite_lock(|| {
            conn.execute(
                "INSERT INTO hm_users (user_id, tenant_id, email, display_name, role)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![user_id.to_string(), tenant_id, email, display_name, role],
            )?;
            conn.execute(
                "INSERT INTO hm_tokens (token_id, token_hash, tenant_id, user_id, actor_id, label)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'default')",
                params![
                    token_id.to_string(),
                    token_hash,
                    tenant_id,
                    user_id.to_string(),
                    actor_id
                ],
            )?;
            Ok(())
        })
        .map_err(storage_error)?;

        Ok(SqliteProvisionedUser {
            user_id,
            email: email.to_owned(),
            display_name: display_name.to_owned(),
            role: role.to_owned(),
            token_id,
            token_secret,
        })
    }

    pub fn mint_user_token(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        label: Option<&str>,
    ) -> crate::Result<(Uuid, String)> {
        let (token_secret, token_hash) = sqlite_generate_token_secret();
        let token_id = Uuid::new_v4();

        let conn = self.connect()?;
        let actor_id: String = retry_sqlite_lock(|| {
            conn.query_row(
                "SELECT 'human:' || email FROM hm_users WHERE user_id = ?1 AND tenant_id = ?2",
                params![user_id.to_string(), tenant_id],
                |r| r.get(0),
            )
        })
        .map_err(storage_error)?;

        retry_sqlite_lock(|| {
            conn.execute(
                "INSERT INTO hm_tokens (token_id, token_hash, tenant_id, user_id, actor_id, label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    token_id.to_string(),
                    token_hash,
                    tenant_id,
                    user_id.to_string(),
                    actor_id,
                    label
                ],
            )
        })
        .map_err(storage_error)?;

        Ok((token_id, token_secret))
    }

    pub fn revoke_token(&self, tenant_id: &str, token_id: Uuid) -> crate::Result<bool> {
        let conn = self.connect()?;
        let n = retry_sqlite_lock(|| {
            conn.execute(
                "UPDATE hm_tokens SET revoked_at = datetime('now')
                 WHERE token_id = ?1 AND tenant_id = ?2 AND revoked_at IS NULL",
                params![token_id.to_string(), tenant_id],
            )
        })
        .map_err(storage_error)?;
        Ok(n > 0)
    }

    pub fn list_users(&self, tenant_id: &str) -> crate::Result<Vec<SqliteUserInfo>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT user_id, email, display_name, role FROM hm_users
                 WHERE tenant_id = ?1 ORDER BY rowid",
            )
            .map_err(storage_error)?;
        let rows: Vec<_> = stmt
            .query_map(params![tenant_id], |r| {
                let uid: String = r.get(0)?;
                Ok(SqliteUserInfo {
                    user_id: Uuid::parse_str(&uid).unwrap_or_default(),
                    email: r.get(1)?,
                    display_name: r.get(2)?,
                    role: r.get(3)?,
                })
            })
            .map_err(storage_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)?;
        Ok(rows)
    }

    pub fn resolve_token(&self, token: &str) -> crate::Result<Option<SqliteResolvedToken>> {
        let secret = token.strip_prefix(SQLITE_TOKEN_PREFIX).unwrap_or(token);
        let token_hash = sqlite_sha256_hex(secret.as_bytes());

        let conn = self.connect()?;
        let row = retry_sqlite_lock(|| {
            conn.query_row(
                "SELECT t.tenant_id, t.user_id, t.actor_id
                 FROM hm_tokens t
                 WHERE t.token_hash = ?1 AND t.revoked_at IS NULL",
                params![token_hash],
                |r| {
                    let uid_str: String = r.get(1)?;
                    Ok((r.get::<_, String>(0)?, uid_str, r.get::<_, String>(2)?))
                },
            )
            .optional()
        })
        .map_err(storage_error)?;

        Ok(row.and_then(|(tenant_id, uid_str, actor_id)| {
            Uuid::parse_str(&uid_str)
                .ok()
                .map(|user_id| SqliteResolvedToken {
                    tenant_id,
                    user_id,
                    actor_id,
                })
        }))
    }
}

fn initialize_user_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hm_users (
             user_id      TEXT PRIMARY KEY,
             tenant_id    TEXT NOT NULL DEFAULT 'local',
             email        TEXT NOT NULL,
             display_name TEXT NOT NULL,
             role         TEXT NOT NULL DEFAULT 'member',
             created_at   TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE (tenant_id, email)
         );
         CREATE TABLE IF NOT EXISTS hm_tokens (
             token_id    TEXT PRIMARY KEY,
             token_hash  TEXT NOT NULL UNIQUE,
             tenant_id   TEXT NOT NULL DEFAULT 'local',
             user_id     TEXT REFERENCES hm_users(user_id),
             actor_id    TEXT NOT NULL DEFAULT 'service:api',
             label       TEXT,
             revoked_at  TEXT,
             created_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX IF NOT EXISTS hm_tokens_hash_idx ON hm_tokens (token_hash);",
    )
}

fn sqlite_generate_token_secret() -> (String, String) {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(a.as_bytes()); // ubs:ignore: fixed-size [0u8;32] slice at compile-time-known bound
    bytes[16..].copy_from_slice(b.as_bytes()); // ubs:ignore: fixed-size [0u8;32] slice at compile-time-known bound
    let hex = bytes_to_lower_hex(&bytes);
    let token_secret = format!("{SQLITE_TOKEN_PREFIX}{hex}");
    let token_hash = sqlite_sha256_hex(hex.as_bytes());
    (token_secret, token_hash)
}

fn sqlite_sha256_hex(data: &[u8]) -> String {
    bytes_to_lower_hex(&Sha256::digest(data))
}

fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
