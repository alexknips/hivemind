use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write as IoWrite};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use uuid::Uuid;

use crate::commands::{CommandContext, Commands};
use crate::error::{CliError, CommandError};
use crate::events::{
    BlockerPriority, CaptureItem, Event, EventId, EventPayload, EventProvenance, EventType,
    RelationKind as EventRelationKind, TenantId,
};
use crate::identity::{
    agent_actor_id, default_actor, default_agent_session, default_agent_tool,
    default_human_actor_id,
};
use crate::ingest::{
    extract_slack_decision_draft, import_documents, import_slack_thread,
    parse_slack_thread_fixture, prepare_document_texts, DocumentConflictResolutionAction,
    DocumentImportFormat, DocumentImportReport, DocumentImportRequest, DocumentPreparationFormat,
    DocumentPreparationReport, DocumentPreparationRequest, SlackIngestOutcome,
    DEFAULT_SLACK_MENTION,
};
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::PostgresEventLedger;
use crate::ledger::{EventLedger, SqliteEventLedger, TenantScopedLedger};
use crate::projector::{
    memory::MemoryGraph, rebuild_graph_for_tenant, GraphParams, GraphProperties, GraphRow,
    GraphValue, GraphView, NodeKind, RelationKind as GraphRelationKind,
};
use crate::queries::{
    derive_decision_status, derive_hypothesis_status, export_read_only_summary,
    get_active_decision_blockers, get_blocker_notification_candidates, get_compact_view,
    get_decision, get_decision_neighborhood, get_decisions_added_since,
    get_decisions_changed_since, get_recent_activity, get_recent_decisions, get_relevant_decisions,
    get_supersession_chain, search_decisions, search_decisions_fts_with_context,
    ActiveDecisionBlockersRequest, BlockerNotificationCandidates,
    BlockerNotificationCandidatesRequest, ChangedSinceRequest, CompactView, DecisionBlockerFilters,
    DecisionBlockerResults, DecisionSearchResults, DecisionStatus, DecisionView,
    DecisionsAddedSinceFilterRequest, DecisionsAddedSinceRequest, DecisionsAddedSinceResults,
    DecisionsChangedSinceResults, HistoryChangeKind, HistoryFilterRequest, HypothesisStatus,
    NeighborhoodRequest, NeighborhoodView, QueryContext, QueryResponse, ReadOnlyExport,
    ReadOnlyExportFormat as QueryReadOnlyExportFormat, ReadOnlyExportQuery,
    ReadOnlyExportQueryKind, ReadOnlyExportRequest, RecentActivityRequest, RecentActivityResults,
    RecentDecisionEntry, RecentDecisionFilterRequest, RecentDecisionsRequest,
    RecentDecisionsResults, SearchDecisionRequest, SupersessionChain,
};
use crate::slack_app::{
    handle_slack_command, slack_app_manifest, slack_oauth_install_url, SlackAppStore,
    SlackCaptureRequest, SlackCaptureSurface, SlackCommandRequest, SlackWorkspaceInstall,
};
use crate::suggest::{
    materialize_document_extraction_candidates, propose_document_extraction_candidates,
    DocumentCandidateExtractor, DocumentCandidateMaterializationRequest, DocumentCandidateRequest,
};
use crate::summarize::{
    recall_decisions, weekly_digest, DigestRequest, RecallRequest, DIGEST_MAX_DECISIONS,
    RECALL_MAX_LIMIT,
};
use crate::{HivemindError, Result};

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
    Suggest(SuggestArgs),
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
}

#[derive(Debug, Clone, Args)]
pub struct QuickstartArgs {}

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
    /// Accepts either an inline JSON string or a path to a JSON file (prefix with @).
    /// Example: --captures '[{"kind":"decision",...}]'
    /// Example: --captures @/tmp/captures.json
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
    #[arg(long = "decision")]
    pub decision_id: String,

    #[arg(long)]
    pub reason: String,
}

