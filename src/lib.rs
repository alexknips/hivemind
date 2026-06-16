pub mod api;
pub mod cli;
pub mod commands;
pub mod error;
pub mod events;
pub mod identity;
pub mod ingest;
pub mod ledger;
pub mod mcp;
pub mod projector;
pub mod queries;
pub mod slack_app;
pub mod suggest;
#[cfg(feature = "tui")]
pub mod tui;

pub use error::{
    CliError, CommandError, HivemindError, LedgerError, ProjectorError, QueryError, Result,
};
