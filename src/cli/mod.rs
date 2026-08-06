mod args;
mod render;
pub mod run;

// Public API - maintain external interface unchanged
pub use args::*;
pub use render::{exit_code_for_error, format_error, render_decision_dot};
pub use run::run;

// Test-only re-imports: everything tests.rs needs via `use super::*`
#[cfg(test)]
use {
    chrono::Utc,
    clap::Parser,
    std::path::PathBuf,
    crate::error::{CliError, CommandError},
    crate::ledger::{EventLedger, SqliteEventLedger},
    crate::projector::{rebuild_graph_for_tenant, memory::MemoryGraph},
    crate::queries::{derive_decision_status, DecisionStatus},
    crate::HivemindError,
    run::{
        added_since_request, cli_tenant, parse_graph_backend, recent_decisions_request,
        resolve_diff_bound, review_recent_decisions_request, run_review_session, TimeZoneSpec,
    },
};

#[cfg(test)]
mod tests;
