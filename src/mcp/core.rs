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
//! Migrated so far: `capture_decision`, `get_situational_decisions`,
//! `resolve_target`/`get_decision_neighborhood`, `recall_decisions`,
//! `supersede_decision`, `get_decision_outcome`. Later tools follow the
//! same shape — an `Args::from_json` parser plus a `core::<tool>`
//! function — one pair per tool, each independently reviewable.

use serde_json::{json, Map, Value};

use crate::commands::{CommandContext, Commands, DecisionProposalInput, SupersedeInput};
use crate::error::{CliError, CommandError, HivemindError};
use crate::events::{EventProvenance, TenantId};
use crate::ledger::{AnyLedger, EventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant, GraphView};
use crate::queries::{
    derive_decision_status, get_decision_brief as query_get_decision_brief,
    get_decision_neighborhood as query_get_decision_neighborhood, resolve_decision_by_description,
    DecisionStatus, NeighborhoodRequest, QueryContext, QueryResponse, ResolveOutcome,
    SituationalRequest,
};
use crate::summarize::{RecallRequest, RECALL_DEFAULT_LIMIT, RECALL_MAX_LIMIT};

use super::args::{
    default_option_description, optional_datetime, optional_option_labels, optional_string,
    optional_string_array, optional_usize, require_string, require_string_array,
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
/// `capture_decision` produced only the first variant; per Alex's
/// 2026-09-20 call, a free-text description that matches nothing is a
/// success outcome (`{outcome: "not_found"}`, see [`ResolvedTarget::NotFound`]),
/// not an error, so `resolve_target` never needed its own `CoreError`
/// variant. whvd.1's note that later tools add their own variant when they
/// need one still stands — none has, yet.
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
            CoreError::InvalidArgument(m) | CoreError::Internal(m) => {
                write!(f, "{m}")
            }
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

// ---------------------------------------------------------------------------
// get_situational_decisions
// ---------------------------------------------------------------------------

/// Parsed, validated arguments for the `get_situational_decisions` tool. No
/// resolver involved: situational takes paths, not a free-text description,
/// so there is no id/description ambiguity to settle before calling the
/// query layer.
pub(crate) struct GetSituationalDecisionsArgs {
    pub(crate) paths: Vec<String>,
    pub(crate) since_offset: Option<u64>,
    pub(crate) since_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
}

impl GetSituationalDecisionsArgs {
    pub(crate) fn from_json(args: &Map<String, Value>) -> Result<Self, CoreError> {
        let paths = require_string_array(args, "paths")?;
        let since_offset = optional_usize(args, "since_offset")?.map(|value| value as u64);
        let since_timestamp = optional_datetime(args, "since_timestamp")?;
        let limit = optional_usize(args, "limit")?.unwrap_or(0);
        let cursor = optional_string(args, "cursor")?;
        Ok(Self {
            paths,
            since_offset,
            since_timestamp,
            limit,
            cursor,
        })
    }
}

// ---------------------------------------------------------------------------
// resolve_target: the shared fluent (no-id) target resolver
// ---------------------------------------------------------------------------
//
// The CLI's equivalent (`resolve_fluent_target`, `src/cli/run/mod.rs`) supports
// `--pick`/`#N` because it has a local, single-user session to cache candidates
// in. MCP has neither: stdio is stateless per call and HTTP has no session
// directory, so there is no disambiguation shortcut here — an ambiguous or
// not-found description is returned to the caller as a success envelope, and
// the caller re-calls with `decision_id` once it knows one.

/// Shared "one of `<selector_field>` or `description` is required" message.
/// Both transports raise this exact text so they can never drift on it, the
/// same discipline `CaptureDecisionArgs` already applies to its own
/// validation. `selector_field` is the caller's id parameter name —
/// `decision_id` for `get_decision_neighborhood`, `old_decision_id` for
/// `supersede_decision` — so the message always names the field the caller
/// actually sent.
fn missing_selector_message(selector_field: &str) -> String {
    format!("one of `{selector_field}` or `description` is required")
}

/// Outcome of resolving a fluent MCP tool's target.
pub(crate) enum ResolvedTarget {
    /// A concrete decision id to proceed with — either the caller supplied it
    /// directly, or resolution found exactly one best-tier match.
    Id(String),
    /// Resolution matched more than one decision at the best tier. This is a
    /// successful outcome (not an error): the caller returns it as-is.
    Ambiguous(ToolOutput),
    /// The description matched nothing. This is also a successful outcome —
    /// `{outcome: "not_found"}` — not an error, per Alex's 2026-09-20 call:
    /// MCP has no session to retry a bare id against, but the caller can
    /// still branch on `data.outcome` without needing a JSON-RPC error path.
    NotFound(ToolOutput),
}

/// Resolve a fluent tool's target: `id` bypasses resolution entirely
/// (unvalidated, matching the CLI's `--id` escape hatch byte-for-byte);
/// otherwise `description` is resolved via [`resolve_decision_by_description`]
/// over a graph rebuilt from `handle`.
///
/// Both ambiguity and not-found are success, rendered as the identical
/// `{result_count, truncated, latency_ms, data: {outcome: ..., ...}}`
/// envelope the CLI's `--json` output uses for the same outcomes (all three
/// resolutions serialize the same `QueryResponse<ResolveOutcome>`).
pub(crate) fn resolve_target<L: EventLedger>(
    handle: &LedgerHandle<L>,
    id: Option<&str>,
    description: Option<&str>,
    topic: Option<&str>,
    selector_field: &str,
) -> Result<ResolvedTarget, CoreError> {
    if let Some(id) = id {
        return Ok(ResolvedTarget::Id(id.to_owned()));
    }

    let Some(description) = description.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(CoreError::InvalidArgument(missing_selector_message(
            selector_field,
        )));
    };

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&handle.ledger, &handle.tenant_id, &graph).map_err(CoreError::from)?;
    let response =
        resolve_decision_by_description(&graph, description, topic).map_err(CoreError::from)?;

    let QueryResponse {
        result_count,
        truncated,
        latency_ms,
        data,
    } = response;
    match data {
        ResolveOutcome::Resolved { candidate } => Ok(ResolvedTarget::Id(candidate.decision_id)),
        ResolveOutcome::NotFound => Ok(ResolvedTarget::NotFound(ToolOutput(json!({
            "result_count": result_count,
            "truncated": truncated,
            "latency_ms": latency_ms,
            "data": ResolveOutcome::NotFound,
        })))),
        ResolveOutcome::Ambiguous { candidates } => {
            let data = ResolveOutcome::Ambiguous { candidates };
            Ok(ResolvedTarget::Ambiguous(ToolOutput(json!({
                "result_count": result_count,
                "truncated": truncated,
                "latency_ms": latency_ms,
                "data": data,
            }))))
        }
    }
}