#[derive(Debug, Clone, Args)]
pub struct SupersedeArgs {
    #[arg(long = "old")]
    pub old_decision_id: String,

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
    const fn as_slack_surface(self) -> SlackCaptureSurface {
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
    fn has_override(&self) -> bool {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ImportDocumentFormat {
    Auto,
    Markdown,
    Text,
}

impl ImportDocumentFormat {
    const fn as_ingest_format(self) -> DocumentImportFormat {
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
    const fn as_ingest_action(self) -> DocumentConflictResolutionAction {
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
    const fn as_ingest_format(self) -> DocumentPreparationFormat {
        match self {
            Self::Auto => DocumentPreparationFormat::Auto,
            Self::Pdf => DocumentPreparationFormat::Pdf,
            Self::Text => DocumentPreparationFormat::Text,
            Self::OcrText => DocumentPreparationFormat::OcrText,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct SuggestArgs {
    #[command(subcommand)]
    pub command: SuggestCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SuggestCommand {
    #[command(name = "document-candidates")]
    DocumentCandidates(SuggestDocumentCandidatesArgs),
    #[command(name = "materialize-document-candidates")]
    MaterializeDocumentCandidates(MaterializeDocumentCandidatesArgs),
}

#[derive(Debug, Clone, Args)]
pub struct SuggestDocumentCandidatesArgs {
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<PathBuf>,

    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    #[arg(long = "format", value_enum, default_value_t = ImportDocumentFormat::Auto)]
    pub format: ImportDocumentFormat,

    #[arg(long = "extractor-command", value_enum)]
    pub extractor_command: Option<DocumentExtractorCommandArg>,

    #[arg(long = "extractor-arg", value_name = "ARG")]
    pub extractor_args: Vec<String>,

    #[arg(long = "llm-response", value_name = "PATH")]
    pub llm_response: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct MaterializeDocumentCandidatesArgs {
    #[arg(long = "input", value_name = "PATH")]
    pub input: PathBuf,

    #[arg(long = "candidate-id", value_name = "ID")]
    pub candidate_ids: Vec<String>,

    #[arg(long = "output", value_name = "PATH")]
    pub output: PathBuf,
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
    #[command(name = "get_supersession_chain")]
    GetSupersessionChain(QueryDecisionArgs),
    #[command(name = "get_decision_neighborhood")]
    GetDecisionNeighborhood(QueryDecisionNeighborhoodArgs),
    /// Layer-3 compact view: signal/noise filter over a decision's subgraph.
    #[command(name = "compact-view")]
    GetCompactView(QueryDecisionArgs),
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
}

#[derive(Debug, Clone, Args)]
pub struct QueryDecisionArgs {
    #[arg(long = "id")]
    pub decision_id: String,
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
    #[arg(long = "id")]
    pub decision_id: String,

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
    const fn as_query_format(self) -> QueryReadOnlyExportFormat {
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
    Assumes,
    Supports,
    Refutes,
}

impl QueryRelationKind {
    const fn as_graph_relation(self) -> GraphRelationKind {
        match self {
            QueryRelationKind::ProposedBy => GraphRelationKind::ProposedBy,
            QueryRelationKind::AcceptedBy => GraphRelationKind::AcceptedBy,
            QueryRelationKind::RejectedBy => GraphRelationKind::RejectedBy,
            QueryRelationKind::Supersedes => GraphRelationKind::Supersedes,
            QueryRelationKind::BasedOn => GraphRelationKind::BasedOn,
            QueryRelationKind::HasOption => GraphRelationKind::HasOption,
            QueryRelationKind::Chose => GraphRelationKind::Chose,
            QueryRelationKind::Assumes => GraphRelationKind::Assumes,
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
    const fn as_blocker_priority(self) -> BlockerPriority {
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
    const fn as_decision_status(self) -> DecisionStatus {
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

pub fn run(cli: &Cli) -> Result<String> {
    validate_global_flags(cli)?;

    match &cli.command {
        Command::Quickstart(args) => run_quickstart(cli, args),
        Command::Emit(command) => run_emit(cli, command),
        Command::Disagree(args) => run_disagree(cli, args),
        Command::Supersede(args) => run_supersede(cli, args),
        Command::Review(args) => run_review(cli, args),
        Command::Import(import) => run_import(cli, import),
        Command::Suggest(suggest) => run_suggest(cli, suggest),
        Command::Query(query) => run_query(cli, query),
        Command::Dump(dump) => run_dump(cli, dump),
        Command::Tui(args) => run_tui(cli, args),
        Command::Ingest(args) => run_ingest(cli, args),
        Command::SlackApp(args) => run_slack_app(cli, args),
        Command::Mcp(args) => run_mcp(cli, args),
        Command::Serve(args) => run_serve(cli, args),
        #[cfg(feature = "shared-backend-postgres")]
        Command::Migrate(args) => run_migrate(cli, args),
        Command::Map(args) => run_map(cli, args),
        Command::Digest(args) => run_digest(cli, args),
        Command::ClassifyQueue(args) => run_classify_queue(cli, args),
    }
}

fn run_quickstart(cli: &Cli, _args: &QuickstartArgs) -> Result<String> {
    let ledger_dir = std::env::temp_dir().join(format!("hivemind-quickstart-{}", Uuid::new_v4()));
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&ledger_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::cli()),
    );
    let decision_args = EmitDecisionProposedArgs {
        title: "Try HiveMind quickstart".to_owned(),
        rationale: "A first decision should be captured with actor provenance and queried back immediately.".to_owned(),
        topic_keys: vec!["quickstart".to_owned(), "onboarding".to_owned()],
        option_ids: vec!["local-ledger".to_owned(), "spreadsheet".to_owned()],
        chosen_option_id: Some("local-ledger".to_owned()),
        hypothesis_ids: Vec::new(),
        evidence_ids: Vec::new(),
    };
    let decision_id = propose_decision_from_option_labels(&commands, &cli.actor, &decision_args)?;

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
    let query = search_decisions(
        &graph,
        &SearchDecisionRequest {
            query: Some("quickstart".to_owned()),
            topic_keys: vec!["quickstart".to_owned()],
            statuses: vec![DecisionStatus::Proposed],
            actor_ids: vec![cli.actor.clone()],
            sources: vec!["cli".to_owned()],
            since: None,
            until: None,
            limit: 5,
            cursor: None,
        },
    )?;
    let first_result_id = query
        .data
        .items
        .first()
        .map(|item| item.decision.id.clone());

    if first_result_id.as_deref() != Some(decision_id.as_str()) {
        return Err(CliError::InvalidInput(
            "quickstart query did not return captured decision".to_owned(),
        )
        .into());
    }

    let report = QuickstartReport {
        ledger_dir: ledger_dir.display().to_string(),
        actor_id: cli.actor.clone(),
        decision_id,
        query: QuickstartQueryReport {
            result_count: query.result_count,
            total_matches: query.data.total_matches,
            truncated: query.truncated,
            first_result_id,
        },
    };

    if cli.json {
        format_json_value(true, &report)
    } else {
        Ok(format_quickstart_report(&report))
    }
}

fn format_quickstart_report(report: &QuickstartReport) -> String {
    format!(
        "HiveMind quickstart complete.\n\
         Ledger: {ledger_dir}\n\
         Actor: {actor_id}\n\
         Captured: {decision_id}\n\
         Queried: found {first_result_id} ({result_count} result, truncated={truncated})\n\n\
         Try the query again:\n\
           hivemind --hivemind-dir {ledger_dir} query search_decisions --topic quickstart --limit 5",
        ledger_dir = report.ledger_dir,
        actor_id = report.actor_id,
        decision_id = report.decision_id,
        first_result_id = report
            .query
            .first_result_id
            .as_deref()
            .unwrap_or("<missing>"),
        result_count = report.query.result_count,
        truncated = report.query.truncated
    )
}

fn run_mcp(cli: &Cli, args: &McpArgs) -> Result<String> {
    let mut config =
        crate::mcp::McpConfig::new(cli.hivemind_dir.clone()).with_tenant(cli_tenant(cli)?);
    if let Some(agent_tool) = args.agent_tool.as_deref().map(str::trim) {
        if !agent_tool.is_empty() {
            config = config.with_agent_tool(agent_tool);
        }
    }
    if let Some(session_id) = args.session_id.as_deref().map(str::trim) {
        if !session_id.is_empty() {
            config = config.with_session_id(session_id);
        }
    }
    crate::mcp::serve_stdio(&config)?;
    // The stdio loop only returns once stdin closes — no payload to print.
    Ok(String::new())
}

fn run_serve(cli: &Cli, args: &ServeArgs) -> Result<String> {
    let config = crate::api::ApiConfig::new(cli.hivemind_dir.clone()).with_port(args.port);
    // Build AppState (which constructs r2d2/postgres pool) BEFORE entering
    // the tokio runtime. r2d2 pool construction internally calls block_on,
    // which panics if already inside an existing runtime.
    let state = crate::api::AppState::from_config(&config)?;
    // Hold a clone so the Arc<ApiBackend> (postgres pool) survives until AFTER
    // the runtime is dropped. Rust drops locals in reverse declaration order:
    // `runtime` (declared below) drops before `_pg_guard`, so the pool's Drop
    // runs outside any runtime context — preventing the block_on-within-block_on
    // SIGABRT on scale-to-zero autostop (hivemind-noc9).
    let _pg_guard = state.clone();
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| CliError::InvalidInput(format!("failed to create tokio runtime: {e}")))?;
    runtime.block_on(crate::api::serve_http(state, &config))?;
    Ok(String::new())
}

fn run_map(cli: &Cli, args: &MapArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;

    let alphas: Vec<f64> = if args.alpha.is_empty() {
        vec![0.5]
    } else {
        args.alpha.clone()
    };

    if alphas.len() == 1 {
        let result = crate::map::compute_map(&graph, &cli.hivemind_dir, alphas[0]) // ubs:ignore: alphas[0] guarded by len()==1 check above
            .map_err(|e| CliError::InvalidInput(e.to_string()))?; // ubs:ignore: error conversion at CLI boundary
        if args.summary {
            let mut out = format!(
                "Decision map: {} decisions, alpha={:.2}, gen={}\n", // ubs:ignore: format! for CLI output
                result.n,
                result.alpha,
                &result.gen_id[..8] // ubs:ignore: UUID is 36 chars; 8-char prefix always in-bounds
            );
            let points: String = result
                .points
                .iter()
                .map(|p| {
                    format!(
                        "  [{:>6.2}, {:>6.2}] {:8} {}\n",
                        p.x_time, p.y_spectral, p.status, p.title
                    )
                })
                .collect();
            out.push_str(&points);
            Ok(out)
        } else {
            serde_json::to_string_pretty(&result)
                .map_err(|e| CliError::InvalidInput(e.to_string()).into()) // ubs:ignore: error conversion at CLI boundary
        }
    } else {
        let mut results = Vec::new();
        for &alpha in &alphas {
            let r = crate::map::compute_map(&graph, &cli.hivemind_dir, alpha)
                .map_err(|e| CliError::InvalidInput(e.to_string()))?; // ubs:ignore: error conversion at CLI boundary
            results.push(r);
        }
        serde_json::to_string_pretty(&results)
            .map_err(|e| CliError::InvalidInput(e.to_string()).into()) // ubs:ignore: error conversion at CLI boundary
    }
}

fn parse_window_duration(window: &str) -> Result<chrono::Duration> {
    let window = window.trim();
    let (digits, unit) = window.split_at(
        window
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(window.len()), // ubs:ignore: unwrap_or — safe default: all-digits treated as days
    );
    let n: i64 = digits
        .parse()
        .map_err(|_| CliError::InvalidInput(format!("--window: invalid duration '{window}'")))?;
    if n <= 0 {
        return Err(CliError::InvalidInput(format!(
            "--window: duration must be positive, got '{window}'"
        ))
        .into());
    }
    match unit {
        "h" => Ok(chrono::Duration::hours(n)),
        "d" | "" => Ok(chrono::Duration::days(n)),
        "w" => Ok(chrono::Duration::weeks(n)),
        other => Err(CliError::InvalidInput(format!(
            "--window: unknown unit '{other}'; use h (hours), d (days), or w (weeks)"
        ))
        .into()),
    }
}

fn run_digest(cli: &Cli, args: &DigestArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let context = QueryContext::new(tenant_id.clone());
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;

    let now = Utc::now();

    let until = match args.until.as_deref() {
        Some(s) => parse_required_query_datetime(s, "--until")?,
        None => now,
    };
    let since = match args.since.as_deref() {
        Some(s) => parse_required_query_datetime(s, "--since")?,
        None => {
            let duration = parse_window_duration(&args.window)?;
            until - duration
        }
    };

    let request = DigestRequest {
        since,
        until,
        actor_ids: args.actor_ids.clone(), // ubs:ignore: clone necessary — building owned DigestRequest from borrowed DigestArgs
        limit: args.limit,
    };
    let response = weekly_digest(&context, &ledger, &graph, &request)?;

    if args.summary {
        let mut out = response.data.text.clone(); // ubs:ignore: clone necessary — formatting owned output from borrowed DigestResponse
        append_truncation_notice(&mut out, response.truncated, None);
        Ok(out.trim_end().to_owned())
    } else {
        format_json_value(true, &response)
    }
}

fn run_classify_queue(cli: &Cli, args: &ClassifyQueueArgs) -> Result<String> {
    match &args.command {
        ClassifyQueueCommand::List(args) => run_classify_queue_list(cli, args),
        ClassifyQueueCommand::Submit(args) => run_classify_queue_submit(cli, args),
    }
}

fn run_classify_queue_list(cli: &Cli, args: &ClassifyQueueListArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let mut batches = crate::classifier::list_pending_batches(&cli.hivemind_dir, &tenant_id)
        .map_err(|e| CliError::InvalidInput(format!("ledger scan failed: {e}")))?;
    batches.truncate(args.limit);
    format_json_value(cli.json, &batches)
}

fn run_classify_queue_submit(cli: &Cli, args: &ClassifyQueueSubmitArgs) -> Result<String> {
    let json_str = if let Some(path) = args.captures.strip_prefix('@') {
        std::fs::read_to_string(path).map_err(|e| {
            CliError::InvalidInput(format!("cannot read captures file '{path}': {e}"))
        })?
    } else {
        args.captures.clone()
    };
    let captures: Vec<CaptureItem> = serde_json::from_str(&json_str)
        .map_err(|e| CliError::InvalidInput(format!("--captures is not valid JSON: {e}")))?;

    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id, EventProvenance::cli()),
    );

    let capture_count = captures.len();
    commands.record_ingest_batch_classified(
        &cli.actor,
        &args.batch_id,
        &args.model,
        crate::classifier::SCHEMA_VERSION,
        captures,
        None,
    )?;

    let result = serde_json::json!({
        "batch_id": args.batch_id,
        "capture_count": capture_count,
    });
    format_json_value(cli.json, &result)
}

fn run_ingest(cli: &Cli, ingest: &IngestArgs) -> Result<String> {
    match &ingest.command {
        IngestCommand::SlackThread(args) => run_ingest_slack_thread(cli, args),
    }
}

fn run_ingest_slack_thread(cli: &Cli, args: &IngestSlackThreadArgs) -> Result<String> {
    let contents = std::fs::read_to_string(&args.file).map_err(|error| {
        CliError::InvalidInput(format!(
            "could not read slack thread file {}: {error}",
            args.file.display()
        ))
    })?;
    let thread = parse_slack_thread_fixture(&contents)?;
    let draft = extract_slack_decision_draft(&thread, &args.mention)?;

    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, cli_tenant(cli)?);
    let outcome = import_slack_thread(&scoped_ledger, &draft)?;

    let kind = match outcome {
        SlackIngestOutcome::Imported { .. } => "decision_id",
        SlackIngestOutcome::AlreadyImported { .. } => "decision_id_existing",
    };
    let envelope = OutputEnvelope::new("ingest", kind, outcome.decision_id().to_owned());
    format_output(cli.json, &envelope)
}

fn run_slack_app(cli: &Cli, args: &SlackAppArgs) -> Result<String> {
    let store = SlackAppStore::new(&cli.hivemind_dir);
    match &args.command {
        SlackAppCommand::Manifest(args) => {
            let manifest = slack_app_manifest(
                &args.request_url,
                args.event_url.as_deref(),
                args.redirect_url.as_deref(),
            )?;
            format_json_value(cli.json, &manifest)
        }
        SlackAppCommand::OauthUrl(args) => {
            slack_oauth_install_url(&args.client_id, &args.redirect_uri, &args.state)
        }
        SlackAppCommand::Install(args) => {
            let summary = store.install_workspace(SlackWorkspaceInstall {
                team_id: args.team_id.clone(),
                team_name: args.team_name.clone(),
                bot_token: args.bot_token.clone(),
                signing_secret: args.signing_secret.clone(),
                hivemind_url: args.hivemind_url.clone(),
                reaction_emoji: args.reaction_emoji.clone(),
                actor_mappings: parse_actor_mappings(&args.actor_mappings)?,
            })?;
            format_json_value(cli.json, &summary)
        }
        SlackAppCommand::EnqueueCapture(args) => {
            let event = store.enqueue_capture(SlackCaptureRequest {
                team_id: args.team_id.clone(),
                user_id: args.user_id.clone(),
                channel_id: args.channel_id.clone(),
                message_ts: args.message_ts.clone(),
                thread_ts: args
                    .thread_ts
                    .clone()
                    .unwrap_or_else(|| args.message_ts.clone()),
                permalink: args.permalink.clone(),
                surface: args.surface.as_slack_surface(),
                reaction_emoji: args.reaction_emoji.clone(),
                title: args.title.clone(),
                rationale: args.rationale.clone(),
                topic_keys: args.topic_keys.clone(),
                option_labels: args.option_labels.clone(),
                chosen_option_label: args.chosen_option_label.clone(),
                thread_text: args.thread_text.clone(),
            })?;
            format_json_value(cli.json, &event)
        }
        SlackAppCommand::Drain(_) => {
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let scoped_ledger = TenantScopedLedger::new(&ledger, cli_tenant(cli)?);
            let report = store.drain_queue(&scoped_ledger)?;
            format_json_value(cli.json, &report)
        }
        SlackAppCommand::Command(args) => {
            let tenant_id = cli_tenant(cli)?;
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let scoped_ledger = TenantScopedLedger::new(&ledger, tenant_id.clone());
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
            let response = handle_slack_command(
                &scoped_ledger,
                &graph,
                &store,
                &SlackCommandRequest {
                    team_id: args.team_id.clone(),
                    user_id: args.user_id.clone(),
                    text: args.text.clone(),
                    limit: args.limit,
                },
            )?;
            format_json_value(cli.json, &response)
        }
    }
}

fn run_emit(cli: &Cli, emit: &EmitArgs) -> Result<String> {
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        cli_command_context(cli, cli_emit_provenance(&cli.actor))?,
    );

    let output = match &emit.command {
        EmitCommand::DecisionCapture(args) => {
            let (actor_id, provenance) = capture_actor_and_provenance(&args.provenance)?;
            let commands =
                Commands::new_with_context(&ledger, cli_command_context(cli, provenance)?);
            let decision_id =
                propose_decision_from_option_labels(&commands, &actor_id, &args.decision)?;
            OutputEnvelope::new("emit", "decision_id", decision_id)
        }
        EmitCommand::DecisionProposed(args) => {
            let decision_id = propose_decision_from_option_labels(&commands, &cli.actor, args)?;
            OutputEnvelope::new("emit", "decision_id", decision_id)
        }
        EmitCommand::DecisionAccepted(args) => {
            let event_id = commands.accept_decision(&args.decision_id, &cli.actor)?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::DecisionRejected(args) => {
            let event_id = commands.reject_decision(&args.decision_id, &cli.actor)?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::DecisionSuperseded(args) => {
            let event_id = commands.supersede_decision(
                &args.old_decision_id,
                &args.new_decision_id,
                &cli.actor,
            )?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::EvidenceRecorded(args) => {
            let (actor_id, commands) = emit_actor_and_commands(cli, &ledger, &args.provenance)?;
            let evidence_id = commands.record_evidence(&actor_id, &args.content)?;
            OutputEnvelope::new("emit", "evidence_id", evidence_id)
        }
        EmitCommand::HypothesisRecorded(args) => {
            let (actor_id, commands) = emit_actor_and_commands(cli, &ledger, &args.provenance)?;
            let hypothesis_id = commands.record_hypothesis(&actor_id, &args.statement)?;
            OutputEnvelope::new("emit", "hypothesis_id", hypothesis_id)
        }
        EmitCommand::OptionRecorded(args) => {
            let option_id = commands.record_option(&cli.actor, &args.label, &args.description)?;
            OutputEnvelope::new("emit", "option_id", option_id)
        }
        EmitCommand::RelationAdded(args) => {
            let event_id = match args.kind {
                EmitRelationKind::BasedOn => {
                    commands.attach_evidence(&args.from_id, &args.to_id, &cli.actor)?
                }
                EmitRelationKind::Supports => commands.relate_evidence_to_hypothesis(
                    &args.from_id,
                    &args.to_id,
                    EventRelationKind::Supports,
                    &cli.actor,
                )?,
                EmitRelationKind::Refutes => commands.relate_evidence_to_hypothesis(
                    &args.from_id,
                    &args.to_id,
                    EventRelationKind::Refutes,
                    &cli.actor,
                )?,
            };

            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::AttachEvidence(args) => {
            let event_id =
                commands.attach_evidence(&args.decision_id, &args.evidence_id, &cli.actor)?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
    };

    format_output(cli.json, &output)
}

fn run_disagree(cli: &Cli, args: &DisagreeArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::human(cli.actor.clone())),
    );
    let event_id = commands.disagree(&cli.actor, &args.decision_id, &args.reason)?;
    let decision_status = decision_status_after_write(&ledger, &tenant_id, &args.decision_id)?;

    format_disagree_output(
        cli.json,
        &DisagreeCommandOutput {
            decision_id: args.decision_id.clone(),
            event_id,
            decision_status,
        },
    )
}

fn run_supersede(cli: &Cli, args: &SupersedeArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::human(cli.actor.clone())),
    );
    let outcome = commands.supersede(
        &cli.actor,
        &args.old_decision_id,
        &args.title,
        &args.rationale,
        &args.topic_keys,
        &args.option_labels,
        args.chosen_option_label.as_deref(),
        &args.hypothesis_ids,
        &args.evidence_ids,
    )?;
    let old_decision_status =
        decision_status_after_write(&ledger, &tenant_id, &args.old_decision_id)?;
    let new_decision_status =
        decision_status_after_write(&ledger, &tenant_id, &outcome.new_decision_id)?;

    format_supersede_output(
        cli.json,
        &SupersedeCommandOutput {
            old_decision_id: args.old_decision_id.clone(),
            new_decision_id: outcome.new_decision_id,
            proposal_event_id: outcome.proposal_event_id,
            relation_event_ids: outcome.relation_event_ids,
            superseded_event_id: outcome.superseded_event_id,
            old_decision_status,
            new_decision_status,
        },
    )
}

fn run_review(cli: &Cli, args: &ReviewArgs) -> Result<String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stderr = io::stderr();
    let mut prompt_output = stderr.lock();
    run_review_session(cli, args, &mut input, &mut prompt_output)
}

fn run_review_session<R: BufRead, W: IoWrite>(
    cli: &Cli,
    args: &ReviewArgs,
    input: &mut R,
    prompt_output: &mut W,
) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, tenant_id.clone());
    let request = review_recent_decisions_request(args)?;
    let response = get_recent_decisions(&scoped_ledger, &request)?;
    let events = read_ledger_events(&scoped_ledger)?;
    let context = ReviewLedgerContext::from_events(&events)?;
    let reviewed_decision_ids = reviewed_decision_ids_by_actor(&events, &cli.actor)?;
    let mut items = response.data.items;

    if args.unreviewed_only {
        items.retain(|item| !reviewed_decision_ids.contains(&item.decision_id));
    }

    if items.is_empty() {
        writeln!(prompt_output, "No matching decisions to review.").map_err(cli_io_error)?;
        return format_review_output(
            cli.json,
            &ReviewCommandOutput {
                reviewer_actor_id: cli.actor.clone(),
                matched_count: 0,
                reviewed_count: 0,
                skipped_count: 0,
                quit: false,
                truncated: response.truncated,
                next_cursor: response.data.next_cursor,
                unreviewed_only: args.unreviewed_only,
                reviewed_semantics: REVIEWED_SEMANTICS,
                actions: Vec::new(),
            },
        );
    }

    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::human(cli.actor.clone())),
    );
    let mut actions = Vec::new();
    let mut quit = false;
    let matched_count = items.len();

    for (index, item) in items.into_iter().enumerate() {
        render_review_item(prompt_output, index + 1, matched_count, &item, &context)?;

        loop {
            let Some(action) = prompt_line(
                input,
                prompt_output,
                "Action [a approve, d disagree, s supersede, n next, q quit]: ",
            )?
            else {
                quit = true;
                break;
            };
            match action.trim().to_ascii_lowercase().as_str() {
                "a" | "approve" => {
                    let event_id = commands.accept_decision(&item.decision_id, &cli.actor)?;
                    let status =
                        decision_status_after_write(&ledger, &tenant_id, &item.decision_id)?;
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "approved",
                        event_id: Some(event_id),
                        proposal_event_id: None,
                        superseded_event_id: None,
                        new_decision_id: None,
                        old_decision_status: Some(status),
                        new_decision_status: None,
                    });
                    break;
                }
                "d" | "disagree" => {
                    let Some(reason) = prompt_required_line(
                        input,
                        prompt_output,
                        "Disagreement reason: ",
                        "reason must not be empty",
                    )?
                    else {
                        quit = true;
                        break;
                    };
                    let event_id = commands.disagree(&cli.actor, &item.decision_id, &reason)?;
                    let status =
                        decision_status_after_write(&ledger, &tenant_id, &item.decision_id)?;
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "disagreed",
                        event_id: Some(event_id),
                        proposal_event_id: None,
                        superseded_event_id: None,
                        new_decision_id: None,
                        old_decision_status: Some(status),
                        new_decision_status: None,
                    });
                    break;
                }
                "s" | "supersede" => {
                    let Some(title) = prompt_required_line(
                        input,
                        prompt_output,
                        "New decision title: ",
                        "title must not be empty",
                    )?
                    else {
                        quit = true;
                        break;
                    };
                    let Some(rationale) = prompt_required_line(
                        input,
                        prompt_output,
                        "New decision rationale: ",
                        "rationale must not be empty",
                    )?
                    else {
                        quit = true;
                        break;
                    };
                    let option_labels = prompt_line(
                        input,
                        prompt_output,
                        "New option labels, comma-separated (blank for default): ",
                    )?
                    .map(|line| split_review_list(&line))
                    .unwrap_or_default();
                    let chosen_option_label = prompt_line(
                        input,
                        prompt_output,
                        "Chosen option label (blank for none): ",
                    )?
                    .and_then(|line| non_empty_owned(&line));

                    let outcome = commands.supersede(
                        &cli.actor,
                        &item.decision_id,
                        &title,
                        &rationale,
                        &item.topic_keys,
                        &option_labels,
                        chosen_option_label.as_deref(),
                        &item.hypothesis_ids,
                        &item.evidence_ids,
                    )?;
                    let old_status =
                        decision_status_after_write(&ledger, &tenant_id, &item.decision_id)?;
                    let new_status =
                        decision_status_after_write(&ledger, &tenant_id, &outcome.new_decision_id)?;
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "superseded",
                        event_id: None,
                        proposal_event_id: Some(outcome.proposal_event_id),
                        superseded_event_id: Some(outcome.superseded_event_id),
                        new_decision_id: Some(outcome.new_decision_id),
                        old_decision_status: Some(old_status),
                        new_decision_status: Some(new_status),
                    });
                    break;
                }
                "" | "n" | "next" | "skip" => {
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "skipped",
                        event_id: None,
                        proposal_event_id: None,
                        superseded_event_id: None,
                        new_decision_id: None,
                        old_decision_status: Some(item.status),
                        new_decision_status: None,
                    });
                    break;
                }
                "q" | "quit" => {
                    quit = true;
                    break;
                }
                other => {
                    writeln!(
                        prompt_output,
                        "Unknown action '{other}'. Use a, d, s, n, or q."
                    )
                    .map_err(cli_io_error)?;
                }
            }
        }

        if quit {
            break;
        }
    }

    let reviewed_count = actions
        .iter()
        .filter(|action| action.action != "skipped")
        .count();
    let skipped_count = actions
        .iter()
        .filter(|action| action.action == "skipped")
        .count();

    format_review_output(
        cli.json,
        &ReviewCommandOutput {
            reviewer_actor_id: cli.actor.clone(),
            matched_count,
            reviewed_count,
            skipped_count,
            quit,
            truncated: response.truncated,
            next_cursor: response.data.next_cursor,
            unreviewed_only: args.unreviewed_only,
            reviewed_semantics: REVIEWED_SEMANTICS,
            actions,
        },
    )
}

