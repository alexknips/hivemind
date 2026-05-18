use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use crate::error::LedgerError;
use crate::events::{Event, EventId, EventSource, EventType};
use crate::Result;

const LEDGER_DB_NAME: &str = "ledger.sqlite";

pub trait EventLedger {
    fn append(&self, event: Event) -> Result<EventId>;

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>>;

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()>;

    fn latest_offset(&self) -> Result<EventId>;
}

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
            &connection,
            "source",
            "ALTER TABLE events ADD COLUMN source TEXT NOT NULL DEFAULT 'cli'",
        )?;
        ensure_column(
            &connection,
            "source_ref",
            "ALTER TABLE events ADD COLUMN source_ref TEXT",
        )?;

        Ok(Self { path, connection })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventLedger for SqliteEventLedger {
    fn append(&self, event: Event) -> Result<EventId> {
        let payload = serde_json::to_string(&event.payload).map_err(storage_error)?;
        let ts = event.ts.unwrap_or_else(Utc::now).to_rfc3339();

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
                    event.event_uuid.to_string(),
                    event_type_as_str(event.event_type),
                    event.actor_id,
                    event.source.as_str(),
                    event.source_ref,
                    event.correlation_id,
                    event.causation_event_id.map(|id| id as i64),
                    payload,
                    ts,
                ],
            )
            .map_err(storage_error)?;

        if inserted == 1 {
            return Ok(u64::try_from(self.connection.last_insert_rowid())
                .map_err(|error| storage_error(format!("invalid sqlite rowid: {error}")))?);
        }

        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT event_id FROM events WHERE event_uuid = ?1",
                params![event.event_uuid.to_string()],
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

        Ok(u64::try_from(existing)
            .map_err(|error| storage_error(format!("invalid event_id: {error}")))?)
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
            .prepare(
                "SELECT
                    event_id,
                    event_uuid,
                    type,
                    actor_id,
                    source,
                    source_ref,
                    correlation_id,
                    causation_event_id,
                    payload,
                    ts
                 FROM events
                 WHERE event_id > ?1
                 ORDER BY event_id ASC
                 LIMIT ?2",
            )
            .map_err(storage_error)?;

        let mut rows = statement
            .query(params![offset, limit])
            .map_err(storage_error)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(storage_error)? {
            events.push(row_to_event(row)?);
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
            .prepare(
                "SELECT
                    event_id,
                    event_uuid,
                    type,
                    actor_id,
                    source,
                    source_ref,
                    correlation_id,
                    causation_event_id,
                    payload,
                    ts
                 FROM events
                 WHERE event_id > ?1
                 ORDER BY event_id ASC",
            )
            .map_err(storage_error)?;

        let mut rows = statement.query(params![offset]).map_err(storage_error)?;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let event = row_to_event(row)?;
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

        Ok(u64::try_from(offset)
            .map_err(|error| storage_error(format!("invalid latest_offset: {error}")))?)
    }
}

#[derive(Debug, Default)]
pub struct InMemoryEventLedger {
    state: Mutex<InMemoryState>,
}

#[derive(Debug, Default)]
struct InMemoryState {
    events: Vec<Event>,
    event_uuid_to_id: HashMap<Uuid, EventId>,
}

impl InMemoryEventLedger {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> Result<MutexGuard<'_, InMemoryState>> {
        self.state.lock().map_err(|error| {
            LedgerError::Storage(format!("in-memory ledger lock poisoned: {error}")).into()
        })
    }
}

impl EventLedger for InMemoryEventLedger {
    fn append(&self, mut event: Event) -> Result<EventId> {
        let mut state = self.state()?;
        if let Some(existing_id) = state.event_uuid_to_id.get(&event.event_uuid) {
            return Ok(*existing_id);
        }

        let next_id = (state.events.len() as EventId) + 1;
        event.event_id = Some(next_id);
        if event.ts.is_none() {
            event.ts = Some(Utc::now());
        }

        state.events.push(event.clone());
        state.event_uuid_to_id.insert(event.event_uuid, next_id);

        Ok(next_id)
    }

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let state = self.state()?;
        Ok(state
            .events
            .iter()
            .filter(|event| event.event_id.unwrap_or_default() > offset)
            .take(limit)
            .cloned()
            .collect())
    }

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        let replay_events = {
            let state = self.state()?;
            state
                .events
                .iter()
                .filter(|event| event.event_id.unwrap_or_default() > offset)
                .cloned()
                .collect::<Vec<_>>()
        };

        for event in &replay_events {
            callback(event)?;
        }

        Ok(())
    }

    fn latest_offset(&self) -> Result<EventId> {
        let state = self.state()?;
        Ok(state
            .events
            .last()
            .and_then(|event| event.event_id)
            .unwrap_or_default())
    }
}

