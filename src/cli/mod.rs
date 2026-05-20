use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::commands::Commands;
use crate::error::{CliError, CommandError};
use crate::events::{BlockerPriority, EventProvenance, RelationKind as EventRelationKind};
use crate::ingest::{
    extract_slack_decision_draft, import_documents, import_slack_thread,
    parse_slack_thread_fixture, DocumentImportFormat, DocumentImportReport, DocumentImportRequest,
    SlackIngestOutcome, DEFAULT_SLACK_MENTION,
};
use crate::ledger::{EventLedger, SqliteEventLedger};
use crate::projector::{
    memory::MemoryGraph, rebuild_graph, GraphParams, GraphProperties, GraphRow, GraphValue,
    GraphView, NodeKind, RelationKind as GraphRelationKind,
};
use crate::queries::{
    derive_decision_status, derive_hypothesis_status, export_read_only_summary,
    get_active_decision_blockers, get_blocker_notification_candidates, get_decision,
    get_decision_neighborhood, get_decisions_added_since, get_decisions_changed_since,
    get_recent_activity, get_relevant_decisions, get_supersession_chain, search_decisions,
    ActiveDecisionBlockersRequest, BlockerNotificationCandidatesRequest, ChangedSinceRequest,
    DecisionBlockerFilters, DecisionStatus, DecisionsAddedSinceFilterRequest,
    DecisionsAddedSinceRequest, HistoryFilterRequest, HypothesisStatus, NeighborhoodRequest,
    ReadOnlyExportFormat as QueryReadOnlyExportFormat, ReadOnlyExportQuery, ReadOnlyExportRequest,
    RecentActivityRequest, SearchDecisionRequest,
};
use crate::slack_app::{
    handle_slack_command, slack_app_manifest, slack_oauth_install_url, SlackAppStore,
    SlackCaptureRequest, SlackCaptureSurface, SlackCommandRequest, SlackWorkspaceInstall,
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
    #[arg(long, global = true, default_value_t = default_actor())]
    pub actor: String,

    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true, default_value = "./hivemind/")]
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
    Emit(Box<EmitArgs>),
    Import(ImportArgs),
    Query(Box<QueryArgs>),
    Dump(DumpArgs),
    Tui(TuiArgs),
    Ingest(IngestArgs),
    #[command(name = "slack-app")]
    SlackApp(SlackAppArgs),
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

#[derive(Debug, Clone, Args)]
pub struct EmitDecisionCaptureArgs {
    #[arg(long = "agent-tool")]
    pub agent_tool: String,

    #[arg(long = "agent-session")]
    pub agent_session: String,

    #[arg(long = "actor-id")]
    pub actor_id: Option<String>,

    #[arg(long = "source-ref")]
    pub source_ref: Option<String>,

    #[command(flatten)]
    pub decision: EmitDecisionProposedArgs,
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
    #[arg(long)]
    pub content: String,
}

#[derive(Debug, Clone, Args)]
pub struct EmitHypothesisRecordedArgs {
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
}

#[derive(Debug, Clone, Args)]
pub struct ImportDocumentsArgs {
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<PathBuf>,

    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,

    #[arg(long = "format", value_enum, default_value_t = ImportDocumentFormat::Auto)]
    pub format: ImportDocumentFormat,
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

#[derive(Debug, Clone, Args)]
pub struct QueryArgs {
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
    #[command(name = "search_decisions")]
    SearchDecisions(QuerySearchDecisionsArgs),
    #[command(name = "get_active_decision_blockers")]
    GetActiveDecisionBlockers(QueryActiveDecisionBlockersArgs),
    #[command(name = "get_blocker_notification_candidates")]
    GetBlockerNotificationCandidates(QueryBlockerNotificationCandidatesArgs),
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

