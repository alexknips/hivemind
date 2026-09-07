use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::events::BlockerPriority;
use crate::identity::default_actor;
use crate::ingest::{
    DocumentConflictResolutionAction, DocumentImportFormat, DocumentPreparationFormat,
    DEFAULT_SLACK_MENTION,
};
use crate::projector::RelationKind as GraphRelationKind;
use crate::queries::{DecisionStatus, ReadOnlyExportFormat as QueryReadOnlyExportFormat};
use crate::slack_app::SlackCaptureSurface;
use crate::summarize::DIGEST_MAX_DECISIONS;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "hivemind",
    about = "Organizational decision-memory ledger and query CLI",
    version,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[arg(long, default_value_t = default_actor())]
    pub actor: String,

    #[arg(long, global = true, env = "HIVEMIND_TENANT", default_value = "local")]
    pub tenant: String,

    #[arg(long, global = true)]
    pub json: bool,

    #[arg(
        long,
        global = true,
        env = "HIVEMIND_DIR",
        default_value = "./hivemind/"
    )]
    pub hivemind_dir: PathBuf,

    #[arg(long, global = true, value_enum)]
    pub graph_backend: Option<GraphBackend>,

    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GraphBackend {
    Memory,
    Kuzu,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Capture and query a first decision on an isolated temporary ledger.
    Quickstart(QuickstartArgs),
    Emit(Box<EmitArgs>),
    Disagree(DisagreeArgs),
    Supersede(SupersedeArgs),
    Review(ReviewArgs),
    Import(ImportArgs),
    /// Run deterministic read queries. JSON is the default; pass --summary for compact text.
    Query(Box<QueryArgs>),
    Dump(DumpArgs),
    Tui(TuiArgs),
    Ingest(IngestArgs),
    #[command(name = "slack-app")]
    SlackApp(SlackAppArgs),
    /// Run an MCP (Model Context Protocol) stdio server that exposes
    /// HiveMind's capture/query surface to MCP-aware clients.
    Mcp(McpArgs),
    /// Start the HTTP REST API server. Auth token is read from
    /// HIVEMIND_API_KEY; when unset the server starts in development mode
    /// with no authentication.
    Serve(ServeArgs),
    /// Migrate an existing local SQLite ledger to a remote Postgres deployment.
    /// Replays all events from the SQLite source into the named Postgres tenant,
    /// preserving event_uuid for idempotency. Requires the
    /// `shared-backend-postgres` feature.
    #[cfg(feature = "shared-backend-postgres")]
    Migrate(MigrateArgs),
    /// Compute the 2-D spectral decision map (x=time, y=semantic embedding).
    /// Outputs a JSON point-set to stdout. Use --alpha to blend semantic and
    /// structural (supersession) similarity. Outputs JSON unless --summary is
    /// passed.
    Map(MapArgs),
    /// Generate a textual decision digest for a time window.
    /// Answers "what did the team decide this week and why?" using graph data.
    /// Outputs structured JSON by default; pass --summary for readable prose.
    Digest(Box<DigestArgs>),
    /// Inspect and drain the classification work queue (Worker A).
    /// Use `classify-queue list` to see pending batches; use `classify-queue submit`
    /// to write structured captures produced by the agent on its subscription seat.
    #[command(name = "classify-queue")]
    ClassifyQueue(ClassifyQueueArgs),
    /// Manage connector authentication (e.g., Google Docs OAuth).
    /// Set HIVEMIND_GOOGLE_CLIENT_ID and HIVEMIND_GOOGLE_CLIENT_SECRET before running.
    Connector(ConnectorArgs),
    /// Scan recent decisions for quality concerns and file Linear tickets for human review.
    /// Precision-biased: only files tickets on strong signals (high_concern tier by default).
    /// Set HIVEMIND_LINEAR_API_KEY and HIVEMIND_LINEAR_TEAM_ID before running.
    /// Pass --dry-run to preview what would be filed without calling Linear.
    #[command(name = "quality-scan")]
    QualityScan(QualityScanArgs),
}

#[derive(Debug, Clone, Args)]
pub struct QuickstartArgs {}

#[derive(Debug, Clone, Args)]
pub struct ConnectorArgs {
    #[command(subcommand)]
    pub command: ConnectorCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConnectorCommand {
    /// Authenticate with a connector. Opens a browser for OAuth consent.
    #[command(name = "auth")]
    Auth(ConnectorAuthArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ConnectorAuthArgs {
    /// Connector to authenticate. Currently supported: gdocs (Google Docs).
    pub connector: String,
}

#[derive(Debug, Clone, Args)]
pub struct QualityScanArgs {
    /// Minimum quality tier to flag. Defaults to high_concern (precision-biased).
    #[arg(long, value_enum, default_value = "high_concern")]
    pub min_tier: QueryQualityTier,

    /// Maximum decisions to file tickets for per run (1–50). Prevents flooding Linear.
    #[arg(long, default_value_t = 10)]
    pub limit: usize,

    /// Minimum ledger event offset (inclusive). Use to restrict the scan to recent decisions.
    #[arg(long)]
    pub since_event_origin: Option<i64>,

    /// Preview mode: show what would be filed without calling the Linear API.
    #[arg(long)]
    pub dry_run: bool,

    /// Linear team ID to file tickets under. Overrides HIVEMIND_LINEAR_TEAM_ID.
    #[arg(long, env = "HIVEMIND_LINEAR_TEAM_ID")]
    pub linear_team_id: Option<String>,

    /// Public base URL of this HiveMind instance, included in ticket descriptions.
    /// E.g. https://hivemind.example.com. Overrides HIVEMIND_PUBLIC_URL.
    #[arg(long, env = "HIVEMIND_PUBLIC_URL")]
    pub hivemind_base_url: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Port to listen on.
    #[arg(long, short = 'p', env = "HIVEMIND_PORT", default_value_t = 8080)]
    pub port: u16,
}

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Clone, Args)]
pub struct MigrateArgs {
    /// Source SQLite directory (strips `sqlite://` prefix if present).
    /// Defaults to `--hivemind-dir` when omitted.
    #[arg(long)]
    pub from: Option<String>,