fn row_to_event(row: &Row<'_>) -> Result<Event> {
    let event_id_raw: i64 = row.get("event_id").map_err(storage_error)?;
    let event_uuid_raw: String = row.get("event_uuid").map_err(storage_error)?;
    let event_type_raw: String = row.get("type").map_err(storage_error)?;
    let actor_id: String = row.get("actor_id").map_err(storage_error)?;
    let source_raw: String = row.get("source").map_err(storage_error)?;
    let source_ref: Option<String> = row.get("source_ref").map_err(storage_error)?;
    let correlation_id: Option<String> = row.get("correlation_id").map_err(storage_error)?;
    let causation_event_id_raw: Option<i64> =
        row.get("causation_event_id").map_err(storage_error)?;
    let payload_raw: String = row.get("payload").map_err(storage_error)?;
    let ts_raw: String = row.get("ts").map_err(storage_error)?;

    let event_id = u64::try_from(event_id_raw)
        .map_err(|error| storage_error(format!("invalid event_id in row: {error}")))?;
    let event_uuid = Uuid::parse_str(&event_uuid_raw)
        .map_err(|error| storage_error(format!("invalid event_uuid in row: {error}")))?;
    let event_type = parse_event_type(&event_type_raw)?;
    let source = parse_event_source(&source_raw)?;
    let causation_event_id = causation_event_id_raw
        .map(|id| {
            u64::try_from(id).map_err(|error| {
                storage_error(format!("invalid causation_event_id in row: {error}"))
            })
        })
        .transpose()?;
    let payload = serde_json::from_str(&payload_raw).map_err(storage_error)?;
    let ts = DateTime::parse_from_rfc3339(&ts_raw)
        .map_err(|error| storage_error(format!("invalid timestamp in row: {error}")))?
        .with_timezone(&Utc);

    Ok(Event {
        event_id: Some(event_id),
        event_uuid,
        correlation_id,
        causation_event_id,
        event_type,
        actor_id,
        source,
        source_ref,
        payload,
        ts: Some(ts),
    })
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

fn event_type_as_str(event_type: EventType) -> &'static str {
    match event_type {
        EventType::DecisionProposed => "decision.proposed",
        EventType::DecisionAccepted => "decision.accepted",
        EventType::DecisionRejected => "decision.rejected",
        EventType::DecisionSuperseded => "decision.superseded",
        EventType::EvidenceRecorded => "evidence.recorded",
        EventType::HypothesisRecorded => "hypothesis.recorded",
        EventType::RelationAdded => "relation.added",
    }
}

fn parse_event_type(value: &str) -> Result<EventType> {
    match value {
        "decision.proposed" => Ok(EventType::DecisionProposed),
        "decision.accepted" => Ok(EventType::DecisionAccepted),
        "decision.rejected" => Ok(EventType::DecisionRejected),
        "decision.superseded" => Ok(EventType::DecisionSuperseded),
        "evidence.recorded" => Ok(EventType::EvidenceRecorded),
        "hypothesis.recorded" => Ok(EventType::HypothesisRecorded),
        "relation.added" => Ok(EventType::RelationAdded),
        other => Err(storage_error(format!("unknown event type in row: {other}")).into()),
    }
}

fn parse_event_source(value: &str) -> Result<EventSource> {
    match value {
        "cli" => Ok(EventSource::Cli),
        "agent" => Ok(EventSource::Agent),
        "slack" => Ok(EventSource::Slack),
        "api" => Ok(EventSource::Api),
        other => Err(storage_error(format!("unknown event source in row: {other}")).into()),
    }
}

fn storage_error(error: impl std::fmt::Display) -> LedgerError {
    LedgerError::Storage(error.to_string())
}

