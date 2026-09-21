use crate::error::LedgerError;

pub(super) fn storage_error(error: impl std::fmt::Display) -> LedgerError {
    LedgerError::Storage(error.to_string())
}

/// A tenant referenced at ledger-open time has no registry row on this
/// backend. `create_hint` names the backend-specific way to create one
/// (SQLite: `hivemind tenant create`; Postgres: the server's provisioning
/// route) so the error is actionable without a lookup.
pub(super) fn unknown_tenant_error(
    tenant_id: &str,
    create_hint: impl std::fmt::Display,
) -> LedgerError {
    LedgerError::Storage(format!("unknown tenant '{tenant_id}': {create_hint}"))
}