// ---------------------------------------------------------------------------
// get_decision_neighborhood (the CLI's `why`)
// ---------------------------------------------------------------------------

/// Parsed, validated arguments for the `get_decision_neighborhood` tool.
pub(crate) struct GetDecisionNeighborhoodArgs {
    pub(crate) decision_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) topic: Option<String>,
}

impl GetDecisionNeighborhoodArgs {
    pub(crate) fn from_json(args: &Map<String, Value>) -> Result<Self, CoreError> {
        Ok(Self {
            decision_id: optional_string(args, "decision_id")?,
            description: optional_string(args, "description")?,
            topic: optional_string(args, "topic")?,
        })
    }
}

/// The migrated core for the `get_situational_decisions` MCP tool: one
/// implementation consumed by both transports.
///
/// `graph` is supplied by the caller rather than resolved via
/// [`LedgerProvider`]: stdio rebuilds a fresh [`crate::projector::memory::MemoryGraph`]
/// per call, while HTTP reuses its per-tenant graph cache. That is a caching
/// decision, not a tenancy one, so it stays outside the provider seam —
/// [`LedgerProvider`] here is only used for the ledger the query needs to
/// resolve `since_offset`/`since_timestamp` change annotations.
pub(crate) fn get_situational_decisions<P: LedgerProvider>(
    provider: &P,
    graph: &impl GraphView,
    args: GetSituationalDecisionsArgs,
) -> Result<ToolOutput, CoreError> {
    let handle = provider.ledger()?;
    let context = QueryContext::new(handle.tenant_id);
    let request = SituationalRequest {
        paths: args.paths,
        since_offset: args.since_offset,
        since_timestamp: args.since_timestamp,
        limit: args.limit,
        cursor: args.cursor,
    };
    let response =
        crate::queries::get_situational_decisions(&context, graph, &handle.ledger, &request)
            .map_err(CoreError::from)?;
    Ok(ToolOutput(json!({
        "result_count": response.result_count,
        "truncated": response.truncated,
        "latency_ms": response.latency_ms,
        "data": response.data,
    })))
}

