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

impl LedgerConfig {
    /// Where a write through this config lands, in words: the tenant and either the local
    /// ledger directory or the shared database. Never the database URL, which carries a
    /// credential.
    pub fn address(&self, tenant_id: &TenantId) -> String {
        match self.database_url.as_deref() {
            Some(url) if !url.is_empty() => format!("tenant `{tenant_id}` in the shared database"),
            _ => format!(
                "tenant `{tenant_id}` in the local ledger under {}",
                self.hivemind_dir.display()
            ),
        }
    }
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
    ///
    /// Errors if `tenant_id` is unknown on the selected backend (no
    /// `tenants` row on SQLite, no `hm_tenants` row on Postgres) — a typo'd
    /// `--tenant` must not silently open a fresh, empty tenant scope.
    pub fn open(config: &LedgerConfig, tenant_id: &TenantId) -> Result<Self> {
        match config.database_url.as_deref() {
            Some(url) if !url.is_empty() => Self::open_postgres(url, tenant_id),
            _ => Self::open_sqlite(&config.hivemind_dir, tenant_id),
        }
    }

    fn open_sqlite(hivemind_dir: &std::path::Path, tenant_id: &TenantId) -> Result<Self> {
        let ledger = SqliteEventLedger::open(hivemind_dir)?;
        ledger.ensure_known_tenant(tenant_id)?;
        Ok(AnyLedger::Sqlite(ledger))
    }

    #[cfg(feature = "shared-backend-postgres")]
    fn open_postgres(url: &str, tenant_id: &TenantId) -> Result<Self> {
        let ledger = PostgresEventLedger::connect(url, tenant_id.as_str())?;
        // Shares the ledger's pool (see PostgresEventLedger::pool()'s doc
        // comment) rather than opening a second one per one-shot CLI/
        // stdio-MCP invocation. Built AFTER the ledger above (see
        // TenantStore::from_pool) so RLS can be enabled on the tables it
        // just created — mirrors AppState::from_config's startup order.
        let tenant_store = crate::ledger::TenantStore::from_pool(ledger.pool().clone())?;
        tenant_store.ensure_known_tenant(tenant_id.as_str())?;
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

#[cfg(test)]
mod tests;