fn run_import(cli: &Cli, import: &ImportArgs) -> Result<String> {
    match &import.command {
        ImportCommand::Documents(args) => {
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let scoped_ledger = TenantScopedLedger::new(&ledger, cli_tenant(cli)?);
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = import_documents(
                &scoped_ledger,
                &DocumentImportRequest {
                    paths,
                    importer_actor_id: cli.actor.clone(),
                    format: args.format.as_ingest_format(),
                    conflict_resolution: args.on_conflict.as_ingest_action(),
                },
            )?;
            format_import_output(cli.json, &report)
        }
        ImportCommand::PrepareDocuments(args) => {
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = prepare_document_texts(&DocumentPreparationRequest {
                paths,
                format: args.format.as_ingest_format(),
                output_dir: args.output_dir.clone(),
            })?;
            format_prepare_documents_output(cli.json, &report)
        }
    }
}

fn run_suggest(cli: &Cli, suggest: &SuggestArgs) -> Result<String> {
    match &suggest.command {
        SuggestCommand::DocumentCandidates(args) => {
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = propose_document_extraction_candidates(&DocumentCandidateRequest {
                paths,
                format: args.format.as_ingest_format(),
                extractor: document_candidate_extractor(args)?,
            })?;
            format_json_value(cli.json, &report)
        }
        SuggestCommand::MaterializeDocumentCandidates(args) => {
            let report = materialize_document_extraction_candidates(
                &DocumentCandidateMaterializationRequest {
                    input: args.input.clone(),
                    candidate_ids: args.candidate_ids.clone(),
                    output: args.output.clone(),
                    reviewed_by: cli.actor.clone(),
                },
            )?;
            format_json_value(cli.json, &report)
        }
    }
}

fn document_candidate_extractor(
    args: &SuggestDocumentCandidatesArgs,
) -> Result<DocumentCandidateExtractor> {
    match (&args.extractor_command, &args.llm_response) {
        (Some(_), Some(_)) => Err(CliError::InvalidInput(
            "use either --extractor-command or --llm-response, not both".to_owned(),
        )
        .into()),
        (Some(_), None) => Ok(DocumentCandidateExtractor::Command {
            args: args.extractor_args.clone(),
        }),
        (None, Some(path)) => {
            if !args.extractor_args.is_empty() {
                return Err(CliError::InvalidInput(
                    "--extractor-arg requires --extractor-command".to_owned(),
                )
                .into());
            }
            Ok(DocumentCandidateExtractor::ResponseFile(path.clone()))
        }
        (None, None) => Err(CliError::InvalidInput(
            "document-candidates requires --extractor-command or --llm-response".to_owned(),
        )
        .into()),
    }
}

fn propose_decision_from_option_labels<L: EventLedger>(
    commands: &Commands<'_, L>,
    actor_id: &str,
    args: &EmitDecisionProposedArgs,
) -> Result<String> {
    let mut option_ids = Vec::with_capacity(args.option_ids.len());
    let mut chosen_option_id = None;
    for option_label in &args.option_ids {
        let mut option_description =
            String::with_capacity("Option generated from CLI value ''".len() + option_label.len());
        let _ = write!(
            option_description,
            "Option generated from CLI value '{option_label}'"
        );
        let option_id = commands.record_option(actor_id, option_label, &option_description)?;
        if args.chosen_option_id.as_deref() == Some(option_label.as_str()) {
            chosen_option_id = Some(option_id.clone());
        }
        option_ids.push(option_id);
    }

    if args.chosen_option_id.is_some() && chosen_option_id.is_none() {
        return Err(CliError::InvalidInput(
            "--chose must match one of the values passed to --options".to_owned(),
        )
        .into());
    }

    commands.propose_decision(
        actor_id,
        &args.title,
        &args.rationale,
        &args.topic_keys,
        &option_ids,
        chosen_option_id.as_deref(),
        &args.hypothesis_ids,
        &args.evidence_ids,
    )
}

fn emit_actor_and_commands<'a>(
    cli: &Cli,
    ledger: &'a SqliteEventLedger,
    provenance_args: &EmitCaptureProvenanceArgs,
) -> Result<(String, Commands<'a, SqliteEventLedger>)> {
    if !provenance_args.has_override() {
        let commands = Commands::new_with_context(
            ledger,
            cli_command_context(cli, cli_emit_provenance(&cli.actor))?,
        );
        return Ok((cli.actor.clone(), commands));
    }

    let (actor_id, provenance) = capture_actor_and_provenance(provenance_args)?;
    let commands = Commands::new_with_context(ledger, cli_command_context(cli, provenance)?);
    Ok((actor_id, commands))
}

fn capture_actor_and_provenance(
    args: &EmitCaptureProvenanceArgs,
) -> Result<(String, EventProvenance)> {
    let actor_id = capture_actor_id(args)?;
    let provenance = capture_provenance(args, &actor_id)?;
    Ok((actor_id, provenance))
}