/// The migrated core for the `get_decision_neighborhood` MCP tool (the CLI's
/// `why`): one implementation consumed by both transports. Payload is
/// identical to `hivemind query why --json` — the same `QueryResponse`
/// envelope around [`crate::queries::NeighborhoodView`], always at the
/// CLI default (depth 1, all relations, non-compact); this bead does not
/// expose those filters over MCP.
pub(crate) fn get_decision_neighborhood<P: LedgerProvider>(
    provider: &P,
    args: GetDecisionNeighborhoodArgs,
) -> Result<ToolOutput, CoreError> {
    let handle = provider.ledger()?;
    let target = resolve_target(
        &handle,
        args.decision_id.as_deref(),
        args.description.as_deref(),
        args.topic.as_deref(),
        "decision_id",
    )?;
    let decision_id = match target {
        ResolvedTarget::Id(id) => id,
        ResolvedTarget::Ambiguous(output) => return Ok(output),
        ResolvedTarget::NotFound(output) => return Ok(output),
    };

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&handle.ledger, &handle.tenant_id, &graph).map_err(CoreError::from)?;
    let response =
        query_get_decision_neighborhood(&graph, &decision_id, &NeighborhoodRequest::all())
            .map_err(CoreError::from)?;

    Ok(ToolOutput(json!({
        "result_count": response.result_count,
        "truncated": response.truncated,
        "latency_ms": response.latency_ms,
        "data": response.data,
    })))
}

// ---------------------------------------------------------------------------
// recall_decisions
// ---------------------------------------------------------------------------

fn parse_decision_status(value: &str) -> Result<DecisionStatus, CoreError> {
    match value {
        "proposed" => Ok(DecisionStatus::Proposed),
        "accepted" => Ok(DecisionStatus::Accepted),
        "rejected" => Ok(DecisionStatus::Rejected),
        "contested" => Ok(DecisionStatus::Contested),
        "superseded" => Ok(DecisionStatus::Superseded),
        other => Err(CoreError::InvalidArgument(format!(
            "unknown status `{other}`"
        ))),
    }
}

/// Parsed, validated arguments for the `recall_decisions` tool. No resolver
/// involved: recall takes a free-text search query and filters, not an id.
pub(crate) struct RecallDecisionsArgs {
    pub(crate) q: Option<String>,
    pub(crate) topic_keys: Vec<String>,
    pub(crate) statuses: Vec<DecisionStatus>,
    pub(crate) actor_ids: Vec<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) since: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) until: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
}

impl RecallDecisionsArgs {
    pub(crate) fn from_json(args: &Map<String, Value>) -> Result<Self, CoreError> {
        let statuses = optional_string_array(args, "status")?
            .iter()
            .map(|status| parse_decision_status(status))
            .collect::<Result<Vec<_>, _>>()?;
        let limit = optional_usize(args, "limit")?.unwrap_or(RECALL_DEFAULT_LIMIT);
        if limit > RECALL_MAX_LIMIT {
            return Err(CoreError::InvalidArgument(format!(
                "limit must be at most {RECALL_MAX_LIMIT}"
            )));
        }
        Ok(Self {
            q: optional_string(args, "q")?,
            topic_keys: optional_string_array(args, "topic")?,
            statuses,
            actor_ids: optional_string_array(args, "actor_id")?,
            sources: optional_string_array(args, "source")?,
            since: optional_datetime(args, "since")?,
            until: optional_datetime(args, "until")?,
            limit,
            cursor: optional_string(args, "cursor")?,
        })
    }
}

