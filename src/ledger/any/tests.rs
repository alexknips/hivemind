#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::ledger::contract_tests::{
    assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
    assert_replay_from_zero_in_order,
};

fn temp_hivemind_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "hivemind-any-ledger-{prefix}-{nanos}-{}",
        std::process::id()
    ))
}

fn with_sqlite_any_ledger<T>(prefix: &str, f: impl FnOnce(&AnyLedger) -> Result<T>) -> Result<T> {
    let dir = temp_hivemind_dir(prefix);
    let config = LedgerConfig {
        hivemind_dir: dir.clone(),
        database_url: None,
    };
    let ledger = AnyLedger::open(&config, &TenantId::local())?;
    let result = f(&ledger);
    let _ = fs::remove_dir_all(&dir);
    result
}

#[test]
fn open_selects_sqlite_when_database_url_unset() -> Result<()> {
    with_sqlite_any_ledger("unset", |ledger| {
        assert!(matches!(ledger, AnyLedger::Sqlite(_)));
        Ok(())
    })
}

#[test]
fn open_selects_sqlite_when_database_url_empty() -> Result<()> {
    let dir = temp_hivemind_dir("empty");
    let config = LedgerConfig {
        hivemind_dir: dir.clone(),
        database_url: Some(String::new()),
    };
    let ledger = AnyLedger::open(&config, &TenantId::local())?;
    assert!(matches!(ledger, AnyLedger::Sqlite(_)));
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn contract_tests_pass_against_any_ledger_sqlite() -> Result<()> {
    with_sqlite_any_ledger("contract-monotonic", assert_monotonic_append)?;
    with_sqlite_any_ledger("contract-dedup", assert_dedup_by_event_uuid)?;
    with_sqlite_any_ledger("contract-replay", assert_replay_from_zero_in_order)?;
    with_sqlite_any_ledger("contract-offset-limit", assert_read_offset_and_limit)?;
    Ok(())
}

#[cfg(not(feature = "shared-backend-postgres"))]
#[test]
fn open_names_the_feature_when_url_set_but_not_compiled() {
    let config = LedgerConfig {
        hivemind_dir: temp_hivemind_dir("not-compiled"),
        database_url: Some("postgres://example/db".to_owned()),
    };
    let err = AnyLedger::open(&config, &TenantId::local())
        .expect_err("a database URL without the feature must fail");
    assert!(err.to_string().contains("shared-backend-postgres"));
}

/// Regression test: `open` must surface the Postgres connection error
/// as-is rather than silently opening a local SQLite ledger instead.
/// The wait for r2d2's default 30s connection_timeout is expected — a
/// closed loopback port fails each attempt fast, but r2d2 retries with
/// backoff until the deadline before giving up.
#[cfg(feature = "shared-backend-postgres")]
#[test]
fn open_does_not_fall_back_to_sqlite_on_unreachable_postgres_url() {
    let config = LedgerConfig {
        hivemind_dir: temp_hivemind_dir("unreachable"),
        database_url: Some("postgres://user:pass@127.0.0.1:1/hivemind_test".to_owned()),
    };
    let result = AnyLedger::open(&config, &TenantId::local());
    assert!(
        result.is_err(),
        "unreachable Postgres URL must not silently succeed"
    );
    assert!(
        !config.hivemind_dir.exists(),
        "must not have opened (and thereby created) a SQLite ledger as a fallback"
    );
}

#[cfg(feature = "shared-backend-postgres")]
#[test]
fn open_selects_postgres_when_database_url_set() -> Result<()> {
    let Some(database_url) = std::env::var("HIVEMIND_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping Postgres AnyLedger test; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let tenant_id = TenantId::new(format!("tenant:test:any-ledger-open:{nanos}"))
        .map_err(|e| crate::error::LedgerError::Storage(e.to_string()))?;
    let config = LedgerConfig {
        hivemind_dir: temp_hivemind_dir("postgres-selected"),
        database_url: Some(database_url),
    };
    let ledger = AnyLedger::open(&config, &tenant_id)?;
    assert!(matches!(ledger, AnyLedger::Postgres(_)));
    Ok(())
}