fn capture_actor_id(args: &EmitCaptureProvenanceArgs) -> Result<String> {
    if let Some(actor_id) = trimmed_optional("--actor-id", &args.actor_id)? {
        return Ok(actor_id.to_owned());
    }

    match args.source.unwrap_or(DecisionCaptureSource::Agent) {
        DecisionCaptureSource::Agent => {
            let tool = capture_agent_tool(args)?;
            let session = capture_agent_session(args, &tool)?;
            Ok(agent_actor_id(&tool, &session))
        }
        DecisionCaptureSource::Human => Ok(default_human_actor_id()),
    }
}

fn capture_agent_tool(args: &EmitCaptureProvenanceArgs) -> Result<String> {
    trimmed_optional("--agent-tool", &args.agent_tool).map(|value| {
        value
            .map(ToOwned::to_owned)
            .unwrap_or_else(default_agent_tool)
    })
}

fn capture_agent_session(args: &EmitCaptureProvenanceArgs, tool: &str) -> Result<String> {
    trimmed_optional("--agent-session", &args.agent_session).map(|value| {
        value
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_agent_session(tool))
    })
}

fn capture_provenance(args: &EmitCaptureProvenanceArgs, actor_id: &str) -> Result<EventProvenance> {
    let source = args.source.unwrap_or(DecisionCaptureSource::Agent);
    if let Some(source_ref) = trimmed_optional("--source-ref", &args.source_ref)? {
        return Ok(match source {
            DecisionCaptureSource::Agent => EventProvenance::agent(source_ref),
            DecisionCaptureSource::Human => EventProvenance::human(source_ref),
        });
    }

    Ok(match source {
        DecisionCaptureSource::Agent => EventProvenance::agent(actor_id),
        DecisionCaptureSource::Human => EventProvenance::human(actor_id),
    })
}

fn cli_emit_provenance(actor_id: &str) -> EventProvenance {
    if actor_id.trim().starts_with("human:") {
        EventProvenance::human(actor_id.trim().to_owned())
    } else {
        EventProvenance::cli()
    }
}

fn trimmed_required<'a>(field: &'static str, value: &'a str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(CliError::InvalidInput(format!("{field} must not be empty")).into())
    } else {
        Ok(trimmed)
    }
}

fn trimmed_optional<'a>(field: &'static str, value: &'a Option<String>) -> Result<Option<&'a str>> {
    match value.as_deref() {
        Some(raw) => Ok(Some(trimmed_required(field, raw)?)),
        None => Ok(None),
    }
}

fn run_query(cli: &Cli, query: &QueryArgs) -> Result<String> {
    let context = cli_query_context(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    if query.command.is_ledger_history_query() {
        let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
        return run_query_with_ledger(&scoped_ledger, query);
    }

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &context.tenant_id, &graph)?;
            run_query_with_graph(&context, &ledger, &graph, query)
        }
        GraphBackend::Kuzu => run_query_with_kuzu(&context, &ledger, &cli.hivemind_dir, query),
    }
}

impl QueryCommand {
    fn is_ledger_history_query(&self) -> bool {
        matches!(
            self,
            QueryCommand::RecentDecisions(_)
                | QueryCommand::GetRecentActivity(_)
                | QueryCommand::GetDecisionsChangedSince(_)
                | QueryCommand::GetDecisionsAddedSince(_)
                | QueryCommand::ExportReadOnlySummary(_)
        )
    }
}

fn render_compact_view_summary(view: &Option<CompactView>) -> String {
    let Some(v) = view else {
        return "decision not found".to_owned();
    };
    let mut out = format!(
        "CompactView: {} [{:?}]\n  rationale: {}\n",
        v.decision.id, v.decision.status, v.decision.rationale,
    );
    if let Some(chain) = &v.supersession_chain {
        out.push_str(&format!(
            "  superseded {} earlier decision(s); oldest: {}\n",
            chain.chain_length - 1,
            chain.oldest_id
        ));
    }
    if let Some(contest) = &v.contest {
        out.push_str(&format!(
            "  CONTESTED: accepted_by={:?} rejected_by={:?}\n",
            contest.accepted_by, contest.rejected_by
        ));
    }
    out.push_str(&format!("  hypotheses: {}\n", v.hypotheses.len()));
    out.push_str(&format!("  evidence_ids: {}\n", v.evidence_ids.len()));
    out.push_str(&format!("  active_blockers: {}\n", v.active_blockers.len()));
    out.push_str(&format!(
        "  elided: {} superseded, {} unchosen options\n",
        v.elided.superseded_decision_count, v.elided.unchosen_option_count
    ));
    out
}