    #[arg(long = "limit", default_value_t = 25)]
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
        Command::Emit(command) => run_emit(cli, command),
        Command::Import(import) => run_import(cli, import),
        Command::Query(query) => run_query(cli, query),
        Command::Dump(dump) => run_dump(cli, dump),
        Command::Tui(args) => run_tui(cli, args),
        Command::Ingest(args) => run_ingest(cli, args),
        Command::SlackApp(args) => run_slack_app(cli, args),
    }
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
    let outcome = import_slack_thread(&ledger, &draft)?;

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
            let report = store.drain_queue(&ledger)?;
            format_json_value(cli.json, &report)
        }
        SlackAppCommand::Command(args) => {
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let graph = MemoryGraph::default();
            rebuild_graph(&ledger, &graph)?;
            let response = handle_slack_command(
                &ledger,
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
    let commands = Commands::new(&ledger);

    let output = match &emit.command {
        EmitCommand::DecisionCapture(args) => {
            let actor_id = agent_actor_id(args)?;
            let source_ref = agent_source_ref(args, &actor_id)?;
            let commands =
                Commands::new_with_provenance(&ledger, EventProvenance::agent(source_ref));
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
            let evidence_id = commands.record_evidence(&cli.actor, &args.content)?;
            OutputEnvelope::new("emit", "evidence_id", evidence_id)
        }
        EmitCommand::HypothesisRecorded(args) => {
            let hypothesis_id = commands.record_hypothesis(&cli.actor, &args.statement)?;
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

fn run_import(cli: &Cli, import: &ImportArgs) -> Result<String> {
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    match &import.command {
        ImportCommand::Documents(args) => {
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = import_documents(
                &ledger,
                &DocumentImportRequest {
                    paths,
                    importer_actor_id: cli.actor.clone(),
                    format: args.format.as_ingest_format(),
                },
            )?;
            format_import_output(cli.json, &report)
        }
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
        let option_id = commands.record_option(
            actor_id,
            option_label,
            &format!("Option generated from CLI value '{option_label}'"),
        )?;
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

fn agent_actor_id(args: &EmitDecisionCaptureArgs) -> Result<String> {
    if let Some(actor_id) = trimmed_optional("--actor-id", &args.actor_id)? {
        return Ok(actor_id.to_owned());
    }

    let tool = trimmed_required("--agent-tool", &args.agent_tool)?;
    let session = trimmed_required("--agent-session", &args.agent_session)?;
    Ok(format!("agent:{tool}:{session}"))
}

fn agent_source_ref(args: &EmitDecisionCaptureArgs, actor_id: &str) -> Result<String> {
    if let Some(source_ref) = trimmed_optional("--source-ref", &args.source_ref)? {
        return Ok(source_ref.to_owned());
    }

    Ok(actor_id.to_owned())
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
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    if query.command.is_ledger_history_query() {
        return run_query_with_ledger(&ledger, query);
    }

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph(&ledger, &graph)?;
            run_query_with_graph(&graph, query)
        }
        GraphBackend::Kuzu => run_query_with_kuzu(&ledger, &cli.hivemind_dir, query),
    }
}

impl QueryCommand {
    fn is_ledger_history_query(&self) -> bool {
        matches!(
            self,
            QueryCommand::GetRecentActivity(_)
                | QueryCommand::GetDecisionsChangedSince(_)
                | QueryCommand::GetDecisionsAddedSince(_)
                | QueryCommand::ExportReadOnlySummary(_)
        )
    }
}

fn run_query_with_ledger(ledger: &impl EventLedger, query: &QueryArgs) -> Result<String> {
    let json = match &query.command {
        QueryCommand::GetRecentActivity(args) => serde_json::to_string(&get_recent_activity(
            ledger,
            &recent_activity_request(args)?,
        )?)
        .map_err(|error| CliError::InvalidInput(format!("json serialization failed: {error}")))?,
        QueryCommand::GetDecisionsChangedSince(args) => serde_json::to_string(
            &get_decisions_changed_since(ledger, &changed_since_request(args)?)?,
        )
        .map_err(|error| CliError::InvalidInput(format!("json serialization failed: {error}")))?,
        QueryCommand::GetDecisionsAddedSince(args) => serde_json::to_string(
            &get_decisions_added_since(ledger, &added_since_request(args)?)?,
        )
        .map_err(|error| CliError::InvalidInput(format!("json serialization failed: {error}")))?,
        QueryCommand::ExportReadOnlySummary(args) => {
            let request = export_read_only_summary_request(args)?;
            serde_json::to_string(&export_read_only_summary(ledger, &request)?).map_err(
                |error| CliError::InvalidInput(format!("json serialization failed: {error}")),
            )?
        }
        QueryCommand::GetDecision(_)
        | QueryCommand::GetRelevantDecisions(_)
        | QueryCommand::GetSupersessionChain(_)
        | QueryCommand::GetDecisionNeighborhood(_)
        | QueryCommand::SearchDecisions(_)
        | QueryCommand::GetActiveDecisionBlockers(_)
        | QueryCommand::GetBlockerNotificationCandidates(_) => {
            return Err(
                CliError::InvalidInput("query requires graph-backed execution".to_owned()).into(),
            )
        }
    };

    Ok(json)
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
    let now = now.unwrap_or_else(Utc::now);
    let resolved = resolve_relative_phrase(value, now, timezone).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "{flag} must be an RFC3339 timestamp or supported phrase (got: {value})"
        ))
    })?;
    Ok(Some(resolved))
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
    use chrono::TimeZone;
    Utc.from_utc_datetime(&now.date_naive().and_hms_opt(0, 0, 0).expect("midnight"))
}

fn start_of_current_iso_week_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::{Datelike, TimeZone};
    let date = now.date_naive();
    let days_from_monday = i64::from(date.weekday().num_days_from_monday());
    let monday = date - chrono::Duration::days(days_from_monday);
    Utc.from_utc_datetime(&monday.and_hms_opt(0, 0, 0).expect("midnight"))
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

