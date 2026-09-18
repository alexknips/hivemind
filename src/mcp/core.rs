//! Transport-agnostic core for MCP tools.
//!
//! Both the stdio transport (`mcp.rs`) and the MCP-over-HTTP transport
//! (`api/mcp_http.rs`) implement the same ~15-tool surface. Historically
//! each did so with its own ~90%-identical body, which already drifted once
//! (see hivemind-whvd). This module is where that surface collapses to one
//! implementation per tool, consumed by both transports.
//!
//! Boundaries:
//!  * [`LedgerProvider`] is the only seam between the core and tenancy/auth.
//!    stdio implements it with a directory-based opener; HTTP implements it
//!    with a tenant-scoped resolver derived from validated WorkOS claims.
//!    The core never resolves tenancy or checks auth itself.
//!  * [`CoreError`] is the only error type the core produces. It carries no
//!    wire code — mapping it to a JSON-RPC error object (stdio) or an
//!    `(i32, String)` pair (HTTP) is entirely the adapter's job.
//!  * [`ToolOutput`] is the transport-neutral success payload. Rendering it
//!    to the MCP `content`/`structuredContent` wire shape is unchanged and
//!    stays with each adapter's existing renderer.
//!
//! Migrated so far: `capture_decision`. Later tools follow the same shape —
//! an `Args::from_json` parser plus a `core::<tool>` function — one pair per
//! tool, each independently reviewable.

use serde_json::{json, Map, Value};

use crate::commands::{CommandContext, Commands, DecisionProposalInput};
use crate::error::{CliError, CommandError, HivemindError};
use crate::events::{EventProvenance, TenantId};
use crate::ledger::EventLedger;

use super::args::{
    default_option_description, optional_string, optional_string_array, require_string,
    require_string_array,
};

// ---------------------------------------------------------------------------
// Error shape
// ---------------------------------------------------------------------------

/// Transport-neutral error from a tool core.
///
/// No JSON-RPC code and no HTTP status live here on purpose — those are wire
/// concerns, and each transport already has its own error envelope. Adapters
/// map each variant to their own wire shape.
///
/// Only the two variants `capture_decision` actually produces are here.
/// Tools that migrate next (e.g. `get_decision`'s not-found, `supersede`'s
/// conflicting concurrent update) add their own variant when they need one,
/// rather than this bead pre-declaring cases nothing constructs yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoreError {
    /// Caller-supplied arguments failed validation.
    InvalidArgument(String),
    /// Ledger I/O or any other internal failure.
    Internal(String),
}

