// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
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
