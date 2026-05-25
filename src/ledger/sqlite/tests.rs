// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::commands::{CommandContext, Commands, DecisionProposalEventUuids};
use crate::events::{EventProvenance, TenantId};
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
        let tenant_a = TenantId::new("tenant:a").expect("tenant a is valid");
        let tenant_b = TenantId::new("tenant:b").expect("tenant b is valid");

        write_shared_decision(ledger, tenant_a.clone(), "Tenant A decision")?;
        write_shared_decision(ledger, tenant_b.clone(), "Tenant B decision")?;

        let tenant_a_events = ledger.read_for_tenant(&tenant_a, 0, 10)?;
        let tenant_b_events = ledger.read_for_tenant(&tenant_b, 0, 10)?;
        assert_eq!(tenant_a_events.len(), 2);
        assert_eq!(tenant_b_events.len(), 2);
        assert!(tenant_a_events
            .iter()
            .all(|event| event.tenant_id == tenant_a));
        assert!(tenant_b_events
            .iter()
            .all(|event| event.tenant_id == tenant_b));

        let graph_a = MemoryGraph::default();
        rebuild_graph_for_tenant(ledger, &tenant_a, &graph_a)?;
        let decision_a = get_decision(&graph_a, "decision-shared")?
            .data
            .expect("tenant a decision exists");
        assert_eq!(decision_a.title, "Tenant A decision");

        let graph_b = MemoryGraph::default();
        rebuild_graph_for_tenant(ledger, &tenant_b, &graph_b)?;
        let decision_b = get_decision(&graph_b, "decision-shared")?
            .data
            .expect("tenant b decision exists");
        assert_eq!(decision_b.title, "Tenant B decision");

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
    tenant_id: TenantId,
    title: &str,
) -> Result<()> {
    let commands = Commands::new_with_context(
        ledger,
        CommandContext::new(
            tenant_id,
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