fn run_query_with_graph(graph: &impl GraphView, query: &QueryArgs) -> Result<String> {
    let json = match &query.command {
        QueryCommand::GetDecision(args) => {
            serde_json::to_string(&get_decision(graph, &args.decision_id)?).map_err(|error| {
                CliError::InvalidInput(format!("json serialization failed: {error}"))
            })?
        }
        QueryCommand::GetRelevantDecisions(args) => serde_json::to_string(&get_relevant_decisions(
            graph,
            &args.topic,
            args.status.map(QueryDecisionStatus::as_decision_status),
        )?)
        .map_err(|error| CliError::InvalidInput(format!("json serialization failed: {error}")))?,
        QueryCommand::GetSupersessionChain(args) => {
            serde_json::to_string(&get_supersession_chain(graph, &args.decision_id)?).map_err(
                |error| CliError::InvalidInput(format!("json serialization failed: {error}")),
            )?
        }
        QueryCommand::GetDecisionNeighborhood(args) => {
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
            serde_json::to_string(&get_decision_neighborhood(
                graph,
                &args.decision_id,
                &request,
            )?)
            .map_err(|error| {
                CliError::InvalidInput(format!("json serialization failed: {error}"))
            })?
        }
        QueryCommand::SearchDecisions(args) => {
            let request = SearchDecisionRequest {
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
                cursor: args.cursor.clone(),
            };
            serde_json::to_string(&search_decisions(graph, &request)?).map_err(|error| {
                CliError::InvalidInput(format!("json serialization failed: {error}"))
            })?
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
            serde_json::to_string(&get_active_decision_blockers(graph, &request)?).map_err(
                |error| CliError::InvalidInput(format!("json serialization failed: {error}")),
            )?
        }
        QueryCommand::GetBlockerNotificationCandidates(args) => {
            let request = BlockerNotificationCandidatesRequest {
                now: parse_required_query_datetime(&args.now, "--now")?,
                policy_version: args.policy_version.clone(),
                limit: args.limit,
                cursor: args.cursor.clone(),
            };
            serde_json::to_string(&get_blocker_notification_candidates(graph, &request)?).map_err(
                |error| CliError::InvalidInput(format!("json serialization failed: {error}")),
            )?
        }
        QueryCommand::GetRecentActivity(_)
        | QueryCommand::GetDecisionsChangedSince(_)
        | QueryCommand::GetDecisionsAddedSince(_)
        | QueryCommand::ExportReadOnlySummary(_) => {
            return Err(
                CliError::InvalidInput("query requires ledger-backed execution".to_owned()).into(),
            )
        }
    };

    Ok(json)
}

fn parse_query_datetime(value: Option<&str>, flag: &str) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| parse_required_query_datetime(value, flag))
        .transpose()
}

fn parse_required_query_datetime(value: &str, flag: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            CliError::InvalidInput(format!("{flag} must be an RFC3339 timestamp: {error}")).into()
        })
}

fn run_dump(cli: &Cli, dump: &DumpArgs) -> Result<String> {
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph(&ledger, &graph)?;
            run_dump_with_graph(&graph, dump)
        }
        GraphBackend::Kuzu => run_dump_with_kuzu(&ledger, &cli.hivemind_dir, dump),
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
            rebuild_graph(&ledger, &graph)?;
            crate::tui::run(&graph, config)?;
        }
        GraphBackend::Kuzu => run_tui_with_kuzu(&ledger, &cli.hivemind_dir, config)?,
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
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    config: crate::tui::TuiConfig,
) -> Result<()> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph(ledger, &graph)?;
    crate::tui::run(&graph, config)
}

#[cfg(all(feature = "tui", not(feature = "graph-kuzu")))]
fn run_tui_with_kuzu(
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
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    query: &QueryArgs,
) -> Result<String> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph(ledger, &graph)?;
    run_query_with_graph(&graph, query)
}

#[cfg(not(feature = "graph-kuzu"))]
fn run_query_with_kuzu(
    _ledger: &impl EventLedger,
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
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    dump: &DumpArgs,
) -> Result<String> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph(ledger, &graph)?;
    run_dump_with_graph(&graph, dump)
}