/// The migrated core for the `recall_decisions` MCP tool: one implementation
/// consumed by both transports.
///
/// `graph` is supplied by the caller, same caching split as
/// [`get_situational_decisions`]. Unlike that tool, the ledger here must be
/// [`AnyLedger`] specifically rather than any `P::Ledger: EventLedger`:
/// `recall_decisions` calls `search_decisions_any` internally, which
/// dispatches on the concrete backend (SQLite FTS5 vs Postgres portable
/// search) and so needs the concrete enum, not just the trait.
pub(crate) fn recall_decisions<P>(
    provider: &P,
    graph: &impl GraphView,
    args: RecallDecisionsArgs,
) -> Result<ToolOutput, CoreError>
where
    P: LedgerProvider<Ledger = AnyLedger>,
{
    let handle = provider.ledger()?;
    let context = QueryContext::new(handle.tenant_id);
    let request = RecallRequest {
        q: args.q,
        topic_keys: args.topic_keys,
        statuses: args.statuses,
        actor_ids: args.actor_ids,
        sources: args.sources,
        since: args.since,
        until: args.until,
        limit: args.limit,
        cursor: args.cursor,
    };
    let response = crate::summarize::recall_decisions(&context, &handle.ledger, graph, &request)
        .map_err(CoreError::from)?;
    Ok(ToolOutput(json!({
        "result_count": response.result_count,
        "truncated": response.truncated,
        "latency_ms": response.latency_ms,
        "data": response.data,
    })))
}

// ---------------------------------------------------------------------------
// get_decision_outcome (the CLI's `verify`)
// ---------------------------------------------------------------------------

/// Parsed, validated arguments for the `get_decision_outcome` tool.
pub(crate) struct GetDecisionOutcomeArgs {
    pub(crate) decision_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) topic: Option<String>,
}

impl GetDecisionOutcomeArgs {
    pub(crate) fn from_json(args: &Map<String, Value>) -> Result<Self, CoreError> {
        Ok(Self {
            decision_id: optional_string(args, "decision_id")?,
            description: optional_string(args, "description")?,
            topic: optional_string(args, "topic")?,
        })
    }
}

/// The migrated core for the `get_decision_outcome` MCP tool (the CLI's
/// `verify`): one implementation consumed by both transports. Payload is
/// `DecisionBrief` — the same envelope `hivemind query verify --json`
/// (`get_decision_brief`, composing `get_decision` + `get_decision_context` +
/// `get_decision_outcome`) returns, not the narrower outcome-signal-only
/// payload this tool returned before this migration — see hivemind-ot72.8
/// ("match the CLI").
pub(crate) fn get_decision_outcome<P: LedgerProvider>(
    provider: &P,
    args: GetDecisionOutcomeArgs,
) -> Result<ToolOutput, CoreError> {
    let handle = provider.ledger()?;
    let target = resolve_target(
        &handle,
        args.decision_id.as_deref(),
        args.description.as_deref(),
        args.topic.as_deref(),
        "decision_id",
    )?;
    let decision_id = match target {
        ResolvedTarget::Id(id) => id,
        ResolvedTarget::Ambiguous(output) => return Ok(output),
        ResolvedTarget::NotFound(output) => return Ok(output),
    };

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&handle.ledger, &handle.tenant_id, &graph).map_err(CoreError::from)?;
    let response = query_get_decision_brief(&graph, &decision_id).map_err(CoreError::from)?;

    Ok(ToolOutput(json!({
        "result_count": response.result_count,
        "truncated": response.truncated,
        "latency_ms": response.latency_ms,
        "data": response.data,
    })))
}

// ---------------------------------------------------------------------------
// supersede_decision
// ---------------------------------------------------------------------------

