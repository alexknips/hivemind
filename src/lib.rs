//! HiveMind library root: re-exports the public API surface and wires submodule visibility.

pub(crate) mod anthropic;
pub mod api;
pub mod classifier;
pub mod cli;
pub mod commands;
pub mod connector;
pub mod embedding;
pub mod error;
pub mod events;
pub(crate) mod grounding;
pub mod identity;
pub mod ingest;
pub mod ledger;
pub mod linear;
pub mod map;
pub mod mcp;
pub mod projector;
pub mod queries;
pub mod scorer;
pub mod slack_app;
pub mod suggest;
pub mod summarize;
#[cfg(feature = "tui")]
pub mod tui;
pub(crate) mod util;

pub use error::{
    CliError, CommandError, HivemindError, LedgerError, ProjectorError, QueryError, Result,
};

/// Commit this binary was built from (short sha), embedded by `build.rs`.
/// "unknown" only when built outside any git checkout with no
/// `HIVEMIND_BUILD_SHA` override (e.g. a stray source tarball).
pub const BUILD_SHA: &str = env!("HIVEMIND_BUILD_SHA");

/// `<cargo semver>+<build sha>` — what `--version` and `/v1/version` report.
/// Cargo.toml's bare semver only moves on a tagged release; this suffix is
/// what makes every build between releases distinguishable (hivemind-zdsh.7).
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("HIVEMIND_BUILD_SHA"));