fn run_query_with_ledger(ledger: &impl EventLedger, query: &QueryArgs) -> Result<String> {
    let output = match &query.command {
        QueryCommand::RecentDecisions(args) => {
            let response = get_recent_decisions(ledger, &recent_decisions_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_recent_decisions_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetRecentActivity(args) => {
            let response = get_recent_activity(ledger, &recent_activity_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_recent_activity_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetDecisionsChangedSince(args) => {
            let response = get_decisions_changed_since(ledger, &changed_since_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_changed_since_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetDecisionsAddedSince(args) => {
            let response = get_decisions_added_since(ledger, &added_since_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_added_since_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::ExportReadOnlySummary(args) => {
            let request = export_read_only_summary_request(args)?;
            let response = export_read_only_summary(ledger, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_read_only_export_summary,
                response.data.continuation_cursor.as_deref(),
            )?
        }
        QueryCommand::GetDecision(_)
        | QueryCommand::GetRelevantDecisions(_)
        | QueryCommand::GetSupersessionChain(_)
        | QueryCommand::GetDecisionNeighborhood(_)
        | QueryCommand::GetCompactView(_)
        | QueryCommand::Search(_)
        | QueryCommand::SearchDecisions(_)
        | QueryCommand::Recall(_)
        | QueryCommand::GetActiveDecisionBlockers(_)
        | QueryCommand::GetBlockerNotificationCandidates(_) => {
            return Err(
                CliError::InvalidInput("query requires graph-backed execution".to_owned()).into(),
            )
        }
    };

    Ok(output)
}

const REVIEWED_SEMANTICS: &str =
    "derived from reviewer-authored decision.accepted, decision.rejected, or decision.superseded events";

fn review_recent_decisions_request(args: &ReviewArgs) -> Result<RecentDecisionsRequest> {
    let now = parse_utc_timestamp("--now", &args.now)?;
    let timezone = TimeZoneSpec::parse(&args.timezone)?;
    let since_timestamp =
        resolve_diff_bound("--since", Some(args.since.as_str()), None, now, timezone)?
            .ok_or_else(|| CliError::InvalidInput("--since must not be empty".to_owned()))?;
    let until_timestamp =
        resolve_diff_bound("--until", args.until.as_deref(), None, now, timezone)?;

    Ok(RecentDecisionsRequest {
        since_timestamp,
        until_timestamp,
        filters: RecentDecisionFilterRequest {
            actor_patterns: args.actor_patterns.clone(),
            sources: Vec::new(),
            topic_keys: Vec::new(),
            statuses: Vec::new(),
        },
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn read_ledger_events(ledger: &impl EventLedger) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    ledger.replay_from(0, &mut |event| {
        events.push(event.clone());
        Ok(())
    })?;
    Ok(events)
}

fn reviewed_decision_ids_by_actor(
    events: &[Event],
    reviewer_actor_id: &str,
) -> Result<BTreeSet<String>> {
    let mut reviewed = BTreeSet::new();
    for event in events
        .iter()
        .filter(|event| event.actor_id == reviewer_actor_id)
    {
        match validated_payload(event)? {
            EventPayload::DecisionAccepted(payload) => {
                reviewed.insert(payload.decision_id);
            }
            EventPayload::DecisionRejected(payload) => {
                reviewed.insert(payload.decision_id);
            }
            EventPayload::DecisionSuperseded(payload) => {
                reviewed.insert(payload.old_decision_id);
            }
            EventPayload::DecisionProposed(_)
            | EventPayload::DecisionRequested(_)
            | EventPayload::EvidenceRecorded(_)
            | EventPayload::HypothesisRecorded(_)
            | EventPayload::RelationAdded(_)
            | EventPayload::BlockerReported(_)
            | EventPayload::BlockerResolved(_)
            | EventPayload::NotificationSent(_)
            | EventPayload::NotificationAcknowledged(_)
            | EventPayload::IngestBatchReceived(_)
            | EventPayload::IngestBatchClassified(_)
            | EventPayload::DecisionScored(_) => {}
        }
    }
    Ok(reviewed)
}

#[derive(Debug, Default)]
struct ReviewLedgerContext {
    evidence: BTreeMap<String, String>,
    hypotheses: BTreeMap<String, String>,
}

impl ReviewLedgerContext {
    fn from_events(events: &[Event]) -> Result<Self> {
        let mut context = Self::default();
        for event in events {
            match validated_payload(event)? {
                EventPayload::EvidenceRecorded(payload) => {
                    context
                        .evidence
                        .insert(payload.evidence_id, payload.content);
                }
                EventPayload::HypothesisRecorded(payload) => {
                    context
                        .hypotheses
                        .insert(payload.hypothesis_id, payload.statement);
                }
                EventPayload::DecisionProposed(_)
                | EventPayload::DecisionRequested(_)
                | EventPayload::DecisionAccepted(_)
                | EventPayload::DecisionRejected(_)
                | EventPayload::DecisionSuperseded(_)
                | EventPayload::RelationAdded(_)
                | EventPayload::BlockerReported(_)
                | EventPayload::BlockerResolved(_)
                | EventPayload::NotificationSent(_)
                | EventPayload::NotificationAcknowledged(_)
                | EventPayload::IngestBatchReceived(_)
                | EventPayload::IngestBatchClassified(_)
                | EventPayload::DecisionScored(_) => {}
            }
        }
        Ok(context)
    }
}

fn render_review_item<W: IoWrite>(
    output: &mut W,
    index: usize,
    total: usize,
    item: &RecentDecisionEntry,
    context: &ReviewLedgerContext,
) -> Result<()> {
    writeln!(output, "\n[{index}/{total}] {}", item.decision_id).map_err(cli_io_error)?;
    writeln!(output, "Title: {}", item.title).map_err(cli_io_error)?;
    writeln!(output, "Status: {}", decision_status_label(item.status)).map_err(cli_io_error)?;
    writeln!(output, "Actors: {}", display_review_list(&item.actor_ids)).map_err(cli_io_error)?;
    writeln!(output, "Topics: {}", display_review_list(&item.topic_keys)).map_err(cli_io_error)?;
    writeln!(output, "Rationale: {}", item.rationale).map_err(cli_io_error)?;
    writeln!(output, "Options:").map_err(cli_io_error)?;
    if item.option_ids.is_empty() {
        writeln!(output, "  - <none>").map_err(cli_io_error)?;
    } else {
        for option_id in &item.option_ids {
            if item.chosen_option_id.as_deref() == Some(option_id.as_str()) {
                writeln!(output, "  - {option_id} (chosen)").map_err(cli_io_error)?;
            } else {
                writeln!(output, "  - {option_id}").map_err(cli_io_error)?;
            }
        }
    }
    writeln!(output, "Evidence:").map_err(cli_io_error)?;
    if item.evidence_ids.is_empty() {
        writeln!(output, "  - <none>").map_err(cli_io_error)?;
    } else {
        for evidence_id in &item.evidence_ids {
            match context.evidence.get(evidence_id) {
                Some(content) => {
                    writeln!(output, "  - {evidence_id}: {content}").map_err(cli_io_error)?
                }
                None => writeln!(output, "  - {evidence_id}").map_err(cli_io_error)?,
            }
        }
    }
    writeln!(output, "Hypotheses:").map_err(cli_io_error)?;
    if item.hypothesis_ids.is_empty() {
        writeln!(output, "  - <none>").map_err(cli_io_error)?;
    } else {
        for hypothesis_id in &item.hypothesis_ids {
            match context.hypotheses.get(hypothesis_id) {
                Some(statement) => {
                    writeln!(output, "  - {hypothesis_id}: {statement}").map_err(cli_io_error)?
                }
                None => writeln!(output, "  - {hypothesis_id}").map_err(cli_io_error)?,
            }
        }
    }
    Ok(())
}

fn prompt_line<R: BufRead, W: IoWrite>(
    input: &mut R,
    output: &mut W,
    prompt: &str,
) -> Result<Option<String>> {
    write!(output, "{prompt}").map_err(cli_io_error)?;
    output.flush().map_err(cli_io_error)?;
    let mut line = String::new();
    let bytes_read = input.read_line(&mut line).map_err(cli_io_error)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_owned()))
}

fn prompt_required_line<R: BufRead, W: IoWrite>(
    input: &mut R,
    output: &mut W,
    prompt: &str,
    empty_message: &str,
) -> Result<Option<String>> {
    loop {
        let Some(value) = prompt_line(input, output, prompt)? else {
            return Ok(None);
        };
        if let Some(value) = non_empty_owned(&value) {
            return Ok(Some(value));
        }
        writeln!(output, "{empty_message}").map_err(cli_io_error)?;
    }
}

fn split_review_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(non_empty_owned)
        .collect::<Vec<_>>()
}

fn non_empty_owned(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn display_review_list(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_owned()
    } else {
        values.join(",")
    }
}

fn validated_payload(event: &Event) -> Result<EventPayload> {
    crate::events::validate(event).map_err(|error| {
        CliError::InvalidInput(format!(
            "ledger event {} failed validation during review: {error}",
            event.event_id.unwrap_or_default()
        ))
        .into()
    })
}

fn cli_io_error(error: io::Error) -> HivemindError {
    CliError::InvalidInput(format!("interactive review I/O failed: {error}")).into()
}

fn added_since_request(args: &QueryAddedSinceArgs) -> Result<DecisionsAddedSinceRequest> {
    let now = parse_utc_timestamp("--now", &args.now)?;
    let timezone = TimeZoneSpec::parse(&args.timezone)?;
    let since_timestamp = resolve_diff_bound(
        "--since",
        args.since.as_deref(),
        args.since_timestamp.as_deref(),
        now,
        timezone,
    )?;
    let until_timestamp = resolve_diff_bound(
        "--until",
        args.until.as_deref(),
        args.until_timestamp.as_deref(),
        now,
        timezone,
    )?;

    Ok(DecisionsAddedSinceRequest {
        since_offset: args.since_offset,
        since_timestamp,
        until_offset: args.until_offset,
        until_timestamp,
        filters: DecisionsAddedSinceFilterRequest {
            actor_ids: args.filters.actor_ids.clone(),
            sources: args.filters.sources.clone(),
            source_refs: args.filters.source_refs.clone(),
            import_run_ids: args.import_run_ids.clone(),
            topic_keys: args.filters.topic_keys.clone(),
            statuses: args
                .filters
                .statuses
                .iter()
                .copied()
                .map(QueryDecisionStatus::as_decision_status)
                .collect(),
        },
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn recent_decisions_request(args: &QueryRecentDecisionsArgs) -> Result<RecentDecisionsRequest> {
    let now = parse_utc_timestamp("--now", &args.now)?;
    let timezone = TimeZoneSpec::parse(&args.timezone)?;
    let since_timestamp =
        resolve_diff_bound("--since", Some(args.since.as_str()), None, now, timezone)?
            .ok_or_else(|| CliError::InvalidInput("--since must not be empty".to_owned()))?;
    let until_timestamp =
        resolve_diff_bound("--until", args.until.as_deref(), None, now, timezone)?;

    Ok(RecentDecisionsRequest {
        since_timestamp,
        until_timestamp,
        filters: RecentDecisionFilterRequest {
            actor_patterns: args.actor_patterns.clone(),
            sources: args.sources.clone(),
            topic_keys: args.topic_keys.clone(),
            statuses: args
                .statuses
                .iter()
                .copied()
                .map(QueryDecisionStatus::as_decision_status)
                .collect(),
        },
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn render_recent_decisions_summary(results: &RecentDecisionsResults) -> String {
    if results.items.is_empty() {
        return "No recent decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let timestamp = item
            .creation
            .ts
            .map(|ts| ts.to_rfc3339())
            .unwrap_or_else(|| "unknown-ts".to_owned());
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}\tactor={}\tsource={}\tcitation={}",
            timestamp,
            decision_status_label(item.status),
            item.decision_id,
            summary_cell(&item.title),
            item.actor_ids.join(","),
            item.creation.source.as_str(),
            item.creation.citation_id
        );
    }
    output.trim_end().to_owned()
}

fn render_recent_activity_summary(results: &RecentActivityResults) -> String {
    if results.items.is_empty() {
        return "No recent activity found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\tactor={}\tsource={}\tdecisions={}\tcitation={}",
            item.event_origin,
            change_kind_label(item.change_kind),
            event_type_label(item.event_type),
            item.actor_id,
            item.source.as_str(),
            item.decision_ids.join(","),
            item.citation_id
        );
    }
    output.trim_end().to_owned()
}

fn render_changed_since_summary(results: &DecisionsChangedSinceResults) -> String {
    if results.items.is_empty() {
        return "No changed decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\tactor={}\tsource={}\tdecisions={}\tcitation={}",
            item.event_origin,
            change_kind_label(item.change_kind),
            event_type_label(item.event_type),
            item.actor_id,
            item.source.as_str(),
            item.decision_ids.join(","),
            item.citation_id
        );
    }
    output.trim_end().to_owned()
}

fn render_added_since_summary(results: &DecisionsAddedSinceResults) -> String {
    if results.added_decisions.is_empty() && results.changed_existing_decisions.is_empty() {
        return "No added or changed decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.added_decisions {
        let _ = writeln!(
            output,
            "added\t{}\t{}\ttopics={}\tcitation={}\tchanges={}",
            decision_status_label(item.status),
            item.decision_id,
            item.topic_keys.join(","),
            item.creation.citation_id,
            item.changes_in_window.len()
        );
    }
    for item in &results.changed_existing_decisions {
        let _ = writeln!(
            output,
            "changed\t{}\t{}\ttopics={}\tchanges={}",
            decision_status_label(item.status),
            item.decision_id,
            item.topic_keys.join(","),
            item.changes_in_window.len()
        );
    }
    output.trim_end().to_owned()
}

fn render_read_only_export_summary(export: &ReadOnlyExport) -> String {
    if let Some(markdown) = &export.markdown {
        return markdown.trim_end().to_owned();
    }

    format!(
        "read_only_export\tquery={}\tformat={}\tresult_count={}\ttruncated={}\tcitations={}",
        read_only_query_label(export.query),
        read_only_format_label(export.format),
        export.result_count,
        export.truncated,
        export.citation_map.len()
    )
}

fn format_query_response<T: Serialize>(
    summary: bool,
    response: &QueryResponse<T>,
    render_summary: impl FnOnce(&T) -> String,
    next_cursor: Option<&str>,
) -> Result<String> {
    if !summary {
        return format_json_value(true, response);
    }

    let mut output = render_summary(&response.data);
    append_truncation_notice(&mut output, response.truncated, next_cursor);
    Ok(output.trim_end().to_owned())
}

fn append_truncation_notice(output: &mut String, truncated: bool, next_cursor: Option<&str>) {
    if !truncated {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    match next_cursor {
        Some(cursor) => {
            let _ = write!(
                output,
                "truncated=true next_cursor={}",
                summary_cell(cursor)
            );
        }
        None => output.push_str("truncated=true"),
    }
}

fn render_decision_summary(decision: &Option<DecisionView>) -> String {
    let Some(decision) = decision else {
        return "No decision found".to_owned();
    };

    let mut output = String::new();
    write_decision_summary_row(&mut output, "decision", decision);
    output.trim_end().to_owned()
}

fn render_decision_list_summary(decisions: &[DecisionView]) -> String {
    if decisions.is_empty() {
        return "No decisions found".to_owned();
    }

    let mut output = String::new();
    for decision in decisions {
        write_decision_summary_row(&mut output, "decision", decision);
    }
    output.trim_end().to_owned()
}

fn render_search_summary(results: &DecisionSearchResults) -> String {
    if results.items.is_empty() {
        return "No matching decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "match\trank={}\t{}\t{}\t{}\ttopics={}\tmatched={}",
            item.rank,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            item.decision.topic_keys.join(","),
            item.matched_fields.join(",")
        );
    }
    output.trim_end().to_owned()
}

fn render_recall_summary(response: &crate::summarize::RecallResponse) -> String {
    let mut output = String::new();
    if response.ranked.items.is_empty() {
        return "No decisions found matching the query.".to_owned();
    }
    let _ = writeln!(output, "digest\t{}", summary_cell(&response.digest.summary));
    let _ = writeln!(
        output,
        "cited\t{}",
        response.digest.cited_decision_ids.join(",")
    );
    for item in &response.ranked.items {
        let _ = writeln!(
            output,
            "match\trank={}\t{}\t{}\t{}\ttopics={}",
            item.rank,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            item.decision.topic_keys.join(","),
        );
    }
    output.trim_end().to_owned()
}

fn render_supersession_summary(chain: &SupersessionChain) -> String {
    if chain.decision_ids.is_empty() {
        return "No supersession chain found".to_owned();
    }

    let mut output = String::new();
    for (index, decision_id) in chain.decision_ids.iter().enumerate() {
        let marker = if index == chain.input_index {
            "input"
        } else {
            "chain"
        };
        let _ = writeln!(output, "{marker}\t{index}\t{decision_id}");
    }
    output.trim_end().to_owned()
}

fn render_neighborhood_summary(neighborhood: &NeighborhoodView) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "root\t{}\t{}\tpresent={}\tnodes={}\tedges={}",
        neighborhood.root.kind.table_name(),
        neighborhood.root.id,
        neighborhood.root.present,
        neighborhood.nodes.len(),
        neighborhood.edges.len()
    );
    for node in &neighborhood.nodes {
        let status = match (node.decision_status, node.hypothesis_status) {
            (Some(status), _) => decision_status_label(status),
            (None, Some(status)) => hypothesis_status_label(status),
            (None, None) => "",
        };
        let _ = writeln!(
            output,
            "node\t{}\t{}\tstatus={}",
            node.kind.table_name(),
            node.id,
            status
        );
    }
    for edge in &neighborhood.edges {
        match edge.event_origin {
            Some(event_origin) => {
                let _ = writeln!(
                    output,
                    "edge\t{}\t{}\t{}\tevent_origin={}",
                    edge.relation.table_name(),
                    edge.from,
                    edge.to,
                    event_origin
                );
            }
            None => {
                let _ = writeln!(
                    output,
                    "edge\t{}\t{}\t{}\tevent_origin=unknown",
                    edge.relation.table_name(),
                    edge.from,
                    edge.to
                );
            }
        }
    }
    output.trim_end().to_owned()
}

fn render_active_blockers_summary(results: &DecisionBlockerResults) -> String {
    if results.items.is_empty() {
        return "No active decision blockers found".to_owned();
    }

    let mut output = String::new();
    for blocker in &results.items {
        let decision_id = match &blocker.decision_id {
            Some(decision_id) => decision_id.as_str(),
            None => "",
        };
        let _ = writeln!(
            output,
            "blocker\t{}\tdecision={}\tpriority={}\tstale={}\tblocked_actor={}\t{}",
            blocker.id,
            decision_id,
            blocker.priority.as_str(),
            blocker.stale,
            blocker.blocked_actor_id,
            summary_cell(&blocker.reason)
        );
    }
    output.trim_end().to_owned()
}

fn render_blocker_notifications_summary(candidates: &BlockerNotificationCandidates) -> String {
    if candidates.items.is_empty() {
        return "No blocker notification candidates found".to_owned();
    }

    let mut output = String::new();
    for candidate in &candidates.items {
        let decision_id = match &candidate.decision_id {
            Some(decision_id) => decision_id.as_str(),
            None => "",
        };
        let _ = writeln!(
            output,
            "notification\tblocker={}\tdecision={}\tpriority={}\trecipient={}\tchannel={}",
            candidate.blocker_id,
            decision_id,
            candidate.priority.as_str(),
            candidate.recipient_actor_id,
            candidate.channel
        );
    }
    output.trim_end().to_owned()
}

fn write_decision_summary_row(output: &mut String, prefix: &str, decision: &DecisionView) {
    let _ = writeln!(
        output,
        "{}\t{}\t{}\t{}\ttopics={}",
        prefix,
        decision_status_label(decision.status),
        decision.id,
        summary_cell(&decision.title),
        decision.topic_keys.join(",")
    );
}

fn summary_cell(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

fn decision_status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn hypothesis_status_label(status: HypothesisStatus) -> &'static str {
    match status {
        HypothesisStatus::Open => "open",
        HypothesisStatus::Supported => "supported",
        HypothesisStatus::Refuted => "refuted",
    }
}

fn event_type_label(event_type: EventType) -> &'static str {
    match event_type {
        EventType::DecisionProposed => "decision.proposed",
        EventType::DecisionRequested => "decision.requested",
        EventType::DecisionAccepted => "decision.accepted",
        EventType::DecisionRejected => "decision.rejected",
        EventType::DecisionSuperseded => "decision.superseded",
        EventType::EvidenceRecorded => "evidence.recorded",
        EventType::HypothesisRecorded => "hypothesis.recorded",
        EventType::RelationAdded => "relation.added",
        EventType::BlockerReported => "blocker.reported",
        EventType::BlockerResolved => "blocker.resolved",
        EventType::NotificationSent => "notification.sent",
        EventType::NotificationAcknowledged => "notification.acknowledged",
        EventType::IngestBatchReceived => "ingest.batch_received",
        EventType::IngestBatchClassified => "ingest.batch_classified",
        EventType::DecisionScored => "decision.scored",
    }
}

fn change_kind_label(kind: HistoryChangeKind) -> &'static str {
    match kind {
        HistoryChangeKind::NewDecision => "new_decision",
        HistoryChangeKind::StatusChange => "status_change",
        HistoryChangeKind::NewEvidence => "new_evidence",
        HistoryChangeKind::RefutedAssumption => "refuted_assumption",
        HistoryChangeKind::Supersession => "supersession",
        HistoryChangeKind::ContextChange => "context_change",
    }
}

fn read_only_query_label(query: ReadOnlyExportQueryKind) -> &'static str {
    match query {
        ReadOnlyExportQueryKind::RecentActivity => "recent_activity",
        ReadOnlyExportQueryKind::DecisionsChangedSince => "decisions_changed_since",
    }
}

fn read_only_format_label(format: QueryReadOnlyExportFormat) -> &'static str {
    match format {
        QueryReadOnlyExportFormat::Json => "json",
        QueryReadOnlyExportFormat::Markdown => "markdown",
    }
}

#[derive(Debug, Clone, Copy)]
enum TimeZoneSpec {
    Utc,
}

impl TimeZoneSpec {
    fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "UTC" | "utc" | "Etc/UTC" => Ok(Self::Utc),
            other => Err(CliError::InvalidInput(format!(
                "--timezone {other} is not supported in slice 1; only UTC is accepted"
            ))
            .into()),
        }
    }
}

fn resolve_diff_bound(
    flag: &'static str,
    raw: Option<&str>,
    explicit_ts: Option<&str>,
    now: Option<DateTime<Utc>>,
    timezone: TimeZoneSpec,
) -> Result<Option<DateTime<Utc>>> {
    if let Some(ts) = explicit_ts {
        return parse_utc_timestamp(flag, &Some(ts.to_owned()));
    }
    let Some(value) = raw.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(Some(parsed.with_timezone(&Utc)));
    }
    if let Some(parsed) = parse_utc_date(value) {
        return Ok(Some(parsed));
    }
    let now = now.unwrap_or_else(Utc::now);
    if let Some(resolved) = resolve_relative_duration(value, now) {
        return Ok(Some(resolved));
    }
    let resolved = resolve_relative_phrase(value, now, timezone).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "{flag} must be an RFC3339 timestamp, YYYY-MM-DD date, duration like 7d/24h, or supported phrase (got: {value})"
        ))
    })?;
    Ok(Some(resolved))
}

