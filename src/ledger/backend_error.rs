use crate::error::LedgerError;

pub(super) fn storage_error(error: impl std::fmt::Display) -> LedgerError {
    LedgerError::Storage(error.to_string())
}
