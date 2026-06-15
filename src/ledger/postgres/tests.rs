// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use uuid::Uuid;

use crate::error::LedgerError;
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
        let tenant_a_events = tenant_a.read(0, 10)?;
        let tenant_b_events = tenant_b.read(0, 10)?;
        let Some(tenant_a_event) = tenant_a_events.first() else {
            return Err(test_error("tenant-a event missing"));
        };
        let Some(tenant_b_event) = tenant_b_events.first() else {
            return Err(test_error("tenant-b event missing"));
        };

        if tenant_a_id != 1 {
            return Err(test_error("tenant-a first event id mismatch"));
        }
        if tenant_b_id != 1 {
            return Err(test_error("tenant-b first event id mismatch"));
        }
        if payload_evidence_id(tenant_a_event) != "tenant-a-event" {
            return Err(test_error("tenant-a payload mismatch"));
        }
        if payload_evidence_id(tenant_b_event) != "tenant-b-event" {
            return Err(test_error("tenant-b payload mismatch"));
        }

        Ok(())
    })
}

#[test]
fn replay_matches_sqlite_event_stream() -> Result<()> {
    with_postgres_ledger("sqlite-parity", |postgres| {
        let sqlite_dir = temp_hivemind_dir("sqlite-parity");
        let sqlite = SqliteEventLedger::open(&sqlite_dir)?;

        const EVIDENCE_IDS: [&str; 5] = [
            "evidence-0",
            "evidence-1",
            "evidence-2",
            "evidence-3",
            "evidence-4",
        ];
        for (index, evidence_id) in EVIDENCE_IDS.into_iter().enumerate() {
            let event = make_parity_event(evidence_id, index);
            sqlite.append(event)?;
        }

        let expected_events = sqlite.read(0, 16)?;
        let events_to_replay = expected_events.clone();
        for event in events_to_replay {
            postgres.append(event)?;
        }

        if expected_events != postgres.read(0, 16)? {
            return Err(test_error("postgres event stream differs from sqlite"));
        }

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

            left.join()
                .map_err(|_| test_error("tenant-a thread panicked"))??;
            right
                .join()
                .map_err(|_| test_error("tenant-b thread panicked"))??;
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
    let mut evidence_id = String::with_capacity(prefix.len() + 20);
    for index in 0..count {
        evidence_id.clear();
        let _ = write!(&mut evidence_id, "{prefix}-{index}");
        ledger.append(make_event(&evidence_id, Uuid::new_v4()))?;
    }
    Ok(())
}

fn assert_tenant_stream(
    ledger: &PostgresEventLedger,
    expected_prefix: &str,
    expected_count: usize,
) -> Result<()> {
    let events = ledger.read(0, expected_count + 1)?;
    if events.len() != expected_count {
        return Err(test_error("tenant stream event count mismatch"));
    }
    for (index, event) in events.iter().enumerate() {
        let expected_event_id =
            u64::try_from(index + 1).map_err(|_| test_error("event index out of range"))?;
        if event.event_id != Some(expected_event_id) {
            return Err(test_error("tenant stream event id mismatch"));
        }
        if !payload_evidence_id(event).starts_with(expected_prefix) {
            return Err(test_error("tenant stream payload prefix mismatch"));
        }
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
    let seconds = i64::try_from(index).unwrap_or(0);
    event.ts = DateTime::from_timestamp(1_769_385_600 + seconds, 123_456_000);
    event
}

fn unique_tenant(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("tenant:test:{prefix}:{nanos}:{}", std::process::id())
}

fn temp_hivemind_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("hivemind-{prefix}-{nanos}-{}", std::process::id()))
}

fn test_error(message: impl Into<String>) -> crate::HivemindError {
    LedgerError::Storage(message.into()).into()
}