fn parse_utc_date(value: &str) -> Option<DateTime<Utc>> {
    use chrono::{NaiveDate, NaiveTime, TimeZone};
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .map(|date| Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN)))
}

fn resolve_relative_duration(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (amount, unit) = value.split_at(value.len().checked_sub(1)?);
    let amount = amount.parse::<i64>().ok()?;
    if amount < 0 {
        return None;
    }
    let duration = match unit.to_ascii_lowercase().as_str() {
        "s" => chrono::Duration::seconds(amount),
        "m" => chrono::Duration::minutes(amount),
        "h" => chrono::Duration::hours(amount),
        "d" => chrono::Duration::days(amount),
        "w" => chrono::Duration::weeks(amount),
        _ => return None,
    };
    now.checked_sub_signed(duration)
}

fn resolve_relative_phrase(
    phrase: &str,
    now: DateTime<Utc>,
    timezone: TimeZoneSpec,
) -> Option<DateTime<Utc>> {
    let normalized = phrase.trim().to_ascii_lowercase();
    let TimeZoneSpec::Utc = timezone;
    match normalized.as_str() {
        "now" => Some(now),
        "last week" | "last_week" | "last-week" => Some(start_of_previous_iso_week_utc(now)),
        "this week" | "this_week" | "this-week" => Some(start_of_current_iso_week_utc(now)),
        "yesterday" => Some(start_of_day_utc(now) - chrono::Duration::days(1)),
        "today" => Some(start_of_day_utc(now)),
        _ => None,
    }
}

fn start_of_day_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::NaiveTime;
    use chrono::TimeZone;
    Utc.from_utc_datetime(&now.date_naive().and_time(NaiveTime::MIN))
}

fn start_of_current_iso_week_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::{Datelike, NaiveTime, TimeZone};
    let date = now.date_naive();
    let days_from_monday = i64::from(date.weekday().num_days_from_monday());
    let monday = date - chrono::Duration::days(days_from_monday);
    Utc.from_utc_datetime(&monday.and_time(NaiveTime::MIN))
}

fn start_of_previous_iso_week_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    start_of_current_iso_week_utc(now) - chrono::Duration::days(7)
}