#[cfg(test)]
pub(crate) mod contract_tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use crate::events::Event;
    use crate::events::{EventId, EventType};
    use crate::ledger::EventLedger;
    use crate::Result;

    pub fn assert_monotonic_append<L: EventLedger>(ledger: &L) -> Result<()> {
        let event_ids = [
            ledger.append(make_event("evidence-1", Uuid::new_v4()))?,
            ledger.append(make_event("evidence-2", Uuid::new_v4()))?,
            ledger.append(make_event("evidence-3", Uuid::new_v4()))?,
        ];

        assert_eq!(event_ids, [1, 2, 3]);
        assert_eq!(ledger.latest_offset()?, 3);
        Ok(())
    }

    pub fn assert_dedup_by_event_uuid<L: EventLedger>(ledger: &L) -> Result<()> {
        let duplicate_uuid = Uuid::new_v4();

        let first_id = ledger.append(make_event("evidence-original", duplicate_uuid))?;
        let second_id = ledger.append(make_event("evidence-ignored", duplicate_uuid))?;

        assert_eq!(first_id, 1);
        assert_eq!(second_id, 1);
        assert_eq!(ledger.latest_offset()?, 1);

        let events = ledger.read(0, 10)?;
        assert_eq!(events.len(), 1);
        assert_eq!(event_id_from_payload(&events[0]), "evidence-original");

        Ok(())
    }

    pub fn assert_replay_from_zero_in_order<L: EventLedger>(ledger: &L) -> Result<()> {
        ledger.append(make_event("evidence-a", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-b", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-c", Uuid::new_v4()))?;

        let mut replayed_ids = Vec::new();
        ledger.replay_from(0, &mut |event| {
            replayed_ids.push(event.event_id.unwrap_or_default());
            Ok(())
        })?;

        assert_eq!(replayed_ids, vec![1, 2, 3]);

        Ok(())
    }

    pub fn assert_read_offset_and_limit<L: EventLedger>(ledger: &L) -> Result<()> {
        ledger.append(make_event("evidence-a", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-b", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-c", Uuid::new_v4()))?;

        let events = ledger.read(1, 1)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, Some(2));

        let events = ledger.read(3, 5)?;
        assert!(events.is_empty());

        Ok(())
    }

    pub fn make_event(evidence_id: &str, event_uuid: Uuid) -> Event {
        Event {
            event_id: None,
            event_uuid,
            correlation_id: None,
            causation_event_id: None,
            event_type: EventType::EvidenceRecorded,
            actor_id: "actor:test".to_owned(),
            source: crate::events::EventSource::Cli,
            source_ref: None,
            payload: json!({
                "evidence_id": evidence_id,
                "content": format!("content for {evidence_id}"),
                "source": "unit-test"
            }),
            ts: Some(Utc::now()),
        }
    }

    fn event_id_from_payload(event: &Event) -> &str {
        event
            .payload
            .get("evidence_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    fn _assert_event_ids_are_monotonic(ids: &[EventId]) {
        let mut previous = 0;
        for id in ids {
            assert!(*id > previous);
            previous = *id;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use rusqlite::OptionalExtension;
    use uuid::Uuid;

    use crate::ledger::contract_tests::{
        assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
        assert_replay_from_zero_in_order, make_event,
    };

    use super::{EventLedger, InMemoryEventLedger, SqliteEventLedger};
    use crate::Result;

    #[test]
    fn in_memory_append_assigns_monotonic_ids() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_monotonic_append(&ledger)
    }

    #[test]
    fn in_memory_append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_dedup_by_event_uuid(&ledger)
    }

    #[test]
    fn in_memory_replay_from_zero_is_ordered() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_replay_from_zero_in_order(&ledger)
    }

    #[test]
    fn in_memory_read_applies_offset_and_limit() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_read_offset_and_limit(&ledger)
    }

    #[test]
    fn sqlite_append_assigns_monotonic_ids() -> Result<()> {
        with_sqlite_ledger("append-monotonic", |ledger| assert_monotonic_append(ledger))
    }

    #[test]
    fn sqlite_append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
        with_sqlite_ledger("append-dedup", |ledger| assert_dedup_by_event_uuid(ledger))
    }

    #[test]
    fn sqlite_replay_from_zero_is_ordered() -> Result<()> {
        with_sqlite_ledger("replay-ordered", |ledger| {
            assert_replay_from_zero_in_order(ledger)
        })
    }

    #[test]
    fn sqlite_read_applies_offset_and_limit() -> Result<()> {
        with_sqlite_ledger("read-offset-limit", |ledger| {
            assert_read_offset_and_limit(ledger)
        })
    }

    #[test]
    fn sqlite_uses_wal_and_creates_file() -> Result<()> {
        with_sqlite_ledger("wal-and-file", |ledger| {
            assert!(ledger.path().exists());

            let journal_mode: Option<String> = ledger
                .connection
                .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
                .optional()
                .map_err(super::storage_error)?;

            assert_eq!(journal_mode.as_deref(), Some("wal"));
            Ok(())
        })
    }

    #[test]
    #[ignore = "performance benchmark; run in isolated environment"]
    fn sqlite_10k_append_plus_read_stays_fast() -> Result<()> {
        with_sqlite_ledger("ten-k-fast", |ledger| {
            let start = Instant::now();

            for index in 0..10_000 {
                let uuid = Uuid::new_v4();
                let event = make_event(&format!("evidence-{index}"), uuid);
                ledger.append(event)?;
            }

            let events = ledger.read(0, 10_000)?;
            assert_eq!(events.len(), 10_000);
            assert!(start.elapsed().as_secs_f64() < 1.0);

            Ok(())
        })
    }

    fn with_sqlite_ledger<T>(
        prefix: &str,
        f: impl FnOnce(&SqliteEventLedger) -> Result<T>,
    ) -> Result<T> {
        let dir = temp_hivemind_dir(prefix);
        let ledger = SqliteEventLedger::open(&dir)?;
        let result = f(&ledger);
        let _ = fs::remove_dir_all(&dir);
        result
    }

    fn temp_hivemind_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("hivemind-{prefix}-{nanos}-{}", std::process::id()))
    }
}
