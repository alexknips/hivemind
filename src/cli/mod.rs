mod args;
mod current_project;
pub(crate) mod project_context;
mod render;
pub mod run;

// Public API - maintain external interface unchanged
pub use args::*;
pub use render::{exit_code_for_error, format_error, render_decision_dot};
pub use run::run;

// Test-only re-imports: everything tests.rs needs via `use super::*`
#[cfg(test)]
use {
    crate::error::{CliError, CommandError},
    crate::ledger::{EventLedger, SqliteEventLedger},
    crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant},
    crate::queries::{derive_decision_status, DecisionStatus},
    crate::HivemindError,
    chrono::Utc,
    clap::Parser,
    project_context::ProjectContextEnv,
    run::{
        added_since_request, cli_tenant, decision_capture_actor_and_provenance,
        parse_graph_backend, recent_decisions_request, resolve_diff_bound,
        review_recent_decisions_request, run_emit_in_context, run_emit_with_notices,
        run_review_session, run_supersede_in_context, run_supersede_with_notices, TimeZoneSpec,
    },
    std::path::PathBuf,
};

#[cfg(test)]
mod tests;