fn recent_activity_request(args: &QueryRecentActivityArgs) -> Result<RecentActivityRequest> {
    Ok(RecentActivityRequest {
        filters: history_filter_request(&args.filters),
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn changed_since_request(args: &QueryChangedSinceArgs) -> Result<ChangedSinceRequest> {
    Ok(ChangedSinceRequest {
        since_offset: args.since_offset,
        since_timestamp: parse_utc_timestamp("--since-ts", &args.since_timestamp)?,
        until_offset: args.until_offset,
        until_timestamp: parse_utc_timestamp("--until-ts", &args.until_timestamp)?,
        filters: history_filter_request(&args.filters),
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn export_read_only_summary_request(
    args: &QueryExportReadOnlySummaryArgs,
) -> Result<ReadOnlyExportRequest> {
    let generated_at =
        parse_utc_timestamp("--generated-at", &args.generated_at)?.unwrap_or_else(Utc::now);
    let filters = history_filter_request(&args.filters);
    let query = match args.query {
        QueryExportKind::RecentActivity => {
            ReadOnlyExportQuery::RecentActivity(RecentActivityRequest {
                filters,
                limit: args.limit,
                cursor: args.cursor.clone(),
            })
        }
        QueryExportKind::DecisionsChangedSince => {
            ReadOnlyExportQuery::DecisionsChangedSince(ChangedSinceRequest {
                since_offset: args.since_offset,
                since_timestamp: parse_utc_timestamp("--since-ts", &args.since_timestamp)?,
                until_offset: args.until_offset,
                until_timestamp: parse_utc_timestamp("--until-ts", &args.until_timestamp)?,
                filters,
                limit: args.limit,
                cursor: args.cursor.clone(),
            })
        }
    };

    Ok(ReadOnlyExportRequest {
        query,
        format: args.format.as_query_format(),
        generated_at,
    })
}

fn history_filter_request(args: &QueryHistoryFilterArgs) -> HistoryFilterRequest {
    HistoryFilterRequest {
        actor_ids: args.actor_ids.clone(),
        sources: args.sources.clone(),
        source_refs: args.source_refs.clone(),
        topic_keys: args.topic_keys.clone(),
        statuses: args
            .statuses
            .iter()
            .copied()
            .map(QueryDecisionStatus::as_decision_status)
            .collect(),
    }
}

fn parse_utc_timestamp(
    field: &'static str,
    value: &Option<String>,
) -> Result<Option<DateTime<Utc>>> {
    match value.as_deref() {
        None => Ok(None),
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
            .map_err(|error| {
                CliError::InvalidInput(format!("{field} must be an RFC 3339 timestamp: {error}"))
                    .into()
            }),
    }
}

fn run_query_with_graph(
    context: &QueryContext,
    ledger: &SqliteEventLedger,
    graph: &impl GraphView,
    query: &QueryArgs,
) -> Result<String> {
    let output = match &query.command {
        QueryCommand::GetDecision(args) => {
            let response = get_decision(graph, &args.decision_id)?;
            format_query_response(query.summary, &response, render_decision_summary, None)?
        }
        QueryCommand::GetRelevantDecisions(args) => {
            let response = get_relevant_decisions(
                graph,
                &args.topic,
                args.status.map(QueryDecisionStatus::as_decision_status),
            )?;
            format_query_response(
                query.summary,
                &response,
                |decisions| render_decision_list_summary(decisions),
                None,
            )?
        }
        QueryCommand::GetSupersessionChain(args) => {
            let response = get_supersession_chain(graph, &args.decision_id)?;
            format_query_response(query.summary, &response, render_supersession_summary, None)?
        }
        QueryCommand::GetDecisionNeighborhood(args) => {
            if args.compact {
                let response = get_compact_view(graph, &args.decision_id)?;
                format_query_response(query.summary, &response, render_compact_view_summary, None)?
            } else {
                if args.depth != 1 {
                    return Err(CliError::InvalidInput(format!(
                        "--depth {} is not supported yet; slice-1 only supports depth=1 with hypothesis SUPPORTS/REFUTES auto-expanded",
                        args.depth
                    ))
                    .into());
                }
                let request = if args.relations.is_empty() {
                    NeighborhoodRequest::all()
                } else {
                    NeighborhoodRequest::with_relations(
                        args.relations
                            .iter()
                            .copied()
                            .map(QueryRelationKind::as_graph_relation),
                    )
                };
                let response = get_decision_neighborhood(graph, &args.decision_id, &request)?;
                format_query_response(query.summary, &response, render_neighborhood_summary, None)?
            }
        }
        QueryCommand::GetCompactView(args) => {
            let response = get_compact_view(graph, &args.decision_id)?;
            format_query_response(query.summary, &response, render_compact_view_summary, None)?
        }
        QueryCommand::Search(args) => {
            let request = search_decision_request(args)?;
            let response = search_decisions_fts_with_context(context, ledger, graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_search_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::SearchDecisions(args) => {
            let request = search_decision_request(args)?;
            let response = search_decisions_fts_with_context(context, ledger, graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_search_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::Recall(args) => {
            let limit = args.limit.clamp(1, RECALL_MAX_LIMIT);
            let request = RecallRequest {
                q: args.query.clone(),
                topic_keys: args.topic_keys.clone(),
                statuses: args
                    .statuses
                    .iter()
                    .copied()
                    .map(QueryDecisionStatus::as_decision_status)
                    .collect(),
                actor_ids: args.actor_ids.clone(),
                sources: args.sources.clone(),
                since: parse_query_datetime(args.since.as_deref(), "--since")?,
                until: parse_query_datetime(args.until.as_deref(), "--until")?,
                limit,
                cursor: args.cursor.clone(),
            };
            let response = recall_decisions(context, ledger, graph, &request)?;
            format_query_response(query.summary, &response, render_recall_summary, None)?
        }
        QueryCommand::GetActiveDecisionBlockers(args) => {
            let request = ActiveDecisionBlockersRequest {
                filters: DecisionBlockerFilters {
                    decision_ids: args.decision_ids.clone(),
                    topic_keys: args.topic_keys.clone(),
                    required_owner_ids: args.required_owner_ids.clone(),
                    blocked_actor_ids: args.blocked_actor_ids.clone(),
                    priorities: args
                        .priorities
                        .iter()
                        .copied()
                        .map(QueryBlockerPriority::as_blocker_priority)
                        .collect(),
                    now: parse_query_datetime(args.now.as_deref(), "--now")?,
                    stale_after_seconds: args.stale_after_seconds,
                },
                limit: args.limit,
                cursor: args.cursor.clone(),
            };
            let response = get_active_decision_blockers(graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_active_blockers_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetBlockerNotificationCandidates(args) => {
            let request = BlockerNotificationCandidatesRequest {
                now: parse_required_query_datetime(&args.now, "--now")?,
                policy_version: args.policy_version.clone(),
                limit: args.limit,
                cursor: args.cursor.clone(),
            };
            let response = get_blocker_notification_candidates(graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_blocker_notifications_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::RecentDecisions(_)
        | QueryCommand::GetRecentActivity(_)
        | QueryCommand::GetDecisionsChangedSince(_)
        | QueryCommand::GetDecisionsAddedSince(_)
        | QueryCommand::ExportReadOnlySummary(_) => {
            return Err(
                CliError::InvalidInput("query requires ledger-backed execution".to_owned()).into(),
            )
        }
    };

    Ok(output)
}

fn parse_query_datetime(value: Option<&str>, flag: &str) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| parse_required_query_datetime(value, flag))
        .transpose()
}

fn search_decision_request(args: &QuerySearchDecisionsArgs) -> Result<SearchDecisionRequest> {
    Ok(SearchDecisionRequest {
        query: args.query.clone(),
        topic_keys: args.topic_keys.clone(),
        statuses: args
            .statuses
            .iter()
            .copied()
            .map(QueryDecisionStatus::as_decision_status)
            .collect(),
        actor_ids: args.actor_ids.clone(),
        sources: args.sources.clone(),
        since: parse_query_datetime(args.since.as_deref(), "--since")?,
        until: parse_query_datetime(args.until.as_deref(), "--until")?,
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn parse_required_query_datetime(value: &str, flag: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            CliError::InvalidInput(format!("{flag} must be an RFC3339 timestamp: {error}")).into()
        })
}

fn run_dump(cli: &Cli, dump: &DumpArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
            run_dump_with_graph(&graph, dump)
        }
        GraphBackend::Kuzu => run_dump_with_kuzu(&tenant_id, &ledger, &cli.hivemind_dir, dump),
    }
}

#[cfg(feature = "tui")]
fn run_tui(cli: &Cli, args: &TuiArgs) -> Result<String> {
    if cli.json {
        return Err(CliError::InvalidInput(
            "--json is not supported for the interactive tui command".to_owned(),
        )
        .into());
    }

    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let config = crate::tui::TuiConfig {
        query: args.query.clone(),
        topic_keys: args.topic_keys.clone(),
        statuses: args
            .statuses
            .iter()
            .copied()
            .map(QueryDecisionStatus::as_decision_status)
            .collect(),
        actor_ids: args.actor_ids.clone(),
        sources: args.sources.clone(),
        limit: args.limit,
        dot_output: args.dot_output.clone(),
    };

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
            crate::tui::run(&graph, config)?;
        }
        GraphBackend::Kuzu => run_tui_with_kuzu(&tenant_id, &ledger, &cli.hivemind_dir, config)?,
    }

    Ok("tui exited".to_owned())
}

#[cfg(not(feature = "tui"))]
fn run_tui(_cli: &Cli, _args: &TuiArgs) -> Result<String> {
    Err(
        CliError::InvalidInput("tui command requires building with --features tui".to_owned())
            .into(),
    )
}

#[cfg(all(feature = "tui", feature = "graph-kuzu"))]
fn run_tui_with_kuzu(
    tenant_id: &TenantId,
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    config: crate::tui::TuiConfig,
) -> Result<()> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    crate::tui::run(&graph, config)
}

#[cfg(all(feature = "tui", not(feature = "graph-kuzu")))]
fn run_tui_with_kuzu(
    _tenant_id: &TenantId,
    _ledger: &impl EventLedger,
    _hivemind_dir: &std::path::Path,
    _config: crate::tui::TuiConfig,
) -> Result<()> {
    Err(CliError::InvalidInput(
        "graph backend 'kuzu' requires building with --features graph-kuzu".to_owned(),
    )
    .into())
}

fn run_dump_with_graph(graph: &impl GraphView, dump: &DumpArgs) -> Result<String> {
    match dump.format {
        DumpFormat::Dot => render_dot(graph),
    }
}

#[cfg(feature = "graph-kuzu")]
fn run_query_with_kuzu(
    context: &QueryContext,
    ledger: &SqliteEventLedger,
    hivemind_dir: &std::path::Path,
    query: &QueryArgs,
) -> Result<String> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph_for_tenant(ledger, &context.tenant_id, &graph)?;
    run_query_with_graph(context, ledger, &graph, query)
}

#[cfg(not(feature = "graph-kuzu"))]
fn run_query_with_kuzu(
    _context: &QueryContext,
    _ledger: &SqliteEventLedger,
    _hivemind_dir: &std::path::Path,
    _query: &QueryArgs,
) -> Result<String> {
    Err(CliError::InvalidInput(
        "graph backend 'kuzu' requires building with --features graph-kuzu".to_owned(),
    )
    .into())
}

#[cfg(feature = "graph-kuzu")]
fn run_dump_with_kuzu(
    tenant_id: &TenantId,
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    dump: &DumpArgs,
) -> Result<String> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    run_dump_with_graph(&graph, dump)
}

#[cfg(not(feature = "graph-kuzu"))]
fn run_dump_with_kuzu(
    _tenant_id: &TenantId,
    _ledger: &impl EventLedger,
    _hivemind_dir: &std::path::Path,
    _dump: &DumpArgs,
) -> Result<String> {
    Err(CliError::InvalidInput(
        "graph backend 'kuzu' requires building with --features graph-kuzu".to_owned(),
    )
    .into())
}

fn format_output(as_json: bool, envelope: &OutputEnvelope) -> Result<String> {
    if as_json {
        serde_json::to_string(envelope).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        Ok(envelope.value.clone())
    }
}

fn format_disagree_output(as_json: bool, output: &DisagreeCommandOutput) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    Ok(format!(
        "event_id={} decision_id={} status={}",
        output.event_id,
        output.decision_id,
        decision_status_label(output.decision_status)
    ))
}

fn format_supersede_output(as_json: bool, output: &SupersedeCommandOutput) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    Ok(format!(
        "proposal_event_id={} superseded_event_id={} old_decision_id={} new_decision_id={} old_status={} new_status={}",
        output.proposal_event_id,
        output.superseded_event_id,
        output.old_decision_id,
        output.new_decision_id,
        decision_status_label(output.old_decision_status),
        decision_status_label(output.new_decision_status)
    ))
}

fn format_review_output(as_json: bool, output: &ReviewCommandOutput) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    let mut rendered = String::new();
    let _ = writeln!(
        rendered,
        "reviewer={} matched={} reviewed={} skipped={} quit={} truncated={}",
        output.reviewer_actor_id,
        output.matched_count,
        output.reviewed_count,
        output.skipped_count,
        output.quit,
        output.truncated
    );
    if let Some(next_cursor) = &output.next_cursor {
        let _ = writeln!(rendered, "next_cursor={next_cursor}");
    }
    for action in &output.actions {
        let _ = write!(
            rendered,
            "{} decision_id={}",
            action.action, action.decision_id
        );
        if let Some(event_id) = action.event_id {
            let _ = write!(rendered, " event_id={event_id}");
        }
        if let Some(proposal_event_id) = action.proposal_event_id {
            let _ = write!(rendered, " proposal_event_id={proposal_event_id}");
        }
        if let Some(superseded_event_id) = action.superseded_event_id {
            let _ = write!(rendered, " superseded_event_id={superseded_event_id}");
        }
        if let Some(new_decision_id) = &action.new_decision_id {
            let _ = write!(rendered, " new_decision_id={new_decision_id}");
        }
        if let Some(old_status) = action.old_decision_status {
            let _ = write!(
                rendered,
                " old_status={}",
                decision_status_label(old_status)
            );
        }
        if let Some(new_status) = action.new_decision_status {
            let _ = write!(
                rendered,
                " new_status={}",
                decision_status_label(new_status)
            );
        }
        rendered.push('\n');
    }
    Ok(rendered.trim_end().to_owned())
}

fn format_json_value<T: Serialize>(compact: bool, value: &T) -> Result<String> {
    if compact {
        serde_json::to_string(value).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        serde_json::to_string_pretty(value).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    }
}

fn decision_status_after_write(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    decision_id: &str,
) -> Result<DecisionStatus> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    derive_decision_status(&graph, decision_id)
}

fn parse_actor_mappings(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut mappings = BTreeMap::new();
    for value in values {
        let (slack_user, actor_id) = value.split_once('=').ok_or_else(|| {
            CliError::InvalidInput(
                "--actor-map must use SlackUser=HiveMindActorId format".to_owned(),
            )
        })?;
        let slack_user = trimmed_required("--actor-map Slack user", slack_user)?;
        let actor_id = trimmed_required("--actor-map actor id", actor_id)?;
        mappings.insert(slack_user.to_owned(), actor_id.to_owned());
    }
    Ok(mappings)
}

fn format_import_output(as_json: bool, report: &DocumentImportReport) -> Result<String> {
    if as_json {
        serde_json::to_string(report).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        Ok(format!(
            "import_run_id={} files_seen={} blocks_imported={} no_op={} conflicts={} resolved={} duplicate_candidates={} validation_errors={} events_written={}",
            report.import_run_id,
            report.summary.files_seen,
            report.summary.blocks_imported,
            report.summary.blocks_noop,
            report.summary.blocks_conflicted,
            report.summary.blocks_resolved,
            report.summary.duplicate_candidates,
            report.summary.validation_errors,
            report.summary.events_written
        ))
    }
}

fn format_prepare_documents_output(
    as_json: bool,
    report: &DocumentPreparationReport,
) -> Result<String> {
    if as_json {
        serde_json::to_string(report).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        Ok(format!(
            "preparation_run_id={} files_seen={} files_prepared={} review_required={} needs_ocr={} validation_errors={} pages_seen={} bytes_written={}",
            report.preparation_run_id,
            report.summary.files_seen,
            report.summary.files_prepared,
            report.summary.files_review_required,
            report.summary.files_needing_ocr,
            report.summary.validation_errors,
            report.summary.pages_seen,
            report.summary.bytes_written
        ))
    }
}

pub fn exit_code_for_error(error: &HivemindError) -> CliExit {
    match error {
        HivemindError::Cli(_) => CliExit::Validation,
        HivemindError::Command(CommandError::Validation(_)) => CliExit::Validation,
        HivemindError::Command(CommandError::Invariant(_)) => CliExit::Invariant,
        HivemindError::Ledger(_) | HivemindError::Projector(_) => CliExit::Storage,
        HivemindError::Query(_) => CliExit::Generic,
    }
}

pub fn format_error(as_json: bool, error: &HivemindError) -> String {
    if as_json {
        serde_json::json!({
            "error": {
                "message": error.to_string(),
                "exit_code": exit_code_for_error(error).code()
            }
        })
        .to_string()
    } else {
        format!("error: {error}")
    }
}

fn validate_global_flags(cli: &Cli) -> Result<()> {
    if cli.actor.trim().is_empty() {
        return Err(CliError::InvalidInput("--actor must not be empty".to_owned()).into());
    }
    cli_tenant(cli)?;

    Ok(())
}

fn cli_tenant(cli: &Cli) -> Result<TenantId> {
    TenantId::new(cli.tenant.trim().to_owned())
        .map_err(|error| CliError::InvalidInput(format!("--tenant is invalid: {error}")).into())
}

fn cli_command_context(cli: &Cli, provenance: EventProvenance) -> Result<CommandContext> {
    Ok(CommandContext::new(cli_tenant(cli)?, provenance))
}

fn cli_query_context(cli: &Cli) -> Result<QueryContext> {
    Ok(QueryContext::new(cli_tenant(cli)?))
}

fn selected_graph_backend(cli: &Cli) -> Result<GraphBackend> {
    if let Some(backend) = cli.graph_backend {
        return Ok(backend);
    }

    match std::env::var("HIVEMIND_GRAPH_BACKEND") {
        Ok(value) => parse_graph_backend(&value),
        Err(std::env::VarError::NotPresent) => Ok(GraphBackend::Memory),
        Err(error) => Err(CliError::InvalidInput(format!(
            "HIVEMIND_GRAPH_BACKEND is not valid unicode: {error}"
        ))
        .into()),
    }
}

fn parse_graph_backend(value: &str) -> Result<GraphBackend> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "memory" | "in-memory" | "in_memory" => Ok(GraphBackend::Memory),
        "kuzu" | "graph-kuzu" | "graph_kuzu" => Ok(GraphBackend::Kuzu),
        other => Err(CliError::InvalidInput(format!(
            "unknown graph backend '{other}'; expected 'memory' or 'kuzu'"
        ))
        .into()),
    }
}

/// Public DOT renderer for callers outside the CLI module (e.g. the MCP
/// server). Delegates to the same internal implementation `hivemind dump`
/// uses so output stays identical across transports.
pub fn render_decision_dot(graph: &impl GraphView) -> Result<String> {
    render_dot(graph)
}

fn render_dot(graph: &impl GraphView) -> Result<String> {
    let mut dot = String::from("digraph hivemind {\n  rankdir=LR;\n");
    let nodes = graph_nodes(graph)?;
    let edges = graph_edges(graph)?;

    for ((kind, id), properties) in &nodes {
        let label = match kind {
            NodeKind::Decision => {
                let title =
                    graph_property_string(properties, "title").unwrap_or_else(|| id.clone());
                let status = decision_status_name(derive_decision_status(graph, id)?);
                label_with_status(&title, status)
            }
            NodeKind::DecisionRequest => graph_property_string(properties, "reason")
                .map(|reason| prefixed_dot_label("Decision request", &reason))
                .unwrap_or_else(|| id.clone()),
            NodeKind::Hypothesis => {
                let statement =
                    graph_property_string(properties, "statement").unwrap_or_else(|| id.clone());
                let status = hypothesis_status_name(derive_hypothesis_status(graph, id)?);
                label_with_status(&statement, status)
            }
            NodeKind::Blocker => graph_property_string(properties, "reason")
                .map(|reason| prefixed_dot_label("Blocker", &reason))
                .unwrap_or_else(|| id.clone()),
            NodeKind::Notification => graph_property_string(properties, "channel")
                .map(|channel| prefixed_dot_label("Notification", &channel))
                .unwrap_or_else(|| id.clone()),
            _ => graph_property_string(properties, "content")
                .or_else(|| graph_property_string(properties, "label"))
                .unwrap_or_else(|| id.clone()),
        };

        let _ = writeln!(
            dot,
            "  \"{}\" [label=\"{}\", shape=box, style=filled, fillcolor=\"{}\"];",
            node_key(*kind, id),
            escape_dot(&label),
            node_color(*kind)
        );
    }

    for edge in &edges {
        let _ = writeln!(
            dot,
            "  \"{}\" -> \"{}\" [label=\"{}\"];",
            node_key(edge.from_kind, &edge.from_id),
            node_key(edge.to_kind, &edge.to_id),
            edge.relation.table_name()
        );
    }

    dot.push_str("}\n");
    Ok(dot)
}

