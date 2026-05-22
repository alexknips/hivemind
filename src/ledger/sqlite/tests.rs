// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::ledger::contract_tests::{
    assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
    assert_replay_from_zero_in_order, make_event,
};
use crate::ledger::EventLedger;
use crate::Result;

use super::super::backend_error::storage_error;
use super::SqliteEventLedger;

#[test]
fn append_assigns_monotonic_ids() -> Result<()> {
    with_sqlite_ledger("append-monotonic", assert_monotonic_append)
}

#[test]
fn append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
    with_sqlite_ledger("append-dedup", assert_dedup_by_event_uuid)
}

#[test]
fn replay_from_zero_is_ordered() -> Result<()> {
    with_sqlite_ledger("replay-ordered", |ledger| {
        assert_replay_from_zero_in_order(ledger)
    })
}

#[test]
fn read_applies_offset_and_limit() -> Result<()> {
    with_sqlite_ledger("read-offset-limit", |ledger| {
        assert_read_offset_and_limit(ledger)
    })
}

#[test]
fn uses_wal_and_creates_file() -> Result<()> {
    with_sqlite_ledger("wal-and-file", |ledger| {
        assert!(ledger.path().exists());

        let journal_mode: Option<String> = ledger
            .connection
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .optional()
            .map_err(storage_error)?;

        assert_eq!(journal_mode.as_deref(), Some("wal"));
        Ok(())
    })
}

#[test]
#[ignore = "performance benchmark; run in isolated environment"]
fn ten_k_append_plus_read_stays_fast() -> Result<()> {
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
