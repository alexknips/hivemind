// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ledger::contract_tests::{
    assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
    assert_replay_from_zero_in_order, make_event,
};
use crate::ledger::{EventLedger, SqliteEventLedger};
use crate::Result;

use super::PostgresEventLedger;

const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

#[test]
fn append_assigns_monotonic_ids() -> Result<()> {
    with_postgres_ledger("append-monotonic", assert_monotonic_append)
}

#[test]
fn append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
    with_postgres_ledger("append-dedup", assert_dedup_by_event_uuid)
}

#[test]
fn replay_from_zero_is_ordered() -> Result<()> {
    with_postgres_ledger("replay-ordered", |ledger| {
        assert_replay_from_zero_in_order(ledger)
    })
}

#[test]
fn read_applies_offset_and_limit() -> Result<()> {
    with_postgres_ledger("read-offset-limit", |ledger| {
        assert_read_offset_and_limit(ledger)
    })
}

#[test]
fn same_event_uuid_is_idempotent_only_inside_tenant() -> Result<()> {
    with_postgres_ledger("tenant-dedup-a", |tenant_a| {
        let tenant_b = tenant_a.for_tenant(unique_tenant("tenant-dedup-b"))?;
        let event_uuid = Uuid::new_v4();

        let tenant_a_id = tenant_a.append(make_event("tenant-a-event", event_uuid))?;
        let tenant_b_id = tenant_b.append(make_event("tenant-b-event", event_uuid))?;

        assert_eq!(tenant_a_id, 1);
        assert_eq!(tenant_b_id, 1);
        assert_eq!(
            payload_evidence_id(&tenant_a.read(0, 10)?[0]),
            "tenant-a-event"
        );
        assert_eq!(
            payload_evidence_id(&tenant_b.read(0, 10)?[0]),
            "tenant-b-event"
        );

        Ok(())
    })
}

#[test]
fn replay_matches_sqlite_event_stream() -> Result<()> {
    with_postgres_ledger("sqlite-parity", |postgres| {
        let sqlite_dir = temp_hivemind_dir("sqlite-parity");
        let sqlite = SqliteEventLedger::open(&sqlite_dir)?;

        for index in 0..5 {
            let event = make_parity_event(&format!("evidence-{index}"), index);
            sqlite.append(event.clone())?;
            postgres.append(event)?;
        }

        assert_eq!(sqlite.read(0, 16)?, postgres.read(0, 16)?);

        let _ = fs::remove_dir_all(sqlite_dir);
        Ok(())
    })
}

#[test]
fn concurrent_tenant_writes_are_isolated_streams() -> Result<()> {
    with_postgres_ledger("concurrent-a", |tenant_a| {
        let tenant_a = tenant_a.clone();
        let tenant_b = tenant_a.for_tenant(unique_tenant("concurrent-b"))?;

        std::thread::scope(|scope| {
            let left = scope.spawn(|| append_events(&tenant_a, "tenant-a", 16));
            let right = scope.spawn(|| append_events(&tenant_b, "tenant-b", 16));

            left.join().expect("tenant-a thread panicked")?;
            right.join().expect("tenant-b thread panicked")?;
            Ok::<_, crate::HivemindError>(())
        })?;

        assert_tenant_stream(&tenant_a, "tenant-a", 16)?;
        assert_tenant_stream(&tenant_b, "tenant-b", 16)?;

        Ok(())
    })
}

fn with_postgres_ledger<T>(
    prefix: &str,
    f: impl FnOnce(&PostgresEventLedger) -> Result<T>,
) -> Result<()> {
    let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping Postgres ledger test; set {TEST_DATABASE_URL_ENV}");
        return Ok(());
    };

    let tenant_id = unique_tenant(prefix);
    let ledger = PostgresEventLedger::connect_with_pool_size(&database_url, tenant_id, 4)?;
    f(&ledger)?;
    Ok(())
}

fn append_events(ledger: &PostgresEventLedger, prefix: &str, count: usize) -> Result<()> {
    for index in 0..count {
        ledger.append(make_event(&format!("{prefix}-{index}"), Uuid::new_v4()))?;
    }
    Ok(())
}

fn assert_tenant_stream(
    ledger: &PostgresEventLedger,
    expected_prefix: &str,
    expected_count: usize,
) -> Result<()> {
    let events = ledger.read(0, expected_count + 1)?;
    assert_eq!(events.len(), expected_count);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.event_id, Some((index + 1) as u64));
        assert!(
            payload_evidence_id(event).starts_with(expected_prefix),
            "unexpected event in tenant stream: {:?}",
            event.payload
        );
    }
    Ok(())
}

fn payload_evidence_id(event: &crate::events::Event) -> &str {
    event
        .payload
        .get("evidence_id")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn make_parity_event(evidence_id: &str, index: usize) -> crate::events::Event {
    let mut event = make_event(evidence_id, Uuid::new_v4());
    event.ts = Some(
        DateTime::parse_from_rfc3339(&format!("2026-05-25T00:00:{index:02}.123456Z"))
            .expect("valid test timestamp")
            .with_timezone(&Utc),
    );
    event
}

fn unique_tenant(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    format!("tenant:test:{prefix}:{nanos}:{}", std::process::id())
}

fn temp_hivemind_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("hivemind-{prefix}-{nanos}-{}", std::process::id()))
}
