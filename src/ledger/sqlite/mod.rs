mod row;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::LedgerError;
use crate::events::{Event, EventId};
use crate::Result;

use super::backend_error::storage_error;
use super::EventLedger;

const LEDGER_DB_NAME: &str = "ledger.sqlite";
const SQLITE_BUSY_TIMEOUT_MS: u64 = 30_000;

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
    fn append(&self, event: Event) -> Result<EventId> {
        let stored = row::StoredEvent::from_event(event)?;
        let event_uuid = stored.event_uuid.clone();

        let inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO events (
                    event_uuid,
                    type,
                    actor_id,
                    source,
                    source_ref,
                    correlation_id,
                    causation_event_id,
                    payload,
                    ts
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    stored.event_uuid,
                    stored.event_type,
                    stored.actor_id,
                    stored.source,
                    stored.source_ref,
                    stored.correlation_id,
                    stored.causation_event_id,
                    stored.payload,
                    stored.ts,
                ],
            )
            .map_err(storage_error)?;

        if inserted == 1 {
            return rowid_to_event_id(self.connection.last_insert_rowid(), "sqlite rowid");
        }

        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT event_id FROM events WHERE event_uuid = ?1",
                params![event_uuid],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;

        let existing = existing.ok_or_else(|| {
            LedgerError::Storage(
                "event dedup failed: duplicate event_uuid not found after INSERT OR IGNORE"
                    .to_owned(),
            )
        })?;

        rowid_to_event_id(existing, "event_id")
    }

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>> {
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
            .query(params![offset, limit])
            .map_err(storage_error)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(storage_error)? {
            events.push(row::event_from_row(row)?);
        }

        Ok(events)
    }

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        let offset = i64::try_from(offset)
            .map_err(|error| storage_error(format!("offset out of range: {error}")))?;

        let mut statement = self
            .connection
            .prepare(&replay_events_sql())
            .map_err(storage_error)?;

        let mut rows = statement.query(params![offset]).map_err(storage_error)?;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let event = row::event_from_row(row)?;
            callback(&event)?;
        }

        Ok(())
    }

    fn latest_offset(&self) -> Result<EventId> {
        let offset: i64 = self
            .connection
            .query_row("SELECT COALESCE(MAX(event_id), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(storage_error)?;

        rowid_to_event_id(offset, "latest_offset")
    }
}

fn initialize_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS events (
                 event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 event_uuid TEXT NOT NULL UNIQUE,
                 type TEXT NOT NULL,
                 actor_id TEXT NOT NULL,
                 source TEXT NOT NULL DEFAULT 'cli',
                 source_ref TEXT,
                 correlation_id TEXT,
                 causation_event_id INTEGER,
                 payload TEXT NOT NULL,
                 ts TEXT NOT NULL
             );",
        )
        .map_err(storage_error)?;
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

    Ok(())
}

fn ensure_column(connection: &Connection, column_name: &str, alter_sql: &str) -> Result<()> {
    let mut statement = connection
        .prepare("PRAGMA table_info(events)")
        .map_err(storage_error)?;
    let column_names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;

    if !column_names.iter().any(|name| name == column_name) {
        connection.execute(alter_sql, []).map_err(storage_error)?;
    }

    Ok(())
}

fn rowid_to_event_id(rowid: i64, label: &str) -> Result<EventId> {
    Ok(u64::try_from(rowid).map_err(|error| storage_error(format!("invalid {label}: {error}")))?)
}

fn event_columns_sql() -> &'static str {
    "event_id,
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
        "SELECT {} FROM events WHERE event_id > ?1 ORDER BY event_id ASC LIMIT ?2",
        event_columns_sql()
    )
}

fn replay_events_sql() -> String {
    format!(
        "SELECT {} FROM events WHERE event_id > ?1 ORDER BY event_id ASC",
        event_columns_sql()
    )
}

#[cfg(test)]
mod tests;