fn label_with_status(label: &str, status: &str) -> String {
    let mut output = String::with_capacity(label.len() + status.len() + "\\nstatus: ".len());
    output.push_str(label);
    output.push_str("\\nstatus: ");
    output.push_str(status);
    output
}

fn prefixed_dot_label(prefix: &str, value: &str) -> String {
    let mut output = String::with_capacity(prefix.len() + value.len() + 2);
    output.push_str(prefix);
    output.push_str("\\n");
    output.push_str(value);
    output
}

fn graph_nodes(graph: &impl GraphView) -> Result<BTreeMap<(NodeKind, String), GraphProperties>> {
    let mut nodes = BTreeMap::new();
    for kind in NodeKind::ALL {
        let rows = graph.query(&node_dump_query(kind), &GraphParams::new())?;
        for row in rows {
            let id = required_row_string(&row, "id")?;
            nodes.insert((kind, id), node_properties_from_row(kind, &row));
        }
    }
    Ok(nodes)
}

fn graph_edges(graph: &impl GraphView) -> Result<BTreeSet<DotEdge>> {
    let mut edges = BTreeSet::new();
    for relation in GraphRelationKind::ALL {
        let (from_kind, to_kind) = relation.endpoints();
        let mut query = String::new();
        let _ = write!(
            query,
            "MATCH (from:`{}`)-[rel:`{}`]->(to:`{}`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
            from_kind.table_name(),
            relation.table_name(),
            to_kind.table_name()
        );
        let rows = graph.query(&query, &GraphParams::new())?;
        for row in rows {
            edges.insert(DotEdge {
                relation,
                from_kind,
                from_id: required_row_string(&row, "from_id")?,
                to_kind,
                to_id: required_row_string(&row, "to_id")?,
            });
        }
    }
    Ok(edges)
}

fn node_dump_query(kind: NodeKind) -> String {
    let projection = match kind {
        NodeKind::Decision => {
            "node.id AS id, node.title AS title, node.rationale AS rationale, node.topic_keys AS topic_keys"
        }
        NodeKind::DecisionRequest => {
            "node.id AS id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.reason AS reason, node.priority AS priority, node.required_owner_id AS required_owner_id, node.authority_class AS authority_class, node.requested_by AS requested_by, node.client_request_id AS client_request_id"
        }
        NodeKind::Actor => "node.id AS id",
        NodeKind::Blocker => {
            "node.id AS id, node.blocked_actor_id AS blocked_actor_id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.blocked_ref AS blocked_ref, node.blocked_ref_type AS blocked_ref_type, node.reason AS reason, node.priority AS priority, node.last_progress_at AS last_progress_at, node.required_owner_id AS required_owner_id"
        }
        NodeKind::Evidence => "node.id AS id, node.content AS content",
        NodeKind::Notification => {
            "node.id AS id, node.blocker_id AS blocker_id, node.recipient_actor_id AS recipient_actor_id, node.channel AS channel, node.threshold_rule AS threshold_rule, node.source_event_ids AS source_event_ids, node.dedupe_key AS dedupe_key, node.sent_at AS sent_at"
        }
        NodeKind::Option => {
            "node.id AS id, node.label AS label, node.description AS description"
        }
        NodeKind::Hypothesis => "node.id AS id, node.statement AS statement",
    };
    format!(
        "MATCH (node:`{}`) RETURN {projection} ORDER BY node.id;",
        kind.table_name()
    )
}

fn node_properties_from_row(kind: NodeKind, row: &GraphRow) -> GraphProperties {
    let mut properties = GraphProperties::new();
    match kind {
        NodeKind::Decision => {
            insert_if_present(&mut properties, row, "title");
            insert_if_present(&mut properties, row, "rationale");
            insert_if_present(&mut properties, row, "topic_keys");
        }
        NodeKind::DecisionRequest => {
            insert_if_present(&mut properties, row, "decision_id");
            insert_if_present(&mut properties, row, "topic_keys");
            insert_if_present(&mut properties, row, "reason");
            insert_if_present(&mut properties, row, "priority");
            insert_if_present(&mut properties, row, "required_owner_id");
            insert_if_present(&mut properties, row, "authority_class");
            insert_if_present(&mut properties, row, "requested_by");
            insert_if_present(&mut properties, row, "client_request_id");
        }
        NodeKind::Actor => {}
        NodeKind::Blocker => {
            insert_if_present(&mut properties, row, "blocked_actor_id");
            insert_if_present(&mut properties, row, "decision_id");
            insert_if_present(&mut properties, row, "topic_keys");
            insert_if_present(&mut properties, row, "blocked_ref");
            insert_if_present(&mut properties, row, "blocked_ref_type");
            insert_if_present(&mut properties, row, "reason");
            insert_if_present(&mut properties, row, "priority");
            insert_if_present(&mut properties, row, "last_progress_at");
            insert_if_present(&mut properties, row, "required_owner_id");
        }
        NodeKind::Evidence => insert_if_present(&mut properties, row, "content"),
        NodeKind::Notification => {
            insert_if_present(&mut properties, row, "blocker_id");
            insert_if_present(&mut properties, row, "recipient_actor_id");
            insert_if_present(&mut properties, row, "channel");
            insert_if_present(&mut properties, row, "threshold_rule");
            insert_if_present(&mut properties, row, "source_event_ids");
            insert_if_present(&mut properties, row, "dedupe_key");
            insert_if_present(&mut properties, row, "sent_at");
        }
        NodeKind::Option => {
            insert_if_present(&mut properties, row, "label");
            insert_if_present(&mut properties, row, "description");
        }
        NodeKind::Hypothesis => insert_if_present(&mut properties, row, "statement"),
    }
    properties
}

fn insert_if_present(properties: &mut GraphProperties, row: &GraphRow, key: &str) {
    if let Some(value) = row.get(key) {
        properties.insert(key.to_owned(), value.clone());
    }
}

fn graph_property_string(properties: &GraphProperties, key: &str) -> Option<String> {
    match properties.get(key) {
        Some(GraphValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn node_key(kind: NodeKind, id: &str) -> String {
    format!("{}:{}", kind.table_name(), id)
}

fn node_color(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Decision => "#d6eaf8",
        NodeKind::DecisionRequest => "#d7bde2",
        NodeKind::Actor => "#d5f5e3",
        NodeKind::Blocker => "#f5b7b1",
        NodeKind::Evidence => "#fcf3cf",
        NodeKind::Notification => "#d2b4de",
        NodeKind::Option => "#f9e79f",
        NodeKind::Hypothesis => "#f5cba7",
    }
}

fn decision_status_name(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn hypothesis_status_name(status: HypothesisStatus) -> &'static str {
    match status {
        HypothesisStatus::Open => "open",
        HypothesisStatus::Supported => "supported",
        HypothesisStatus::Refuted => "refuted",
    }
}

fn escape_dot(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn required_row_string(row: &GraphRow, key: &str) -> Result<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Ok(value.clone()),
        _ => Err(CliError::InvalidInput(format!("row missing string field: {key}")).into()),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DotEdge {
    relation: GraphRelationKind,
    from_kind: NodeKind,
    from_id: String,
    to_kind: NodeKind,
    to_id: String,
}

#[derive(Debug, Serialize)]
struct QuickstartReport {
    ledger_dir: String,
    actor_id: String,
    decision_id: String,
    query: QuickstartQueryReport,
}

#[derive(Debug, Serialize)]
struct QuickstartQueryReport {
    result_count: usize,
    total_matches: usize,
    truncated: bool,
    first_result_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutputEnvelope {
    subcommand: &'static str,
    kind: &'static str,
    value: String,
}

impl OutputEnvelope {
    fn new(subcommand: &'static str, kind: &'static str, value: String) -> Self {
        Self {
            subcommand,
            kind,
            value,
        }
    }
}

#[derive(Debug, Serialize)]
struct DisagreeCommandOutput {
    decision_id: String,
    event_id: EventId,
    decision_status: DecisionStatus,
}

#[derive(Debug, Serialize)]
struct SupersedeCommandOutput {
    old_decision_id: String,
    new_decision_id: String,
    proposal_event_id: EventId,
    relation_event_ids: Vec<EventId>,
    superseded_event_id: EventId,
    old_decision_status: DecisionStatus,
    new_decision_status: DecisionStatus,
}

#[derive(Debug, Serialize)]
struct ReviewCommandOutput {
    reviewer_actor_id: String,
    matched_count: usize,
    reviewed_count: usize,
    skipped_count: usize,
    quit: bool,
    truncated: bool,
    next_cursor: Option<String>,
    unreviewed_only: bool,
    reviewed_semantics: &'static str,
    actions: Vec<ReviewActionOutput>,
}

#[derive(Debug, Serialize)]
struct ReviewActionOutput {
    decision_id: String,
    action: &'static str,
    event_id: Option<EventId>,
    proposal_event_id: Option<EventId>,
    superseded_event_id: Option<EventId>,
    new_decision_id: Option<String>,
    old_decision_status: Option<DecisionStatus>,
    new_decision_status: Option<DecisionStatus>,
}

// ---------------------------------------------------------------------------
// migrate subcommand
// ---------------------------------------------------------------------------

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Serialize)]
struct MigrateReport {
    dry_run: bool,
    source_dir: String,
    source_tenant: String,
    destination_tenant: String,
    events_migrated: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    parity_check: Option<ParityCheckResult>,
}

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Serialize)]
struct ParityCheckResult {
    source_event_count: u64,
    destination_event_count: u64,
    ok: bool,
}

#[cfg(feature = "shared-backend-postgres")]
fn run_migrate(cli: &Cli, args: &MigrateArgs) -> Result<String> {
    let source_dir = match &args.from {
        Some(s) => std::path::PathBuf::from(s.strip_prefix("sqlite://").unwrap_or(s.as_str())),
        None => cli.hivemind_dir.clone(),
    };
    let source_tenant = cli_tenant(cli)?;
    let sqlite = SqliteEventLedger::open(&source_dir)?;

    if args.dry_run {
        let mut count = 0u64;
        sqlite.replay_from_for_tenant(&source_tenant, 0, &mut |_event| {
            count += 1;
            Ok(())
        })?;
        let report = MigrateReport {
            dry_run: true,
            source_dir: source_dir.display().to_string(),
            source_tenant: source_tenant.to_string(),
            destination_tenant: args.to_tenant.clone(),
            events_migrated: count,
            parity_check: None,
        };
        return if cli.json {
            format_json_value(true, &report)
        } else {
            Ok(format!(
                "Dry run: {count} events would be migrated\n\
                 Source: {} (tenant: {})\n\
                 Destination tenant: {}",
                report.source_dir, report.source_tenant, report.destination_tenant
            ))
        };
    }

    let pg = PostgresEventLedger::connect(&args.to, &args.to_tenant)?;

    let mut migrated = 0u64;
    sqlite.replay_from_for_tenant(&source_tenant, 0, &mut |event| {
        pg.append(event.clone())?;
        migrated += 1;
        Ok(())
    })?;

    let mut pg_count = 0u64;
    pg.replay_from(0, &mut |_event| {
        pg_count += 1;
        Ok(())
    })?;

    let parity_ok = pg_count >= migrated;
    let report = MigrateReport {
        dry_run: false,
        source_dir: source_dir.display().to_string(),
        source_tenant: source_tenant.to_string(),
        destination_tenant: args.to_tenant.clone(),
        events_migrated: migrated,
        parity_check: Some(ParityCheckResult {
            source_event_count: migrated,
            destination_event_count: pg_count,
            ok: parity_ok,
        }),
    };

    if !parity_ok {
        return Err(CliError::InvalidInput(format!(
            "parity check failed: migrated {migrated} events but found {pg_count} in Postgres tenant '{}'",
            args.to_tenant
        ))
        .into());
    }

    if cli.json {
        format_json_value(true, &report)
    } else {
        Ok(format!(
            "Migration complete: {migrated} events migrated\n\
             Source: {} (tenant: {})\n\
             Destination tenant: {}\n\
             Parity check: OK ({pg_count} events in destination)",
            report.source_dir, report.source_tenant, report.destination_tenant
        ))
    }
}

#[cfg(test)]
mod tests;