#[cfg(not(feature = "graph-kuzu"))]
fn run_dump_with_kuzu(
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
            "import_run_id={} files_seen={} blocks_imported={} no_op={} conflicts={} duplicate_candidates={} validation_errors={} events_written={}",
            report.import_run_id,
            report.summary.files_seen,
            report.summary.blocks_imported,
            report.summary.blocks_noop,
            report.summary.blocks_conflicted,
            report.summary.duplicate_candidates,
            report.summary.validation_errors,
            report.summary.events_written
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

    Ok(())
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

fn default_actor() -> String {
    std::env::var("HIVEMIND_ACTOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "unknown-actor".to_owned())
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
                format!("{title}\\nstatus: {status}")
            }
            NodeKind::DecisionRequest => graph_property_string(properties, "reason")
                .map(|reason| format!("Decision request\\n{reason}"))
                .unwrap_or_else(|| id.clone()),
            NodeKind::Hypothesis => {
                let statement =
                    graph_property_string(properties, "statement").unwrap_or_else(|| id.clone());
                let status = hypothesis_status_name(derive_hypothesis_status(graph, id)?);
                format!("{statement}\\nstatus: {status}")
            }
            NodeKind::Blocker => graph_property_string(properties, "reason")
                .map(|reason| format!("Blocker\\n{reason}"))
                .unwrap_or_else(|| id.clone()),
            NodeKind::Notification => graph_property_string(properties, "channel")
                .map(|channel| format!("Notification\\n{channel}"))
                .unwrap_or_else(|| id.clone()),
            _ => graph_property_string(properties, "content")
                .or_else(|| graph_property_string(properties, "label"))
                .unwrap_or_else(|| id.clone()),
        };

        dot.push_str(&format!(
            "  \"{}\" [label=\"{}\", shape=box, style=filled, fillcolor=\"{}\"];\n",
            node_key(*kind, id),
            escape_dot(&label),
            node_color(*kind)
        ));
    }

    for edge in &edges {
        dot.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            node_key(edge.from_kind, &edge.from_id),
            node_key(edge.to_kind, &edge.to_id),
            edge.relation.table_name()
        ));
    }

    dot.push_str("}\n");
    Ok(dot)
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
        let rows = graph.query(
            &format!(
                "MATCH (from:`{}`)-[rel:`{}`]->(to:`{}`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
                from_kind.table_name(),
                relation.table_name(),
                to_kind.table_name()
            ),
            &GraphParams::new(),
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn resolves_since_last_week_against_frozen_now_in_utc() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
        let resolved = resolve_diff_bound(
            "--since",
            Some("last week"),
            None,
            Some(now),
            TimeZoneSpec::Utc,
        )
        .expect("resolves last week");
        assert_eq!(
            resolved,
            Some(Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap()),
            "last week must resolve to the start of the previous ISO week (Mon 00:00 UTC)"
        );
    }

    #[test]
    fn resolves_today_yesterday_this_week_against_frozen_now() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
        assert_eq!(
            resolve_diff_bound("--since", Some("today"), None, Some(now), TimeZoneSpec::Utc)
                .unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap())
        );
        assert_eq!(
            resolve_diff_bound(
                "--since",
                Some("yesterday"),
                None,
                Some(now),
                TimeZoneSpec::Utc,
            )
            .unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap())
        );
        assert_eq!(
            resolve_diff_bound(
                "--since",
                Some("this week"),
                None,
                Some(now),
                TimeZoneSpec::Utc,
            )
            .unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap())
        );
        assert_eq!(
            resolve_diff_bound("--since", Some("now"), None, Some(now), TimeZoneSpec::Utc).unwrap(),
            Some(now)
        );
    }

    #[test]
    fn non_utc_timezone_is_rejected_in_slice_1() {
        let error = TimeZoneSpec::parse("America/New_York").expect_err("non-utc rejected");
        assert!(error.to_string().contains("only UTC is accepted"));
    }

    #[test]
    fn explicit_rfc3339_in_since_takes_precedence_over_phrase_parser() {
        let resolved = resolve_diff_bound(
            "--since",
            Some("2026-05-01T08:30:00Z"),
            None,
            None,
            TimeZoneSpec::Utc,
        )
        .expect("rfc3339 parses");
        use chrono::TimeZone;
        assert_eq!(
            resolved,
            Some(Utc.with_ymd_and_hms(2026, 5, 1, 8, 30, 0).unwrap())
        );
    }

    #[test]
    fn unknown_phrase_returns_friendly_error() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
        let error = resolve_diff_bound(
            "--since",
            Some("two fortnights ago"),
            None,
            Some(now),
            TimeZoneSpec::Utc,
        )
        .expect_err("unknown phrase rejected");
        assert!(error.to_string().contains("supported phrase"));
    }

    #[test]
    fn parses_get_decisions_added_since_command() {
        let cli = Cli::parse_from([
            "hivemind",
            "query",
            "get_decisions_added_since",
            "--since",
            "last week",
            "--timezone",
            "UTC",
            "--now",
            "2026-05-19T12:00:00Z",
            "--source",
            "document",
            "--limit",
            "10",
        ]);
        let args = match cli.command {
            Command::Query(args) => args,
            command => {
                assert!(
                    matches!(command, Command::Query(_)),
                    "expected query command"
                );
                return;
            }
        };
        let args = match args.command {
            QueryCommand::GetDecisionsAddedSince(args) => args,
            command => {
                assert!(
                    matches!(command, QueryCommand::GetDecisionsAddedSince(_)),
                    "expected GetDecisionsAddedSince"
                );
                return;
            }
        };
        assert_eq!(args.since.as_deref(), Some("last week"));
        assert_eq!(args.now.as_deref(), Some("2026-05-19T12:00:00Z"));
        assert_eq!(args.filters.sources, vec!["document"]);
        assert_eq!(args.limit, 10);

        let request = added_since_request(&args).expect("request built");
        use chrono::TimeZone;
        assert_eq!(
            request.since_timestamp,
            Some(Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap())
        );
        assert_eq!(request.filters.sources, vec!["document"]);
    }

    #[test]
    fn parses_global_flags_and_emit_subcommand() {
        let cli = Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--json",
            "--hivemind-dir",
            "./state",
            "--graph-backend",
            "memory",
            "-vv",
            "emit",
            "evidence.recorded",
            "--content",
            "sample",
        ]);

        assert_eq!(cli.actor, "agent-1");
        assert!(cli.json);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.hivemind_dir, PathBuf::from("./state"));
        assert_eq!(cli.graph_backend, Some(GraphBackend::Memory));
        assert!(matches!(
            &cli.command,
            Command::Emit(command)
                if matches!(command.command, EmitCommand::EvidenceRecorded(_))
        ));
    }

    #[test]
    fn cli_version_comes_from_cargo_package_version() {
        assert_eq!(
            Cli::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn parses_tui_filters_and_export_path() {
        let cli = Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            "./state",
            "tui",
            "--q",
            "queue",
            "--topic",
            "infra,storage",
            "--status",
            "accepted",
            "--actor-id",
            "agent:codex:1",
            "--source",
            "agent",
            "--limit",
            "5",
            "--dot-output",
            "focused.dot",
        ]);

        let args = match cli.command {
            Command::Tui(args) => args,
            command => {
                assert!(matches!(command, Command::Tui(_)), "expected tui command");
                return;
            }
        };
        assert_eq!(args.query.as_deref(), Some("queue"));
        assert_eq!(args.topic_keys, vec!["infra", "storage"]);
        assert_eq!(args.statuses, vec![QueryDecisionStatus::Accepted]);
        assert_eq!(args.actor_ids, vec!["agent:codex:1"]);
        assert_eq!(args.sources, vec!["agent"]);
        assert_eq!(args.limit, 5);
        assert_eq!(args.dot_output, PathBuf::from("focused.dot"));
    }

    #[cfg(not(feature = "tui"))]
    #[test]
    fn tui_command_requires_feature() {
        let cli = Cli::parse_from(["hivemind", "tui"]);

        let error = run(&cli).expect_err("tui needs feature");

        assert!(error
            .to_string()
            .contains("requires building with --features tui"));
    }

    #[test]
    fn parses_graph_backend_from_env_aliases() {
        assert_eq!(parse_graph_backend("memory").unwrap(), GraphBackend::Memory);
        assert_eq!(
            parse_graph_backend("in-memory").unwrap(),
            GraphBackend::Memory
        );
        assert_eq!(parse_graph_backend("kuzu").unwrap(), GraphBackend::Kuzu);
        assert!(parse_graph_backend("postgres").is_err());
    }

    #[test]
    fn maps_exit_codes_by_error_kind() {
        assert_eq!(
            exit_code_for_error(&HivemindError::Cli(CliError::InvalidInput("x".into()))).code(),
            2
        );
        assert_eq!(
            exit_code_for_error(&HivemindError::Command(CommandError::Validation(
                "x".into()
            )))
            .code(),
            2
        );
        assert_eq!(
            exit_code_for_error(&HivemindError::Command(CommandError::Invariant("x".into())))
                .code(),
            3
        );
        assert_eq!(
            exit_code_for_error(&HivemindError::Ledger(crate::LedgerError::Storage(
                "x".into()
            )))
            .code(),
            4
        );
        assert_eq!(
            exit_code_for_error(&HivemindError::Query(crate::QueryError::Execution(
                "x".into()
            )))
            .code(),
            1
        );
    }

    #[test]
    fn emit_records_evidence_as_json() {
        let hivemind_dir = unique_test_dir("emit-records-evidence");
        let cli = Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "evidence.recorded",
            "--content",
            "API latency evidence",
        ]);

        let output = run(&cli).expect("emit evidence succeeds");
        let output: serde_json::Value = serde_json::from_str(&output).expect("valid json output");

        assert_eq!(
            output.get("subcommand").and_then(|value| value.as_str()),
            Some("emit")
        );
        assert_eq!(
            output.get("kind").and_then(|value| value.as_str()),
            Some("evidence_id")
        );
        assert!(output
            .get("value")
            .and_then(|value| value.as_str())
            .expect("evidence id")
            .starts_with("evidence-"));
    }

    #[test]
    fn emit_proposes_decision_with_cli_option_labels() {
        let hivemind_dir = unique_test_dir("emit-proposes-decision");
        let cli = Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "decision.proposed",
            "--title",
            "Pick queue",
            "--rationale",
            "Need durable ingestion",
            "--topic-keys",
            "infra,queue",
            "--options",
            "sync,async",
            "--chose",
            "async",
        ]);

        let output = run(&cli).expect("emit decision succeeds");

        assert!(output.starts_with("decision-"));
    }

    #[test]
    fn search_decisions_cli_returns_query_response() {
        let hivemind_dir = unique_test_dir("query-search-decisions");
        let decision_id = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "decision.proposed",
            "--title",
            "Pick queue",
            "--rationale",
            "Need durable ingestion",
            "--topic-keys",
            "infra,queue",
            "--options",
            "sync,async",
            "--chose",
            "async",
        ]))
        .expect("emit decision succeeds");

        let query = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "query",
            "search_decisions",
            "--q",
            "queue",
            "--topic",
            "infra",
            "--status",
            "proposed",
            "--actor-id",
            "agent-1",
            "--source",
            "cli",
            "--limit",
            "5",
        ]))
        .expect("search query succeeds");
        let query: serde_json::Value = serde_json::from_str(&query).expect("valid query json");

        assert_eq!(query["result_count"], serde_json::json!(1));
        assert_eq!(query["data"]["items"][0]["decision"]["id"], decision_id);
        assert_eq!(query["data"]["items"][0]["rank"], serde_json::json!(1));
        assert_eq!(query["data"]["next_cursor"], serde_json::Value::Null);

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn ledger_history_cli_queries_and_exports_read_only_summary() {
        let hivemind_dir = unique_test_dir("query-ledger-history");
        let decision_id = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "decision.proposed",
            "--title",
            "Pick queue",
            "--rationale",
            "Need durable ingestion",
            "--topic-keys",
            "infra,queue",
            "--options",
            "sync,async",
            "--chose",
            "async",
        ]))
        .expect("emit decision succeeds");

        let recent = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "query",
            "get_recent_activity",
            "--limit",
            "1",
            "--source",
            "cli",
        ]))
        .expect("recent activity query succeeds");
        let recent: serde_json::Value =
            serde_json::from_str(&recent).expect("valid recent activity json");
        assert_eq!(recent["result_count"], serde_json::json!(1));
        assert_eq!(recent["data"]["items"][0]["decision_ids"][0], decision_id);
        assert!(recent["data"]["items"][0]["citation_id"]
            .as_str()
            .expect("citation id")
            .starts_with("event:"));

        let changed = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "query",
            "get_decisions_changed_since",
            "--since-offset",
            "0",
            "--limit",
            "1",
        ]))
        .expect("changed-since query succeeds");
        let changed: serde_json::Value =
            serde_json::from_str(&changed).expect("valid changed-since json");
        assert_eq!(
            changed["data"]["items"][0]["change_kind"],
            serde_json::json!("new_decision")
        );

        let export = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "query",
            "export_read_only_summary",
            "--query",
            "recent_activity",
            "--format",
            "markdown",
            "--generated-at",
            "2026-05-19T12:00:00Z",
            "--limit",
            "10",
        ]))
        .expect("export query succeeds");
        let export: serde_json::Value = serde_json::from_str(&export).expect("valid export json");
        assert_eq!(export["data"]["format"], serde_json::json!("markdown"));
        assert_eq!(
            export["data"]["citation_map"]["event:1"]["source"],
            serde_json::json!("cli")
        );
        assert!(export["data"]["markdown"]
            .as_str()
            .expect("markdown body")
            .contains("citation=event:1"));

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn import_documents_cli_imports_queryable_document_decisions_and_reimport_noops() {
        let hivemind_dir = unique_test_dir("import-documents");
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/documents");

        let output = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "importer:local",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "import",
            "documents",
            fixtures.to_str().expect("utf-8 fixture path"),
        ]))
        .expect("document import succeeds");
        let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
        assert_eq!(output["summary"]["blocks_imported"], serde_json::json!(2));
        assert_eq!(output["summary"]["events_written"].as_u64(), Some(15));

        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let latest_after_first = ledger.latest_offset().expect("latest offset");

        let search = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "query",
            "search_decisions",
            "--source",
            "document",
            "--topic",
            "storage",
        ]))
        .expect("document decision search succeeds");
        let search: serde_json::Value = serde_json::from_str(&search).expect("valid search json");
        assert_eq!(search["result_count"], serde_json::json!(1));
        assert_eq!(
            search["data"]["items"][0]["decision"]["status"],
            serde_json::json!("accepted")
        );

        let events = ledger.read(0, 100).expect("events read");
        let storage_proposal = events
            .iter()
            .find(|event| {
                event.event_type == crate::events::EventType::DecisionProposed
                    && event.payload.get("title").and_then(|value| value.as_str())
                        == Some("Use SQLite for the local prototype")
            })
            .expect("storage proposal event");
        assert_eq!(storage_proposal.actor_id, "actor:alice");
        assert_eq!(
            storage_proposal.source,
            crate::events::EventSource::Document
        );
        let storage_ref: serde_json::Value = serde_json::from_str(
            storage_proposal
                .source_ref
                .as_deref()
                .expect("document source ref"),
        )
        .expect("document source ref json");
        assert_eq!(storage_ref["source"], serde_json::json!("document"));
        assert_eq!(storage_ref["block_id"], serde_json::json!("local-storage"));
        assert_eq!(storage_ref["provisional_actor"], serde_json::json!(false));
        assert!(storage_ref["path"]
            .as_str()
            .expect("source path")
            .ends_with("storage_decision.md"));
        assert!(storage_ref["sha256"].as_str().expect("source hash").len() >= 64);
        assert!(storage_ref["source_span"]["line_start"].as_u64().unwrap() > 0);
        assert!(storage_ref["source_snippet"]
            .as_str()
            .expect("snippet")
            .contains("Decision:"));

        let report_proposal = events
            .iter()
            .find(|event| {
                event.event_type == crate::events::EventType::DecisionProposed
                    && event.payload.get("title").and_then(|value| value.as_str())
                        == Some("Import weekly decision notes locally")
            })
            .expect("report proposal event");
        assert_eq!(report_proposal.actor_id, "importer:local");
        let report_ref: serde_json::Value = serde_json::from_str(
            report_proposal
                .source_ref
                .as_deref()
                .expect("document source ref"),
        )
        .expect("document source ref json");
        assert_eq!(report_ref["provisional_actor"], serde_json::json!(true));
        assert_eq!(report_ref["original_actor_id"], serde_json::Value::Null);

        let second_output = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "importer:local",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "import",
            "documents",
            fixtures.to_str().expect("utf-8 fixture path"),
        ]))
        .expect("document re-import succeeds");
        let second_output: serde_json::Value =
            serde_json::from_str(&second_output).expect("valid import json");
        assert_eq!(
            second_output["summary"]["blocks_imported"],
            serde_json::json!(0)
        );
        assert_eq!(
            second_output["summary"]["blocks_noop"],
            serde_json::json!(2)
        );
        assert_eq!(
            second_output["summary"]["events_written"],
            serde_json::json!(0)
        );
        assert_eq!(
            ledger.latest_offset().expect("latest offset unchanged"),
            latest_after_first
        );

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn import_documents_cli_reports_changed_same_id_as_conflict_without_writes() {
        let hivemind_dir = unique_test_dir("import-document-conflict-ledger");
        let scratch_dir = unique_test_dir("import-document-conflict-doc");
        std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
        let document_path = scratch_dir.join("decision.md");
        std::fs::write(
            &document_path,
            "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: proposed\n  topic_keys: conflict\n  rationale: First rationale.\n  options:\n    - first option\n",
        )
        .expect("write initial doc");

        run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "importer:local",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "import",
            "documents",
            "--file",
            document_path.to_str().expect("utf-8 doc path"),
        ]))
        .expect("initial import succeeds");
        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let latest_after_first = ledger.latest_offset().expect("latest offset");

        std::fs::write(
            &document_path,
            "Decision:\n  id: conflict-demo\n  title: Changed title\n  status: proposed\n  topic_keys: conflict\n  rationale: Changed rationale.\n  options:\n    - first option\n",
        )
        .expect("write changed doc");

        let output = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "importer:local",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "import",
            "documents",
            "--file",
            document_path.to_str().expect("utf-8 doc path"),
        ]))
        .expect("conflict import reports successfully");
        let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
        assert_eq!(output["summary"]["blocks_conflicted"], serde_json::json!(1));
        assert_eq!(output["summary"]["events_written"], serde_json::json!(0));
        assert!(output["files"][0]["blocks"][0]["message"]
            .as_str()
            .expect("conflict message")
            .contains("stable decision id already exists"));
        assert_eq!(
            ledger.latest_offset().expect("latest offset unchanged"),
            latest_after_first
        );

        let _ = std::fs::remove_dir_all(&hivemind_dir);
        let _ = std::fs::remove_dir_all(&scratch_dir);
    }

    #[test]
    fn emit_decision_capture_records_codex_and_claude_agent_provenance() {
        let hivemind_dir = unique_test_dir("emit-agent-decision-capture");

        let codex_decision = run(&Cli::parse_from([
            "hivemind",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "decision.capture",
            "--agent-tool",
            "codex",
            "--agent-session",
            "session-1",
            "--title",
            "Use direct CLI capture for Codex",
            "--rationale",
            "Codex can invoke a deterministic local command from the workspace",
            "--topic-keys",
            "agents,capture",
            "--options",
            "direct-cli,mcp",
            "--chose",
            "direct-cli",
        ]))
        .expect("codex capture succeeds");
        let codex_decision = envelope_value(&codex_decision);

        let claude_decision = run(&Cli::parse_from([
            "hivemind",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "decision.capture",
            "--agent-tool",
            "claude",
            "--agent-session",
            "session-2",
            "--title",
            "Use direct CLI capture for Claude",
            "--rationale",
            "Claude can call the same command with only identity changed",
            "--topic-keys",
            "agents,capture",
            "--options",
            "direct-cli,hooks",
            "--chose",
            "direct-cli",
        ]))
        .expect("claude capture succeeds");
        let claude_decision = envelope_value(&claude_decision);

        assert_decision_queryable(&hivemind_dir, &codex_decision);
        assert_decision_queryable(&hivemind_dir, &claude_decision);

        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let events = ledger.read(0, 100).expect("events read");
        for (decision_id, actor_id) in [
            (&codex_decision, "agent:codex:session-1"),
            (&claude_decision, "agent:claude:session-2"),
        ] {
            let event = events
                .iter()
                .find(|event| {
                    event.event_type == crate::events::EventType::DecisionProposed
                        && event
                            .payload
                            .get("decision_id")
                            .and_then(|value| value.as_str())
                            == Some(decision_id.as_str())
                })
                .expect("decision proposal exists");

            assert_eq!(event.actor_id, actor_id);
            assert_eq!(event.source, crate::events::EventSource::Agent);
            assert_eq!(event.source_ref.as_deref(), Some(actor_id));

            let proposal_id = event.event_id.expect("proposal has ledger origin");
            let relation_events = events
                .iter()
                .filter(|event| event.causation_event_id == Some(proposal_id))
                .collect::<Vec<_>>();
            assert!(!relation_events.is_empty());
            for relation_event in relation_events {
                assert_eq!(relation_event.actor_id, actor_id);
                assert_eq!(relation_event.source, crate::events::EventSource::Agent);
                assert_eq!(relation_event.source_ref.as_deref(), Some(actor_id));
            }
        }

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn ingest_slack_thread_creates_queryable_decision_with_slack_provenance() {
        let hivemind_dir = unique_test_dir("ingest-slack-thread");
        let fixture = workspace_fixture("tests/fixtures/slack/thread_with_mention.json");

        let output = run(&Cli::parse_from([
            "hivemind",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "ingest",
            "slack-thread",
            "--file",
            fixture.to_str().expect("utf-8 fixture path"),
        ]))
        .expect("ingest succeeds");

        let output: serde_json::Value = serde_json::from_str(&output).expect("json output");
        assert_eq!(output["subcommand"], serde_json::json!("ingest"));
        assert_eq!(output["kind"], serde_json::json!("decision_id"));
        let decision_id = output["value"].as_str().expect("decision id").to_owned();
        assert!(decision_id.starts_with("decision-"));

        assert_decision_queryable(&hivemind_dir, &decision_id);

        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let events = ledger.read(0, 100).expect("events read");
        let proposal = events
            .iter()
            .find(|event| {
                event.event_type == crate::events::EventType::DecisionProposed
                    && event
                        .payload
                        .get("decision_id")
                        .and_then(|value| value.as_str())
                        == Some(decision_id.as_str())
            })
            .expect("proposal event present");
        assert_eq!(proposal.actor_id, "slack:T123:U111");
        assert_eq!(proposal.source, crate::events::EventSource::Slack);
        assert_eq!(
            proposal.source_ref.as_deref(),
            Some("slack://T123/C456/1715970800.000100")
        );

        let proposal_id = proposal.event_id.expect("proposal event id");
        let related: Vec<_> = events
            .iter()
            .filter(|event| event.causation_event_id == Some(proposal_id))
            .collect();
        assert!(!related.is_empty(), "proposal must fan out relations");
        for event in &related {
            assert_eq!(event.source, crate::events::EventSource::Slack);
            assert_eq!(
                event.source_ref.as_deref(),
                Some("slack://T123/C456/1715970800.000100")
            );
        }

        let evidence_count = events
            .iter()
            .filter(|event| {
                event.event_type == crate::events::EventType::EvidenceRecorded
                    && event.source == crate::events::EventSource::Slack
            })
            .count();
        assert_eq!(evidence_count, 1);

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn ingest_slack_thread_is_idempotent_on_reimport() {
        let hivemind_dir = unique_test_dir("ingest-slack-thread-reimport");
        let fixture = workspace_fixture("tests/fixtures/slack/thread_with_mention.json");

        let args = [
            "hivemind",
            "--json",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "ingest",
            "slack-thread",
            "--file",
            fixture.to_str().expect("utf-8 fixture path"),
        ];

        let first: serde_json::Value =
            serde_json::from_str(&run(&Cli::parse_from(args)).expect("first ingest")).unwrap();
        assert_eq!(first["kind"], serde_json::json!("decision_id"));
        let first_decision = first["value"]
            .as_str()
            .expect("first decision id")
            .to_owned();

        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let first_event_count = ledger.read(0, 1024).expect("read events").len();

        let second: serde_json::Value =
            serde_json::from_str(&run(&Cli::parse_from(args)).expect("second ingest")).unwrap();
        assert_eq!(second["kind"], serde_json::json!("decision_id_existing"));
        assert_eq!(second["value"].as_str(), Some(first_decision.as_str()));

        let second_event_count = ledger.read(0, 1024).expect("read events").len();
        assert_eq!(
            first_event_count, second_event_count,
            "re-import must not append events"
        );

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn ingest_slack_thread_rejects_thread_without_mention() {
        let hivemind_dir = unique_test_dir("ingest-slack-thread-no-mention");
        let fixture = workspace_fixture("tests/fixtures/slack/thread_without_mention.json");

        let error = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "ingest",
            "slack-thread",
            "--file",
            fixture.to_str().expect("utf-8 fixture path"),
        ]))
        .expect_err("mention gate rejects thread");

        assert!(
            error.to_string().contains("missing required mention"),
            "error should mention gate: {error}"
        );

        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        assert!(
            ledger.read(0, 10).expect("read events").is_empty(),
            "no events should have been written"
        );

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    fn workspace_fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[cfg(not(feature = "graph-kuzu"))]
    #[test]
    fn kuzu_backend_requires_feature() {
        let hivemind_dir = unique_test_dir("kuzu-feature-required");
        let cli = Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "--graph-backend",
            "kuzu",
            "query",
            "get_decision",
            "--id",
            "decision-missing",
        ]);

        let error = run(&cli).expect_err("kuzu backend needs feature");

        assert!(error
            .to_string()
            .contains("requires building with --features graph-kuzu"));
    }

    #[cfg(feature = "graph-kuzu")]
    #[test]
    fn kuzu_backend_queries_and_dumps_persistent_projection() {
        let hivemind_dir = unique_test_dir("kuzu-query");
        let decision_id = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "emit",
            "decision.proposed",
            "--title",
            "Persist query graph",
            "--rationale",
            "Kuzu mode should project SQLite events before reads",
            "--topic-keys",
            "architecture,storage",
            "--options",
            "memory,kuzu",
            "--chose",
            "kuzu",
        ]))
        .expect("emit decision succeeds");

        let query_args = [
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "--graph-backend",
            "kuzu",
            "query",
            "get_relevant_decisions",
            "--topic",
            "architecture",
        ];
        let first_query = run(&Cli::parse_from(query_args)).expect("kuzu query succeeds");
        let second_query = run(&Cli::parse_from(query_args)).expect("repeated kuzu query succeeds");
        let mut first_json: serde_json::Value =
            serde_json::from_str(&first_query).expect("first query json");
        let mut second_json: serde_json::Value =
            serde_json::from_str(&second_query).expect("second query json");
        first_json["latency_ms"] = serde_json::json!(0);
        second_json["latency_ms"] = serde_json::json!(0);

        assert_eq!(first_json, second_json);
        assert_eq!(first_json["result_count"], serde_json::json!(1));
        assert_eq!(first_json["data"][0]["id"], serde_json::json!(decision_id));
        assert!(hivemind_dir.join("graph.kuzu").exists());

        let dot = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "--graph-backend",
            "kuzu",
            "dump",
            "--format",
            "dot",
        ]))
        .expect("kuzu dump succeeds");
        assert!(dot.contains("Persist query graph"));

        let _ = std::fs::remove_dir_all(&hivemind_dir);
    }

    #[test]
    fn format_error_outputs_structured_json() {
        let error = HivemindError::Command(CommandError::Validation("bad input".to_owned()));
        let output = format_error(true, &error);
        let output: serde_json::Value = serde_json::from_str(&output).expect("valid json error");

        assert_eq!(
            output
                .pointer("/error/exit_code")
                .and_then(|value| value.as_i64()),
            Some(2)
        );
        assert!(output
            .pointer("/error/message")
            .and_then(|value| value.as_str())
            .expect("message")
            .contains("bad input"));
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hivemind-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn envelope_value(output: &str) -> String {
        let output: serde_json::Value = serde_json::from_str(output).expect("valid json output");
        assert_eq!(
            output.get("kind").and_then(|value| value.as_str()),
            Some("decision_id")
        );
        output
            .get("value")
            .and_then(|value| value.as_str())
            .expect("decision id")
            .to_owned()
    }

    fn assert_decision_queryable(hivemind_dir: &std::path::Path, decision_id: &str) {
        let query = run(&Cli::parse_from([
            "hivemind",
            "--hivemind-dir",
            hivemind_dir.to_str().expect("utf-8 temp path"),
            "query",
            "get_decision",
            "--id",
            decision_id,
        ]))
        .expect("query succeeds");
        let query: serde_json::Value = serde_json::from_str(&query).expect("valid query json");
        assert_eq!(query["result_count"], serde_json::json!(1));
        assert_eq!(query["data"]["id"], serde_json::json!(decision_id));
    }
}