/// Parsed, validated arguments for the `supersede_decision` tool. The old
/// decision is selected the same way `get_decision_neighborhood` selects its
/// target: `old_decision_id` bypasses resolution, otherwise `description`
/// (+ optional `topic`) resolves via [`resolve_target`].
pub(crate) struct SupersedeDecisionArgs {
    pub(crate) actor_id: String,
    pub(crate) old_decision_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) topic: Option<String>,
    pub(crate) title: String,
    pub(crate) rationale: String,
    pub(crate) topic_keys: Vec<String>,
    pub(crate) option_labels: Vec<String>,
    pub(crate) chosen_option_label: Option<String>,
    pub(crate) hypothesis_ids: Vec<String>,
    pub(crate) evidence_ids: Vec<String>,
}

impl SupersedeDecisionArgs {
    pub(crate) fn from_json(
        args: &Map<String, Value>,
        actor_id: String,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            actor_id,
            old_decision_id: optional_string(args, "old_decision_id")?,
            description: optional_string(args, "description")?,
            topic: optional_string(args, "topic")?,
            title: require_string(args, "title")?,
            rationale: require_string(args, "rationale")?,
            topic_keys: optional_string_array(args, "topic_keys")?,
            option_labels: optional_option_labels(args, "options")?,
            chosen_option_label: optional_string(args, "chosen_option_label")?,
            hypothesis_ids: optional_string_array(args, "hypothesis_ids")?,
            evidence_ids: optional_string_array(args, "evidence_ids")?,
        })
    }
}

/// The migrated core for the `supersede_decision` MCP tool: one
/// implementation consumed by both transports. The old decision is resolved
/// exactly like [`get_decision_neighborhood`]'s target via [`resolve_target`];
/// an ambiguous or not-found resolution is returned as-is, with
/// `commands.supersede` never called — no ledger write happens for either
/// outcome, matching Alex's 2026-09-20 fluent-resolution call for the rest of
/// this tool surface.
pub(crate) fn supersede_decision<P: LedgerProvider>(
    provider: &P,
    args: SupersedeDecisionArgs,
) -> Result<ToolOutput, CoreError> {
    let handle = provider.ledger()?;
    let target = resolve_target(
        &handle,
        args.old_decision_id.as_deref(),
        args.description.as_deref(),
        args.topic.as_deref(),
        "old_decision_id",
    )?;
    let old_decision_id = match target {
        ResolvedTarget::Id(id) => id,
        ResolvedTarget::Ambiguous(output) => return Ok(output),
        ResolvedTarget::NotFound(output) => return Ok(output),
    };

    let commands = Commands::new_with_context(
        &handle.ledger,
        CommandContext::new(
            handle.tenant_id.clone(),
            EventProvenance::agent(args.actor_id.clone()),
        ),
    );
    let outcome = commands
        .supersede(SupersedeInput {
            actor_id: &args.actor_id,
            old_decision_id: &old_decision_id,
            new_title: &args.title,
            new_rationale: &args.rationale,
            topic_keys: &args.topic_keys,
            option_labels: &args.option_labels,
            chosen_option_label: args.chosen_option_label.as_deref(),
            hypothesis_ids: &args.hypothesis_ids,
            evidence_ids: &args.evidence_ids,
        })
        .map_err(CoreError::from)?;

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&handle.ledger, &handle.tenant_id, &graph).map_err(CoreError::from)?;
    let old_decision_status =
        derive_decision_status(&graph, &old_decision_id).map_err(CoreError::from)?;
    let new_decision_status =
        derive_decision_status(&graph, &outcome.new_decision_id).map_err(CoreError::from)?;

    Ok(ToolOutput(json!({
        "old_decision_id": old_decision_id,
        "new_decision_id": outcome.new_decision_id,
        "proposal_event_id": outcome.proposal_event_id,
        "relation_event_ids": outcome.relation_event_ids,
        "superseded_event_id": outcome.superseded_event_id,
        "old_decision_status": old_decision_status,
        "new_decision_status": new_decision_status,
    })))
}