impl CoreError {
    pub(crate) fn into_message(self) -> String {
        match self {
            CoreError::InvalidArgument(m) | CoreError::Internal(m) => m,
        }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::InvalidArgument(m) | CoreError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<HivemindError> for CoreError {
    fn from(error: HivemindError) -> Self {
        match &error {
            HivemindError::Command(CommandError::Validation(_))
            | HivemindError::Cli(CliError::InvalidInput(_)) => {
                CoreError::InvalidArgument(error.to_string())
            }
            _ => CoreError::Internal(error.to_string()),
        }
    }
}

/// The shared JSON-RPC arg helpers in `args.rs` return the raw `(code,
/// message)` pair. Every error they raise is a validation failure, so the
/// conversion is unconditional — this lets `?` bridge straight from those
/// helpers into `CoreError` without per-call-site `map_err`.
impl From<(i32, String)> for CoreError {
    fn from((_, message): (i32, String)) -> Self {
        CoreError::InvalidArgument(message)
    }
}

// ---------------------------------------------------------------------------
// Ledger acquisition seam
// ---------------------------------------------------------------------------

/// A ledger already scoped to the right tenant, handed to the core by its
/// transport. The core only ever sees the result of tenant resolution, never
/// how it happened.
pub(crate) struct LedgerHandle<L: EventLedger> {
    pub(crate) ledger: L,
    pub(crate) tenant_id: TenantId,
}

/// Transport-specific ledger acquisition, injected into the core.
///
/// stdio implements this with a directory-based opener; HTTP implements it
/// with a tenant-scoped resolver derived from validated WorkOS claims. Auth
/// stays entirely on the HTTP side of that boundary — this trait is called
/// only after the adapter has already authenticated the caller.
pub(crate) trait LedgerProvider {
    type Ledger: EventLedger;

    fn ledger(&self) -> Result<LedgerHandle<Self::Ledger>, CoreError>;
}

// ---------------------------------------------------------------------------
// Success payload
// ---------------------------------------------------------------------------

/// Transport-neutral payload from a successful core call. Each transport
/// renders this into its own `content`/`structuredContent` wire shape
/// exactly as it did for the plain `Value` payloads before migration.
#[derive(Debug, Clone)]
pub(crate) struct ToolOutput(pub(crate) Value);

impl ToolOutput {
    pub(crate) fn into_value(self) -> Value {
        self.0
    }
}

// ---------------------------------------------------------------------------
// capture_decision
// ---------------------------------------------------------------------------

/// One parsed, validated option from `capture_decision`'s `options` array.
pub(crate) struct CaptureOptionArg {
    pub(crate) label: String,
    pub(crate) description: String,
}

/// Parsed, validated arguments for the `capture_decision` tool. Shared by
/// both transports so argument-shape validation — and its error text — can
/// never drift between them again (it already had once: stdio and HTTP
/// disagreed on the empty-label message before this module existed).
pub(crate) struct CaptureDecisionArgs {
    pub(crate) actor_id: String,
    pub(crate) title: String,
    pub(crate) rationale: String,
    pub(crate) topic_keys: Vec<String>,
    pub(crate) options: Vec<CaptureOptionArg>,
    pub(crate) chosen_option_label: Option<String>,
    pub(crate) hypothesis_ids: Vec<String>,
    pub(crate) evidence_ids: Vec<String>,
}

impl CaptureDecisionArgs {
    pub(crate) fn from_json(
        args: &Map<String, Value>,
        actor_id: String,
    ) -> Result<Self, CoreError> {
        let title = require_string(args, "title")?;
        let rationale = require_string(args, "rationale")?;
        let topic_keys = require_string_array(args, "topic_keys")?;
        if topic_keys.is_empty() {
            return Err(CoreError::InvalidArgument(
                "topic_keys must not be empty".to_owned(),
            ));
        }

        let options_value = args
            .get("options")
            .cloned()
            .ok_or_else(|| CoreError::InvalidArgument("missing `options`".to_owned()))?;
        let options_array = match options_value {
            Value::Array(items) => items,
            _ => {
                return Err(CoreError::InvalidArgument(
                    "`options` must be an array".to_owned(),
                ))
            }
        };
        if options_array.is_empty() {
            return Err(CoreError::InvalidArgument(
                "options must not be empty".to_owned(),
            ));
        }

        let mut options = Vec::with_capacity(options_array.len());
        for (index, option) in options_array.into_iter().enumerate() {
            let option_obj = match option {
                Value::Object(map) => map,
                _ => {
                    return Err(CoreError::InvalidArgument(format!(
                        "options[{index}] must be an object"
                    )))
                }
            };
            let label = option_obj
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CoreError::InvalidArgument(format!(
                        "options[{index}].label must be a non-empty string"
                    ))
                })?
                .to_owned();
            let description = option_obj
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| default_option_description(&label));
            options.push(CaptureOptionArg { label, description });
        }

        let chosen_option_label = optional_string(args, "chosen_option_label")?;
        let hypothesis_ids = optional_string_array(args, "hypothesis_ids")?;
        let evidence_ids = optional_string_array(args, "evidence_ids")?;

        Ok(Self {
            actor_id,
            title,
            rationale,
            topic_keys,
            options,
            chosen_option_label,
            hypothesis_ids,
            evidence_ids,
        })
    }
}

/// The migrated core for the `capture_decision` MCP tool: one implementation
/// consumed by both transports.
pub(crate) fn capture_decision<P: LedgerProvider>(
    provider: &P,
    args: CaptureDecisionArgs,
) -> Result<ToolOutput, CoreError> {
    let handle = provider.ledger()?;
    let commands = Commands::new_with_context(
        &handle.ledger,
        CommandContext::new(
            handle.tenant_id,
            EventProvenance::agent(args.actor_id.clone()),
        ),
    );

    let mut option_ids: Vec<String> = Vec::with_capacity(args.options.len());
    let mut chosen_option_id: Option<String> = None;
    for option in &args.options {
        let option_id = commands
            .record_option(&args.actor_id, &option.label, &option.description)
            .map_err(CoreError::from)?;
        // ubs:ignore: == compares option labels (user-visible strings), not secrets
        if args.chosen_option_label.as_deref() == Some(option.label.as_str()) {
            chosen_option_id = Some(option_id.clone());
        }
        option_ids.push(option_id);
    }
    if args.chosen_option_label.is_some() && chosen_option_id.is_none() {
        return Err(CoreError::InvalidArgument(
            "chosen_option_label must match one of the supplied option labels".to_owned(),
        ));
    }

    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: &args.actor_id,
            title: &args.title,
            rationale: &args.rationale,
            topic_keys: &args.topic_keys,
            option_ids: &option_ids,
            chosen_option_id: chosen_option_id.as_deref(),
            hypothesis_ids: &args.hypothesis_ids,
            evidence_ids: &args.evidence_ids,
        })
        .map_err(CoreError::from)?;

    Ok(ToolOutput(json!({
        "decision_id": decision_id,
        "option_ids": option_ids,
        "chosen_option_id": chosen_option_id,
    })))
}
