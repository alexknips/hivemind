pub type Result<T> = std::result::Result<T, HivemindError>;

#[derive(Debug, thiserror::Error)]
pub enum HivemindError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),

    #[error(transparent)]
    Projector(#[from] ProjectorError),

    #[error(transparent)]
    Command(#[from] CommandError),

    #[error(transparent)]
    Query(#[from] QueryError),

    #[error(transparent)]
    Cli(#[from] CliError),
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("ledger storage error: {0}")]
    Storage(String),

    #[error("ledger serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectorError {
    #[error("graph projection error: {0}")]
    Projection(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("validation failed: {0}")]
    Validation(String),

    #[error("invariant violated: {0}")]
    Invariant(String),
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("query failed: {0}")]
    Execution(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("invalid command line input: {0}")]
    InvalidInput(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_errors_convert_to_crate_error() {
        let ledger_error: HivemindError = LedgerError::Storage("disk full".to_owned()).into();
        assert!(matches!(ledger_error, HivemindError::Ledger(_)));

        let projector_error: HivemindError =
            ProjectorError::Projection("missing node".to_owned()).into();
        assert!(matches!(projector_error, HivemindError::Projector(_)));

        let command_error: HivemindError =
            CommandError::Invariant("actor_id is required".to_owned()).into();
        assert!(matches!(command_error, HivemindError::Command(_)));

        let query_error: HivemindError = QueryError::Execution("timeout".to_owned()).into();
        assert!(matches!(query_error, HivemindError::Query(_)));

        let cli_error: HivemindError = CliError::InvalidInput("--actor is empty".to_owned()).into();
        assert!(matches!(cli_error, HivemindError::Cli(_)));
    }
}