    /// Destination Postgres connection URL (e.g. `postgres://user:pass@host/db`).
    #[arg(long)]
    pub to: String,

    /// Tenant name to write events under in the Postgres destination.
    #[arg(long = "to-tenant")]
    pub to_tenant: String,

    /// Count events that would be migrated without writing to Postgres.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args)]
pub struct MapArgs {
    /// Blend weight between pure-semantic (0.0) and structural-supersession (1.0).
    /// Values between 0 and 1 blend both signals. Use 0.0,0.5 to output both.
    #[arg(long, default_value = "0.5")]
    pub alpha: Vec<f64>,

    /// Output compact text summary instead of JSON.
    #[arg(long)]
    pub summary: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DigestArgs {
    /// Time window as a duration string: Nd (days), Nh (hours), Nw (weeks).
    /// Defaults to "7d" (the past 7 days from now).
    #[arg(long, default_value = "7d")]
    pub window: String,

    /// Explicit window start (ISO 8601 / RFC 3339). Overrides --window.
    #[arg(long)]
    pub since: Option<String>,

    /// Explicit window end (ISO 8601 / RFC 3339). Defaults to now.
    #[arg(long)]
    pub until: Option<String>,

    /// Filter to decisions involving these actor IDs (repeatable, comma-separated).
    #[arg(long = "actor", value_delimiter = ',')]
    pub actor_ids: Vec<String>,

    /// Maximum number of decisions to include (1–50, default 50).
    #[arg(long, default_value_t = DIGEST_MAX_DECISIONS)]
    pub limit: usize,

    /// Output readable prose instead of JSON.
    #[arg(long)]
    pub summary: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ClassifyQueueArgs {
    #[command(subcommand)]
    pub command: ClassifyQueueCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ClassifyQueueCommand {
    /// List batches pending classification (received but not yet classified).
    List(ClassifyQueueListArgs),
    /// Submit agent-produced captures for a batch, appending an IngestBatchClassified event.
    Submit(ClassifyQueueSubmitArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ClassifyQueueListArgs {
    /// Maximum number of pending batches to return.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Clone, Args)]
pub struct ClassifyQueueSubmitArgs {
    /// Batch ID to classify (from `classify-queue list` output).
    #[arg(long = "batch-id")]
    pub batch_id: String,

    /// Structured captures as a JSON array of CaptureItem objects.
    #[arg(long)]
    pub captures: String,

    /// Classifier identifier recorded in the event.
    /// Defaults to "agent:worker-a" to indicate subscription-seat classification.
    #[arg(long, default_value = "agent:worker-a")]
    pub model: String,
}

#[derive(Debug, Clone, Args)]
pub struct McpArgs {
    /// Override the session identifier embedded in event provenance for
    /// captures coming through this server. Defaults to a generated id.
    #[arg(long)]
    pub session_id: Option<String>,

    /// Agent tool name used when MCP write calls omit actor_id.
    #[arg(long = "agent-tool")]
    pub agent_tool: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct DisagreeArgs {
    /// Free-text description to resolve to a decision (fluent alternative to --decision).
    /// A bare `#N` refers to candidate N from the previous ambiguous resolver output.
    pub description: Option<String>,

    #[arg(long = "decision")]
    pub decision_id: Option<String>,

    /// Select candidate N when a description resolves ambiguously.
    #[arg(long = "pick")]
    pub pick: Option<usize>,

    /// Narrow resolution to decisions carrying this topic key.
    #[arg(long = "topic")]
    pub topic: Option<String>,

    #[arg(long)]
    pub reason: String,
}

#[derive(Debug, Clone, Args)]
pub struct SupersedeArgs {
    /// Free-text description to resolve to the decision being superseded (fluent alternative to
    /// --old). A bare `#N` refers to candidate N from the previous ambiguous resolver output.
    pub description: Option<String>,

    #[arg(long = "old")]
    pub old_decision_id: Option<String>,

    /// Select candidate N when a description resolves ambiguously.
    #[arg(long = "pick")]
    pub pick: Option<usize>,

    /// Narrow resolution to decisions carrying this topic key.
    #[arg(long = "topic")]
    pub topic: Option<String>,

    #[arg(long)]
    pub title: String,

    #[arg(long)]
    pub rationale: String,

    #[arg(long = "topic-keys", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "options", value_delimiter = ',')]
    pub option_labels: Vec<String>,

    #[arg(long = "chose")]
    pub chosen_option_label: Option<String>,

    #[arg(long = "hypotheses", value_delimiter = ',')]
    pub hypothesis_ids: Vec<String>,

    #[arg(long = "evidence", value_delimiter = ',')]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ReviewArgs {
    /// Glob pattern for decision actor ids to review, for example agent:*.
    #[arg(long = "actor", value_delimiter = ',')]
    pub actor_patterns: Vec<String>,

    #[arg(long = "since", default_value = "7d")]
    pub since: String,

    #[arg(long = "until")]
    pub until: Option<String>,

    #[arg(long = "timezone", default_value = "UTC")]
    pub timezone: String,

    #[arg(long = "now", hide = true)]
    pub now: Option<String>,

    #[arg(long = "unreviewed-only")]
    pub unreviewed_only: bool,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct IngestArgs {
    #[command(subcommand)]
    pub command: IngestCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum IngestCommand {
    #[command(name = "slack-thread")]
    SlackThread(IngestSlackThreadArgs),
}

#[derive(Debug, Clone, Args)]
pub struct IngestSlackThreadArgs {
    #[arg(long)]
    pub file: PathBuf,

    #[arg(long, default_value = DEFAULT_SLACK_MENTION)]
    pub mention: String,
}

#[derive(Debug, Clone, Args)]
pub struct SlackAppArgs {
    #[command(subcommand)]
    pub command: SlackAppCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SlackAppCommand {
    Manifest(SlackManifestArgs),
    #[command(name = "oauth-url")]
    OauthUrl(SlackOauthUrlArgs),
    Install(SlackInstallArgs),
    #[command(name = "enqueue-capture")]
    EnqueueCapture(SlackEnqueueCaptureArgs),
    Drain(SlackDrainArgs),
    Command(SlackCommandArgs),
}

#[derive(Debug, Clone, Args)]
pub struct SlackManifestArgs {
    #[arg(long = "request-url")]
    pub request_url: String,

    #[arg(long = "event-url")]
    pub event_url: Option<String>,

    #[arg(long = "redirect-url")]
    pub redirect_url: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SlackOauthUrlArgs {
    #[arg(long = "client-id")]
    pub client_id: String,

    #[arg(long = "redirect-uri")]
    pub redirect_uri: String,

    #[arg(long)]
    pub state: String,
}

#[derive(Debug, Clone, Args)]
pub struct SlackInstallArgs {
    #[arg(long = "team-id")]
    pub team_id: String,

    #[arg(long = "team-name")]
    pub team_name: String,

    #[arg(long = "bot-token")]
    pub bot_token: String,

    #[arg(long = "signing-secret")]
    pub signing_secret: String,

    #[arg(long = "hivemind-url", default_value = "http://127.0.0.1:8787")]
    pub hivemind_url: String,

    #[arg(long = "reaction-emoji", default_value = "hivemind")]
    pub reaction_emoji: String,

    #[arg(long = "actor-map")]
    pub actor_mappings: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SlackEnqueueCaptureArgs {
    #[arg(long = "team-id")]
    pub team_id: String,

    #[arg(long = "user-id")]
    pub user_id: String,

    #[arg(long = "channel-id")]
    pub channel_id: String,

    #[arg(long = "message-ts")]
    pub message_ts: String,

    #[arg(long = "thread-ts")]
    pub thread_ts: Option<String>,

    #[arg(long)]
    pub permalink: String,

    #[arg(long, value_enum)]
    pub surface: SlackCaptureSurfaceArg,

    #[arg(long = "reaction-emoji")]
    pub reaction_emoji: Option<String>,

    #[arg(long)]
    pub title: String,

    #[arg(long)]
    pub rationale: String,

    #[arg(long = "topic-keys", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "options", value_delimiter = ',')]
    pub option_labels: Vec<String>,

    #[arg(long = "chose")]
    pub chosen_option_label: Option<String>,

    #[arg(long = "thread-text", default_value = "")]
    pub thread_text: String,
}

#[derive(Debug, Clone, Args)]
pub struct SlackDrainArgs {}

#[derive(Debug, Clone, Args)]
pub struct SlackCommandArgs {
    #[arg(long = "team-id")]
    pub team_id: String,

    #[arg(long = "user-id")]
    pub user_id: String,

    #[arg(long)]
    pub text: String,

    #[arg(long, default_value_t = 5)]
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum SlackCaptureSurfaceArg {
    SlashCommand,
    MessageAction,
    Reaction,
}

impl SlackCaptureSurfaceArg {
    pub(crate) const fn as_slack_surface(self) -> SlackCaptureSurface {
        match self {
            SlackCaptureSurfaceArg::SlashCommand => SlackCaptureSurface::SlashCommand,
            SlackCaptureSurfaceArg::MessageAction => SlackCaptureSurface::MessageAction,
            SlackCaptureSurfaceArg::Reaction => SlackCaptureSurface::Reaction,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct EmitArgs {
    #[command(subcommand)]
    pub command: EmitCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum EmitCommand {
    #[command(name = "decision.capture")]
    DecisionCapture(EmitDecisionCaptureArgs),
    #[command(name = "decision.proposed")]
    DecisionProposed(EmitDecisionProposedArgs),
    #[command(name = "decision.accepted")]
    DecisionAccepted(EmitDecisionIdArgs),
    #[command(name = "decision.rejected")]
    DecisionRejected(EmitDecisionIdArgs),
    #[command(name = "decision.superseded")]
    DecisionSuperseded(EmitDecisionSupersededArgs),
    #[command(name = "evidence.recorded")]
    EvidenceRecorded(EmitEvidenceRecordedArgs),
    #[command(name = "hypothesis.recorded")]
    HypothesisRecorded(EmitHypothesisRecordedArgs),
    #[command(name = "option.recorded")]
    OptionRecorded(EmitOptionRecordedArgs),
    #[command(name = "relation.added")]
    RelationAdded(EmitRelationAddedArgs),
    #[command(name = "relation.attach_evidence")]
    AttachEvidence(EmitAttachEvidenceArgs),
    #[command(name = "ingest.batch_classified")]
    IngestBatchClassified(EmitIngestBatchClassifiedArgs),
    #[command(name = "decision.scored")]
    DecisionScored(EmitDecisionScoredArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DecisionCaptureSource {
    Agent,
    Human,
}

#[derive(Debug, Clone, Args)]
pub struct EmitDecisionCaptureArgs {
    #[command(flatten)]
    pub provenance: EmitCaptureProvenanceArgs,

    #[command(flatten)]
    pub decision: EmitDecisionProposedArgs,
}

#[derive(Debug, Clone, Args)]
pub struct EmitCaptureProvenanceArgs {
    #[arg(long = "source", value_enum)]
    pub source: Option<DecisionCaptureSource>,

    #[arg(long = "agent-tool")]
    pub agent_tool: Option<String>,

    #[arg(long = "agent-session")]
    pub agent_session: Option<String>,

    #[arg(long = "actor-id")]
    pub actor_id: Option<String>,

    #[arg(long = "source-ref")]
    pub source_ref: Option<String>,
}

impl EmitCaptureProvenanceArgs {
    pub(crate) fn has_override(&self) -> bool {
        self.source.is_some()
            || self.agent_tool.is_some()
            || self.agent_session.is_some()
            || self.actor_id.is_some()
            || self.source_ref.is_some()
    }
}

#[derive(Debug, Clone, Args)]
pub struct EmitDecisionProposedArgs {
    #[arg(long)]
    pub title: String,

    #[arg(long)]
    pub rationale: String,

    #[arg(long = "topic-keys", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "options", value_delimiter = ',')]
    pub option_ids: Vec<String>,

    #[arg(long = "chose")]
    pub chosen_option_id: Option<String>,

    #[arg(long = "hypotheses", value_delimiter = ',')]
    pub hypothesis_ids: Vec<String>,

    #[arg(long = "evidence", value_delimiter = ',')]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct EmitDecisionIdArgs {
    #[arg(long = "decision-id")]
    pub decision_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct EmitDecisionSupersededArgs {
    #[arg(long = "old")]
    pub old_decision_id: String,

    #[arg(long = "new")]
    pub new_decision_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct EmitEvidenceRecordedArgs {
    #[command(flatten)]
    pub provenance: EmitCaptureProvenanceArgs,

    #[arg(long)]
    pub content: String,
}

#[derive(Debug, Clone, Args)]
pub struct EmitHypothesisRecordedArgs {
    #[command(flatten)]
    pub provenance: EmitCaptureProvenanceArgs,

    #[arg(long)]
    pub statement: String,
}

#[derive(Debug, Clone, Args)]
pub struct EmitOptionRecordedArgs {
    #[arg(long)]
    pub label: String,

    #[arg(long)]
    pub description: String,
}

#[derive(Debug, Clone, Args)]
pub struct EmitAttachEvidenceArgs {
    #[arg(long = "decision-id")]
    pub decision_id: String,

    #[arg(long = "evidence-id")]
    pub evidence_id: String,
}

/// Submit a pre-classified capture batch from a plugin/edge session.
///
/// The captures JSON must be an array of CaptureItem objects matching the
/// schema produced by src/classifier.rs (the ingest.batch_classified contract).
/// This path writes IngestBatchClassified directly — no ANTHROPIC_API_KEY
/// needed. The server classifier skips this batch because no companion
/// IngestBatchReceived event exists for this batch_id.
#[derive(Debug, Clone, Args)]
pub struct EmitIngestBatchClassifiedArgs {
    /// Path to a JSON file containing the captures array (CaptureItem[]).
    #[arg(long = "captures")]
    pub captures_file: PathBuf,

    /// Classifier model name (e.g. "claude-haiku-4-5-20251001"). Records which
    /// model the plugin ran in-session.
    #[arg(long = "classifier-model", default_value = "claude-haiku-4-5-20251001")]
    pub classifier_model: String,

    /// Schema version; must be "2" for downstream schema parity.
    #[arg(long = "schema-version", default_value = "2")]
    pub schema_version: String,

    #[command(flatten)]
    pub provenance: EmitCaptureProvenanceArgs,
}

/// Submit a pre-scored decision quality/importance assessment from a
/// plugin/edge session.
///
/// The scores JSON must match the schema produced by src/scorer.rs's
/// SCORER_PROMPT (quality_dims + importance) — the same contract the
/// server-side scorer's Haiku call produces. This path writes
/// DecisionScored directly — no ANTHROPIC_API_KEY needed. The target
/// capture node is resolved from a prior `ingest.batch_classified` batch
/// via `--batch-id` + `--capture-index`, so callers never construct the
/// `capture:{event_id}:{idx}` node-id format themselves — only the server
/// knows that shape.
#[derive(Debug, Clone, Args)]
pub struct EmitDecisionScoredArgs {
    /// batch_id returned by a prior `emit ingest.batch_classified` call.
    #[arg(long = "batch-id")]
    pub batch_id: String,

    /// Index of the decision capture within that batch's captures array
    /// (0-based, matching its position in the JSON array submitted to
    /// ingest.batch_classified).
    #[arg(long = "capture-index")]
    pub capture_index: usize,

    /// Path to a JSON file with `{"quality_dims": {...}, "importance": {...}}`
    /// matching the SCORER_PROMPT schema in src/scorer.rs.
    #[arg(long = "scores")]
    pub scores_file: PathBuf,

    /// Scorer model name (e.g. "claude-haiku-4-5-20251001"). Records which
    /// model the plugin ran in-session.
    #[arg(long = "scorer-model", default_value = "claude-haiku-4-5-20251001")]
    pub scorer_model: String,

    /// Quality-dimension weight version tag (e.g. "v1") so composites can
    /// recompute if weights change later.
    #[arg(long = "weight-version", default_value = "v1")]
    pub weight_version: String,

    #[command(flatten)]
    pub provenance: EmitCaptureProvenanceArgs,
}

#[derive(Debug, Clone, Args)]
pub struct EmitRelationAddedArgs {
    #[arg(long)]
    pub kind: EmitRelationKind,

    #[arg(long = "from")]
    pub from_id: String,

    #[arg(long = "to")]
    pub to_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EmitRelationKind {
    Supports,
    Refutes,
    #[value(alias = "based_on")]
    BasedOn,
}

#[derive(Debug, Clone, Args)]
pub struct ImportArgs {
    #[command(subcommand)]
    pub command: ImportCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ImportCommand {
    #[command(name = "documents", alias = "document")]
    Documents(ImportDocumentsArgs),
    #[command(name = "prepare-documents", alias = "prepare-document")]
    PrepareDocuments(PrepareDocumentsArgs),
    #[command(name = "connector")]
    Connector(ImportConnectorArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ImportDocumentsArgs {
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<PathBuf>,

    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    #[arg(long = "format", value_enum, default_value_t = ImportDocumentFormat::Auto)]
    pub format: ImportDocumentFormat,

    #[arg(long = "on-conflict", value_enum, default_value_t = ImportDocumentConflictAction::Report)]
    pub on_conflict: ImportDocumentConflictAction,

    /// Extractor command to use for prose documents (auto-detection: no Decision: blocks).
    #[arg(long = "extractor-command", value_enum)]
    pub extractor_command: Option<DocumentExtractorCommandArg>,

    /// Extra arguments forwarded to the extractor command.
    #[arg(long = "extractor-arg", value_name = "ARG")]
    pub extractor_args: Vec<String>,

    /// Path to a pre-computed LLM response file for prose extraction (mutually exclusive with --extractor-command).
    #[arg(long = "llm-response", value_name = "PATH")]
    pub llm_response: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ImportDocumentFormat {
    Auto,
    Markdown,
    Text,
}

impl ImportDocumentFormat {
    pub(crate) const fn as_ingest_format(self) -> DocumentImportFormat {
        match self {
            Self::Auto => DocumentImportFormat::Auto,
            Self::Markdown => DocumentImportFormat::Markdown,
            Self::Text => DocumentImportFormat::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ImportDocumentConflictAction {
    Report,
    #[value(alias = "keep")]
    KeepExisting,
    #[value(alias = "capture_superseding_decision")]
    Supersede,
    #[value(alias = "contest_existing")]
    Contest,
    #[value(alias = "add_new_context", alias = "add_new_evidence_hypothesis")]
    AddContext,
}

impl ImportDocumentConflictAction {
    pub(crate) const fn as_ingest_action(self) -> DocumentConflictResolutionAction {
        match self {
            Self::Report => DocumentConflictResolutionAction::Report,
            Self::KeepExisting => DocumentConflictResolutionAction::KeepExisting,
            Self::Supersede => DocumentConflictResolutionAction::Supersede,
            Self::Contest => DocumentConflictResolutionAction::Contest,
            Self::AddContext => DocumentConflictResolutionAction::AddContext,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct ImportConnectorArgs {
    #[command(subcommand)]
    pub command: ImportConnectorCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ImportConnectorCommand {
    #[command(name = "run")]
    Run(ImportConnectorRunArgs),
    #[command(name = "same-as-candidates")]
    SameAsCandidates(ImportConnectorSameAsCandidatesArgs),
    #[command(name = "confirm-same-as")]
    ConfirmSameAs(ImportConnectorConfirmSameAsArgs),
    #[command(name = "retract-same-as")]
    RetractSameAs(ImportConnectorRetractSameAsArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ImportConnectorRunArgs {
    #[arg(long = "url", value_name = "URL_OR_PATH")]
    pub url_or_id: String,

    #[arg(long = "max-versions", default_value_t = 50)]
    pub max_versions: usize,
}

#[derive(Debug, Clone, Args)]
pub struct ImportConnectorSameAsCandidatesArgs {
    #[arg(long = "since-run", value_name = "IMPORT_RUN_ID")]
    pub import_run_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ImportConnectorConfirmSameAsArgs {
    #[arg(long = "left", value_name = "DECISION_ID")]
    pub left_id: String,
    #[arg(long = "right", value_name = "DECISION_ID")]
    pub right_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ImportConnectorRetractSameAsArgs {
    #[arg(long = "left", value_name = "DECISION_ID")]
    pub left_id: String,
    #[arg(long = "right", value_name = "DECISION_ID")]
    pub right_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct PrepareDocumentsArgs {
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<PathBuf>,

    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    #[arg(long = "format", value_enum, default_value_t = PrepareDocumentFormat::Auto)]
    pub format: PrepareDocumentFormat,

    #[arg(long = "output-dir", value_name = "DIR")]
    pub output_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum PrepareDocumentFormat {
    Auto,
    Pdf,
    Text,
    OcrText,
}

impl PrepareDocumentFormat {
    pub(crate) const fn as_ingest_format(self) -> DocumentPreparationFormat {
        match self {
            Self::Auto => DocumentPreparationFormat::Auto,
            Self::Pdf => DocumentPreparationFormat::Pdf,
            Self::Text => DocumentPreparationFormat::Text,
            Self::OcrText => DocumentPreparationFormat::OcrText,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab_case")]
pub enum DocumentExtractorCommandArg {
    HivemindDocumentExtractor,
}

#[derive(Debug, Clone, Args)]
#[command(
    about = "Run deterministic read queries. JSON is the default output; use --summary for compact text."
)]
pub struct QueryArgs {
    #[arg(
        long = "summary",
        global = true,
        help = "Render compact human-readable text instead of JSON; JSON is the default output"
    )]
    pub summary: bool,

    #[command(subcommand)]
    pub command: QueryCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum QueryCommand {
    #[command(name = "get_decision")]
    GetDecision(QueryDecisionArgs),
    #[command(name = "get_relevant_decisions")]
    GetRelevantDecisions(QueryRelevantDecisionsArgs),
    /// Friendlier alias: `chain`.
    #[command(name = "get_supersession_chain", alias = "chain")]
    GetSupersessionChain(QueryFluentDecisionArgs),
    /// Friendlier alias: `why`.
    #[command(name = "get_decision_neighborhood", alias = "why")]
    GetDecisionNeighborhood(QueryDecisionNeighborhoodArgs),
    /// Layer-3 compact view: signal/noise filter over a decision's subgraph.
    #[command(name = "compact-view")]
    GetCompactView(QueryFluentDecisionArgs),
    /// "Did this decision hold up?" — leads with the decision, rationale, rejected options,
    /// who decided, and whether it still holds. Friendlier alias: `verify`.
    #[command(name = "get_decision_outcome", alias = "verify")]
    GetDecisionOutcome(QueryFluentDecisionArgs),
    #[command(name = "search")]
    Search(QuerySearchDecisionsArgs),
    #[command(name = "search_decisions")]
    SearchDecisions(QuerySearchDecisionsArgs),
    /// Layer-3 recall: search + summarize in one call. Answers "what was decided about X?".
    #[command(name = "recall")]
    Recall(QueryRecallArgs),
    #[command(name = "get_active_decision_blockers")]
    GetActiveDecisionBlockers(QueryActiveDecisionBlockersArgs),
    #[command(name = "get_blocker_notification_candidates")]
    GetBlockerNotificationCandidates(QueryBlockerNotificationCandidatesArgs),
    #[command(name = "recent_decisions", alias = "recent")]
    RecentDecisions(QueryRecentDecisionsArgs),
    #[command(name = "get_recent_activity")]
    GetRecentActivity(QueryRecentActivityArgs),
    #[command(name = "get_decisions_changed_since")]
    GetDecisionsChangedSince(QueryChangedSinceArgs),
    #[command(name = "get_decisions_added_since")]
    GetDecisionsAddedSince(QueryAddedSinceArgs),
    #[command(name = "export_read_only_summary")]
    ExportReadOnlySummary(QueryExportReadOnlySummaryArgs),
    /// In-house explainable quality score for a single decision.
    #[command(name = "score_decision")]
    ScoreDecision(QueryScoreDecisionArgs),
    /// Bulk in-house quality scan: scores all decisions (or a filtered subset).
    #[command(name = "scan_decision_quality")]
    ScanDecisionQuality(QueryScanDecisionQualityArgs),
    /// "What should I know before I touch this?" — decisions bearing on the working
    /// situation (touched paths / a diff / the current branch / cwd), no question needed.
    #[command(name = "situational")]
    GetSituationalDecisions(QuerySituationalArgs),
}

#[derive(Debug, Clone, Args)]
pub struct QueryDecisionArgs {
    #[arg(long = "id")]
    pub decision_id: String,
}

/// Shared arg shape for fluent follow-up verbs: a free-text `description` (resolved via
/// `resolve_decision_by_description`) with `--id` as the escape hatch, `--pick` to disambiguate,
/// and `--topic` to narrow. A bare `#N` description refers to candidate N from the previous
/// ambiguous resolver output. See docs/AGENT_FLUENT_QUERYING.md.
#[derive(Debug, Clone, Args)]
pub struct QueryFluentDecisionArgs {
    pub description: Option<String>,

    #[arg(long = "id")]
    pub decision_id: Option<String>,

    #[arg(long = "pick")]
    pub pick: Option<usize>,

    #[arg(long = "topic")]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryRelevantDecisionsArgs {
    #[arg(long = "topic")]
    pub topic: String,

    #[arg(long = "status")]
    pub status: Option<QueryDecisionStatus>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryDecisionNeighborhoodArgs {
    /// Free-text description to resolve to a decision (fluent alternative to --id).
    /// A bare `#N` refers to candidate N from the previous ambiguous resolver output.
    pub description: Option<String>,

    #[arg(long = "id")]
    pub decision_id: Option<String>,

    /// Select candidate N when a description resolves ambiguously.
    #[arg(long = "pick")]
    pub pick: Option<usize>,

    /// Narrow resolution to decisions carrying this topic key.
    #[arg(long = "topic")]
    pub topic: Option<String>,

    #[arg(long = "depth", default_value_t = 1)]
    pub depth: u8,

    #[arg(long = "relations", value_delimiter = ',')]
    pub relations: Vec<QueryRelationKind>,

    #[arg(
        long = "compact",
        help = "Return a CompactView (Layer-3 signal/noise filter) instead of the raw neighborhood"
    )]
    pub compact: bool,
}

#[derive(Debug, Clone, Args)]
pub struct QuerySearchDecisionsArgs {
    #[arg(long = "q")]
    pub query: Option<String>,

    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "status", value_delimiter = ',')]
    pub statuses: Vec<QueryDecisionStatus>,

    #[arg(long = "actor-id", value_delimiter = ',')]
    pub actor_ids: Vec<String>,

    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,

    #[arg(long = "since")]
    pub since: Option<String>,

    #[arg(long = "until")]
    pub until: Option<String>,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryRecallArgs {
    /// Free-text search query (what was decided about X?).
    pub query: Option<String>,

    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "status", value_delimiter = ',')]
    pub statuses: Vec<QueryDecisionStatus>,

    #[arg(long = "actor-id", value_delimiter = ',')]
    pub actor_ids: Vec<String>,

    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,

    #[arg(long = "since")]
    pub since: Option<String>,

    #[arg(long = "until")]
    pub until: Option<String>,

    #[arg(long = "limit", default_value_t = 5)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryActiveDecisionBlockersArgs {
    #[arg(long = "decision-id", value_delimiter = ',')]
    pub decision_ids: Vec<String>,

    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "owner", value_delimiter = ',')]
    pub required_owner_ids: Vec<String>,

    #[arg(long = "blocked-actor", value_delimiter = ',')]
    pub blocked_actor_ids: Vec<String>,

    #[arg(long = "priority", value_delimiter = ',')]
    pub priorities: Vec<QueryBlockerPriority>,

    #[arg(long = "now")]
    pub now: Option<String>,

    #[arg(long = "stale-after-seconds")]
    pub stale_after_seconds: Option<i64>,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryBlockerNotificationCandidatesArgs {
    #[arg(long = "now")]
    pub now: String,

    #[arg(long = "policy-version", default_value = "default-v1")]
    pub policy_version: String,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryHistoryFilterArgs {
    #[arg(long = "actor-id", value_delimiter = ',')]
    pub actor_ids: Vec<String>,

    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,

    #[arg(long = "source-ref", value_delimiter = ',')]
    pub source_refs: Vec<String>,

    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "status", value_delimiter = ',')]
    pub statuses: Vec<QueryDecisionStatus>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryRecentActivityArgs {
    #[command(flatten)]
    pub filters: QueryHistoryFilterArgs,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryRecentDecisionsArgs {
    #[arg(long = "since")]
    pub since: String,

    #[arg(long = "until")]
    pub until: Option<String>,

    #[arg(long = "timezone", default_value = "UTC")]
    pub timezone: String,

    #[arg(long = "now")]
    pub now: Option<String>,

    #[arg(long = "actor", value_delimiter = ',')]
    pub actor_patterns: Vec<String>,

    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "status", value_delimiter = ',')]
    pub statuses: Vec<QueryDecisionStatus>,

    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryChangedSinceArgs {
    #[arg(long = "since-offset")]
    pub since_offset: Option<u64>,

    #[arg(long = "since-ts", alias = "since-timestamp")]
    pub since_timestamp: Option<String>,

    #[arg(long = "until-offset")]
    pub until_offset: Option<u64>,

    #[arg(long = "until-ts", alias = "until-timestamp")]
    pub until_timestamp: Option<String>,

    #[command(flatten)]
    pub filters: QueryHistoryFilterArgs,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryAddedSinceArgs {
    #[arg(long = "since")]
    pub since: Option<String>,

    #[arg(long = "since-offset")]
    pub since_offset: Option<u64>,

    #[arg(long = "since-ts", alias = "since-timestamp")]
    pub since_timestamp: Option<String>,

    #[arg(long = "until")]
    pub until: Option<String>,

    #[arg(long = "until-offset")]
    pub until_offset: Option<u64>,

    #[arg(long = "until-ts", alias = "until-timestamp")]
    pub until_timestamp: Option<String>,

    #[arg(long = "timezone", default_value = "UTC")]
    pub timezone: String,

    #[arg(long = "now")]
    pub now: Option<String>,

    #[arg(long = "import-run", value_delimiter = ',')]
    pub import_run_ids: Vec<String>,

    #[command(flatten)]
    pub filters: QueryHistoryFilterArgs,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QuerySituationalArgs {
    /// Explicit files/dirs to treat as the situation. Defaults to the current git
    /// diff + staged set when omitted (and --diff/--branch/--cwd are not given).
    #[arg(long = "paths", value_delimiter = ',')]
    pub paths: Vec<String>,

    /// Read a unified diff from stdin instead of shelling out to `git diff`.
    #[arg(long = "diff")]
    pub diff: bool,

    /// Include the current branch name's tokens as part of the situation.
    #[arg(long = "branch")]
    pub branch: bool,

    /// Include the current working directory's path segments as situational terms.
    #[arg(long = "cwd")]
    pub cwd: bool,

    /// Annotate results with whether they changed since this ledger offset (exclusive).
    #[arg(long = "since-offset")]
    pub since_offset: Option<u64>,

    /// Annotate results with whether they changed since this timestamp.
    #[arg(long = "since-ts", alias = "since-timestamp")]
    pub since_timestamp: Option<String>,

    /// Resolve the since-boundary to the timestamp of the commit where the current
    /// branch diverged from --base, so "what changed since I last worked here" is
    /// one call. Mutually exclusive with --since-offset/--since-ts.
    #[arg(long = "since-branch-point")]
    pub since_branch_point: bool,

    /// Base ref for --since-branch-point's merge-base resolution.
    #[arg(long = "base", default_value = "origin/master")]
    pub base: String,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryExportReadOnlySummaryArgs {
    #[arg(long = "query", value_enum)]
    pub query: QueryExportKind,

    #[arg(long = "format", value_enum, default_value_t = QueryExportFormat::Json)]
    pub format: QueryExportFormat,

    #[arg(long = "generated-at")]
    pub generated_at: Option<String>,

    #[arg(long = "since-offset")]
    pub since_offset: Option<u64>,

    #[arg(long = "since-ts", alias = "since-timestamp")]
    pub since_timestamp: Option<String>,

    #[arg(long = "until-offset")]
    pub until_offset: Option<u64>,

    #[arg(long = "until-ts", alias = "until-timestamp")]
    pub until_timestamp: Option<String>,

    #[command(flatten)]
    pub filters: QueryHistoryFilterArgs,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum QueryExportKind {
    RecentActivity,
    DecisionsChangedSince,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum QueryExportFormat {
    Json,
    Markdown,
}

impl QueryExportFormat {
    pub(crate) const fn as_query_format(self) -> QueryReadOnlyExportFormat {
        match self {
            QueryExportFormat::Json => QueryReadOnlyExportFormat::Json,
            QueryExportFormat::Markdown => QueryReadOnlyExportFormat::Markdown,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct TuiArgs {
    #[arg(long = "q")]
    pub query: Option<String>,

    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "status", value_delimiter = ',')]
    pub statuses: Vec<QueryDecisionStatus>,

    #[arg(long = "actor-id", value_delimiter = ',')]
    pub actor_ids: Vec<String>,

    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,

    #[arg(long = "limit", default_value_t = 25)]
    pub limit: usize,

    #[arg(long = "dot-output", default_value = "hivemind-neighborhood.dot")]
    pub dot_output: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum QueryRelationKind {
    ProposedBy,
    AcceptedBy,
    RejectedBy,
    Supersedes,
    BasedOn,
    HasOption,
    Chose,
    PremisedOn,
    Supports,
    Refutes,
}

impl QueryRelationKind {
    pub(crate) const fn as_graph_relation(self) -> GraphRelationKind {
        match self {
            QueryRelationKind::ProposedBy => GraphRelationKind::ProposedBy,
            QueryRelationKind::AcceptedBy => GraphRelationKind::AcceptedBy,
            QueryRelationKind::RejectedBy => GraphRelationKind::RejectedBy,
            QueryRelationKind::Supersedes => GraphRelationKind::Supersedes,
            QueryRelationKind::BasedOn => GraphRelationKind::BasedOn,
            QueryRelationKind::HasOption => GraphRelationKind::HasOption,
            QueryRelationKind::Chose => GraphRelationKind::Chose,
            QueryRelationKind::PremisedOn => GraphRelationKind::PremisedOn,
            QueryRelationKind::Supports => GraphRelationKind::Supports,
            QueryRelationKind::Refutes => GraphRelationKind::Refutes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QueryDecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Contested,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum QueryBlockerPriority {
    P0,
    P1,
    P2,
    P3,
    P4,
}

impl QueryBlockerPriority {
    pub(crate) const fn as_blocker_priority(self) -> BlockerPriority {
        match self {
            QueryBlockerPriority::P0 => BlockerPriority::P0,
            QueryBlockerPriority::P1 => BlockerPriority::P1,
            QueryBlockerPriority::P2 => BlockerPriority::P2,
            QueryBlockerPriority::P3 => BlockerPriority::P3,
            QueryBlockerPriority::P4 => BlockerPriority::P4,
        }
    }
}

impl QueryDecisionStatus {
    pub(crate) const fn as_decision_status(self) -> DecisionStatus {
        match self {
            QueryDecisionStatus::Proposed => DecisionStatus::Proposed,
            QueryDecisionStatus::Accepted => DecisionStatus::Accepted,
            QueryDecisionStatus::Rejected => DecisionStatus::Rejected,
            QueryDecisionStatus::Contested => DecisionStatus::Contested,
            QueryDecisionStatus::Superseded => DecisionStatus::Superseded,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct DumpArgs {
    #[arg(long, value_enum, default_value_t = DumpFormat::Dot)]
    pub format: DumpFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DumpFormat {
    Dot,
}

#[derive(Debug, Clone, Args)]
pub struct QueryScoreDecisionArgs {
    /// Decision ID to score.
    #[arg(long = "id")]
    pub decision_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct QueryScanDecisionQualityArgs {
    /// Minimum ledger event offset (inclusive). Filters to decisions proposed at or after this offset.
    #[arg(long = "since-event-origin")]
    pub since_event_origin: Option<i64>,

    /// Maximum results to return (1–1000, default 25).
    #[arg(long, default_value_t = 25)]
    pub limit: usize,

    /// Pagination cursor from a previous response.
    #[arg(long)]
    pub cursor: Option<String>,

    /// Only return decisions at this tier or worse.
    #[arg(long, value_enum)]
    pub min_tier: Option<QueryQualityTier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum QueryQualityTier {
    Clean,
    MinorConcerns,
    SignificantConcerns,
    HighConcern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliExit {
    Success = 0,
    Generic = 1,
    Validation = 2,
    Invariant = 3,
    Storage = 4,
}

impl CliExit {
    pub const fn code(self) -> i32 {
        self as i32
    }
}

pub fn parse() -> Cli {
    Cli::parse()
}
