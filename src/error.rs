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
mod tests;
