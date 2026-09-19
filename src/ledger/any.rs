//! `AnyLedger`: the ledger value type for one-shot process invocations (CLI,
//! stdio MCP), selected by [`LedgerConfig`] at open time. Mirrors the HTTP
//! API server's per-request ledger value (`api::ApiLedger`, itself an alias
//! for this type): both dispatch to SQLite or Postgres through one enum so
//! downstream signatures are shared. The server keeps its own `ApiBackend`
//! (shared pool + `for_tenant`) as its per-request factory; `AnyLedger::open`
//! is for one-shot processes and opens a fresh connection each time.

use std::path::PathBuf;

#[cfg(not(feature = "shared-backend-postgres"))]
use crate::error::CliError;
use crate::events::{Event, EventId, TenantId};
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::PostgresEventLedger;
use crate::ledger::{EventLedger, SqliteEventLedger};
use crate::Result;

/// Backend selector for [`AnyLedger::open`].
#[derive(Debug, Clone)]
pub struct LedgerConfig {
    pub hivemind_dir: PathBuf,
    /// `None` or empty selects SQLite.
    pub database_url: Option<String>,
}

/// One ledger connection for one process invocation.
#[derive(Debug)]
pub enum AnyLedger {
    Sqlite(SqliteEventLedger),
    #[cfg(feature = "shared-backend-postgres")]
    Postgres(PostgresEventLedger),
}

impl AnyLedger {
    /// Selects the backend from `config.database_url`: set and non-empty
    /// connects to Postgres (a hard error names the feature when this binary
    /// was not built with `shared-backend-postgres`); unset or empty opens
    /// the local SQLite ledger under `config.hivemind_dir`. Never falls back
    /// from Postgres to SQLite on connection failure — a misconfigured cell
    /// must fail loudly, not silently write to a local file.
    pub fn open(config: &LedgerConfig, tenant_id: &TenantId) -> Result<Self> {
        match config.database_url.as_deref() {
            Some(url) if !url.is_empty() => Self::open_postgres(url, tenant_id),
            _ => Ok(AnyLedger::Sqlite(SqliteEventLedger::open(
                &config.hivemind_dir,
            )?)),
        }
    }

    #[cfg(feature = "shared-backend-postgres")]
    fn open_postgres(url: &str, tenant_id: &TenantId) -> Result<Self> {
        let ledger = PostgresEventLedger::connect(url, tenant_id.as_str())?;
        Ok(AnyLedger::Postgres(ledger))
    }

    #[cfg(not(feature = "shared-backend-postgres"))]
    fn open_postgres(_url: &str, _tenant_id: &TenantId) -> Result<Self> {
        Err(CliError::InvalidInput(
            "a database URL was configured but this binary was built without the \
             shared-backend-postgres feature"
                .to_owned(),
        )
        .into())
    }
}

impl EventLedger for AnyLedger {
    fn append_for_tenant(&self, tenant_id: &TenantId, event: Event) -> Result<EventId> {
        match self {
            AnyLedger::Sqlite(l) => l.append_for_tenant(tenant_id, event),
            // Use explicit trait dispatch to avoid the inherent &str overload.
            #[cfg(feature = "shared-backend-postgres")]
            AnyLedger::Postgres(l) => EventLedger::append_for_tenant(l, tenant_id, event),
        }
    }

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        match self {
            AnyLedger::Sqlite(l) => l.read_for_tenant(tenant_id, offset, limit),
            #[cfg(feature = "shared-backend-postgres")]
            AnyLedger::Postgres(l) => EventLedger::read_for_tenant(l, tenant_id, offset, limit),
        }
    }

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        match self {
            AnyLedger::Sqlite(l) => l.replay_from_for_tenant(tenant_id, offset, callback),
            #[cfg(feature = "shared-backend-postgres")]
            AnyLedger::Postgres(l) => {
                EventLedger::replay_from_for_tenant(l, tenant_id, offset, callback)
            }
        }
    }

    fn latest_offset_for_tenant(&self, tenant_id: &TenantId) -> Result<EventId> {
        match self {
            AnyLedger::Sqlite(l) => l.latest_offset_for_tenant(tenant_id),
            #[cfg(feature = "shared-backend-postgres")]
            AnyLedger::Postgres(l) => EventLedger::latest_offset_for_tenant(l, tenant_id),
        }
    }
}

// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
mod tests {
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

    fn with_sqlite_any_ledger<T>(
        prefix: &str,
        f: impl FnOnce(&AnyLedger) -> Result<T>,
    ) -> Result<T> {
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
}
