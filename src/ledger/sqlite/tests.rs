// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::commands::{CommandContext, Commands, DecisionProposalEventUuids};
use crate::error::CommandError;
use crate::events::{Event, EventProvenance, TenantId};
use crate::ledger::contract_tests::{
    assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
    assert_replay_from_zero_in_order, make_event,
};
use crate::ledger::EventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::get_decision;
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
fn tenant_scoped_sqlite_ledger_isolates_commands_and_queries() -> Result<()> {
    with_sqlite_ledger("tenant-isolation", |ledger| {
        let tenant_a = test_tenant_id("tenant:a")?;
        let tenant_b = test_tenant_id("tenant:b")?;

        write_shared_decision(ledger, &tenant_a, "Tenant A decision")?;
        write_shared_decision(ledger, &tenant_b, "Tenant B decision")?;

        let tenant_a_events = ledger.read_for_tenant(&tenant_a, 0, 10)?;
        let tenant_b_events = ledger.read_for_tenant(&tenant_b, 0, 10)?;
        require_event_count("tenant a event count", &tenant_a_events, 2)?;
        require_event_count("tenant b event count", &tenant_b_events, 2)?;
        require_all_events_for_tenant("tenant a events", &tenant_a_events, &tenant_a)?;
        require_all_events_for_tenant("tenant b events", &tenant_b_events, &tenant_b)?;

        let graph_a = MemoryGraph::default();
        rebuild_graph_for_tenant(ledger, &tenant_a, &graph_a)?;
        let decision_a = get_decision(&graph_a, "decision-shared")?
            .data
            .ok_or_else(|| test_failure("tenant a decision exists"))?;
        require_title("tenant a decision", &decision_a.title, "Tenant A decision")?;

        let graph_b = MemoryGraph::default();
        rebuild_graph_for_tenant(ledger, &tenant_b, &graph_b)?;
        let decision_b = get_decision(&graph_b, "decision-shared")?
            .data
            .ok_or_else(|| test_failure("tenant b decision exists"))?;
        require_title("tenant b decision", &decision_b.title, "Tenant B decision")?;

        Ok(())
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
fn configures_busy_timeout_for_write_contention() -> Result<()> {
    with_sqlite_ledger("busy-timeout", |ledger| {
        let busy_timeout_ms: i64 = ledger
            .connection
            .query_row("PRAGMA busy_timeout;", [], |row| row.get(0))
            .map_err(storage_error)?;

        assert_eq!(busy_timeout_ms, 30_000);
        Ok(())
    })
}

#[test]
fn concurrent_first_open_initializes_schema_once() -> Result<()> {
    let dir = temp_hivemind_dir("concurrent-first-open");
    fs::create_dir_all(&dir).map_err(storage_error)?;

    const EVENT_IDS: [&str; 8] = [
        "concurrent-first-open-0",
        "concurrent-first-open-1",
        "concurrent-first-open-2",
        "concurrent-first-open-3",
        "concurrent-first-open-4",
        "concurrent-first-open-5",
        "concurrent-first-open-6",
        "concurrent-first-open-7",
    ];
    let worker_count = EVENT_IDS.len();
    let dir = Arc::new(dir);
    let barrier = Arc::new(Barrier::new(worker_count));
    let mut handles = Vec::new();
    for event_id in EVENT_IDS {
        let dir = Arc::clone(&dir);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || -> Result<()> {
            barrier.wait();
            let ledger = SqliteEventLedger::open(dir.as_ref())?;
            ledger.append(make_event(event_id, Uuid::new_v4()))?;
            Ok(())
        }));
    }

    for handle in handles {
        handle
            .join()
            .map_err(|_| storage_error("concurrent open worker panicked"))??;
    }

    let ledger = SqliteEventLedger::open(dir.as_ref())?;
    let expected_events =
        u64::try_from(worker_count).map_err(|_| storage_error("worker count out of range"))?;
    if ledger.latest_offset()? != expected_events {
        return Err(storage_error("concurrent first open event count mismatch").into());
    }

    let _ = fs::remove_dir_all(dir.as_ref());
    Ok(())
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

fn write_shared_decision(
    ledger: &SqliteEventLedger,
    tenant_id: &TenantId,
    title: &str,
) -> Result<()> {
    let commands = Commands::new_with_context(
        ledger,
        CommandContext::new(
            tenant_id.clone(),
            EventProvenance::api(Some("tenant-test".to_owned())),
        ),
    );
    commands.record_option_with_id(
        "actor:test",
        "option-shared",
        "Shared",
        "Shared option label across tenants",
    )?;
    commands.propose_decision_with_id(
        "actor:test",
        "decision-shared",
        title,
        "Tenant-specific rationale",
        &["tenant-isolation".to_owned()],
        &["option-shared".to_owned()],
        None,
        &[],
        &[],
        DecisionProposalEventUuids {
            proposal: Uuid::from_u128(1),
            has_option: vec![Uuid::from_u128(2)],
            chose: None,
            assumes: Vec::new(),
            based_on: Vec::new(),
        },
    )?;
    Ok(())
}

fn test_tenant_id(value: &str) -> Result<TenantId> {
    TenantId::new(value)
        .map_err(|error| test_failure(format!("invalid test tenant id {value}: {error}")))
}

fn require_event_count(label: &str, events: &[Event], expected: usize) -> Result<()> {
    if events.len() == expected {
        Ok(())
    } else {
        Err(test_failure(format!(
            "{label}: expected {expected}, got {}",
            events.len()
        )))
    }
}

fn require_all_events_for_tenant(
    label: &str,
    events: &[Event],
    tenant_id: &TenantId,
) -> Result<()> {
    if events.iter().all(|event| event.tenant_id == *tenant_id) {
        Ok(())
    } else {
        Err(test_failure(format!(
            "{label}: expected only tenant {tenant_id}"
        )))
    }
}

fn require_title(label: &str, actual: &str, expected: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(test_failure(format!(
            "{label}: expected title {expected:?}, got {actual:?}"
        )))
    }
}

fn test_failure(message: impl Into<String>) -> crate::HivemindError {
    CommandError::Invariant(message.into()).into()
}
