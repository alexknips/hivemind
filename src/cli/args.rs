use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::events::{
    BlockerPriority, ProjectAnchorKind as EventProjectAnchorKind,
    ProjectLinkKind as EventProjectLinkKind, ProjectSource as EventProjectSource,
};
use crate::identity::default_actor;
use crate::ingest::{
    DocumentConflictResolutionAction, DocumentImportFormat, DocumentPreparationFormat,
    DEFAULT_SLACK_MENTION,
};
use crate::ledger::LedgerConfig;
use crate::projector::RelationKind as GraphRelationKind;
use crate::quality_profile::SCAN_DEFAULT_LIMIT;
use crate::queries::{DecisionStatus, ReadOnlyExportFormat as QueryReadOnlyExportFormat};
use crate::slack_app::SlackCaptureSurface;
use crate::summarize::DIGEST_MAX_DECISIONS;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "hivemind",
    about = "Organizational decision-memory ledger and query CLI",
    version = crate::VERSION,
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

    /// Shared Postgres backend connection URL. Unset or empty selects the local
    /// SQLite ledger under --hivemind-dir. A flag value beats the environment
    /// variable; an empty flag value is treated as unset, same as
    /// HIVEMIND_DATABASE_URL. Requires the shared-backend-postgres feature.
    #[arg(long, global = true, env = "HIVEMIND_DATABASE_URL")]
    pub database_url: Option<String>,

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

impl LedgerConfig {
    /// Builds a `LedgerConfig` from global CLI flags: `--hivemind-dir` and
    /// `--database-url` (flag beats `HIVEMIND_DATABASE_URL`; an empty value
    /// means unset, same rule as `ApiConfig::new`).
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            hivemind_dir: cli.hivemind_dir.clone(),
            database_url: cli.database_url.clone().filter(|url| !url.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Capture and query a first decision on an isolated temporary ledger.
    Quickstart(QuickstartArgs),
    Emit(Box<EmitArgs>),
    Disagree(DisagreeArgs),
    Supersede(SupersedeArgs),
    /// Move a decision to another project, found by describing it. A description that
    /// matches more than one decision lists the candidates and writes nothing; pick one with
    /// --pick N. One that matches nothing is a successful `not_found` answer. Recorded with
    /// who, when, from, to and why; reversed by moving it back.
    Move(MoveArgs),
    /// Say what an existing decision rests on, after the fact: a decision it follows from,
    /// something observed, something assumed, or a declared bet. Append-only and attributed to
    /// whoever runs it (--actor), so older decisions stop reading "nothing declared" without
    /// pretending the grounding was there at capture. Resolves the decision by description with
    /// the same ambiguity gate as `supersede`; nothing is written when the description is
    /// ambiguous, a premise cannot be pinned to one decision, or a premise would close a loop.
    Ground(GroundArgs),
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
    /// Start the HTTP REST API server. Binds 127.0.0.1 by default (see
    /// --bind). Auth token is read from HIVEMIND_API_KEY; when unset the
    /// server starts in development mode with no authentication, which is
    /// refused on a non-loopback --bind unless --allow-unauthenticated-remote
    /// is passed.
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
    /// File a Linear ticket for each decision that needs a look (an attention finding: a bet
    /// past its check date, a premise that changed, evidence nobody re-checked) for human review.
    /// Set HIVEMIND_LINEAR_API_KEY and HIVEMIND_LINEAR_TEAM_ID before running.
    /// Pass --dry-run to preview what would be filed without calling Linear.
    #[command(name = "quality-scan")]
    QualityScan(QualityScanArgs),
    /// Export the decision log as a tree of Markdown files grouped per
    /// project: an INDEX.md with one section per project, and per project a
    /// projects/<handle>/INDEX.md plus one file per decision (personal
    /// projects under projects/personal/<actor>/). Writes to --out, which the
    /// export owns — stale files from a prior run are pruned. Not a `query`
    /// subcommand: query subcommands print one envelope, this one writes a
    /// directory tree.
    Export(ExportArgs),
    /// Manage the local tenant registry. Every ledger open (CLI, stdio-MCP,
    /// and the HTTP API server) errors on an unrecognized --tenant/
    /// X-HiveMind-Tenant rather than silently opening a fresh, empty scope.
    Tenant(TenantArgs),
    /// Manage the project registry: shared projects, part_of/depends_on links,
    /// and anchors. A personal project (`personal:<actor>`) is derived from the
    /// actor and never registered — `project show` resolves it directly.
    Project(ProjectArgs),
}

#[derive(Debug, Clone, Args)]
pub struct QuickstartArgs {}

#[derive(Debug, Clone, Args)]
pub struct TenantArgs {
    #[command(subcommand)]
    pub command: TenantCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TenantCommand {
    /// Register a tenant in the local SQLite tenant registry. SQLite only —
    /// on a Postgres backend, tenants are created through the server's
    /// provisioning route (POST /v1/tenants), never the CLI.
    Create(TenantCreateArgs),
}

#[derive(Debug, Clone, Args)]
pub struct TenantCreateArgs {
    /// Tenant id to register.
    pub tenant_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProjectCommand {
    /// Register a shared project. Handles are lowercase letters, digits, and
    /// dashes, 2-40 characters; the "personal:" prefix is reserved.
    Register(ProjectRegisterArgs),
    /// Link two registered projects (`part_of` or `depends_on`).
    Link(ProjectLinkArgs),
    /// Remove a currently active link. Refused when no such link is active.
    Unlink(ProjectLinkArgs),
    /// Anchor a registered project to a place in the world (folder, rig,
    /// jira, linear, github, or channel).
    Anchor(ProjectAnchorArgs),
    /// List registered (shared) projects, paged. Personal projects never
    /// appear here — resolve one directly with `project show`.
    List(ProjectListArgs),
    /// Show one project by handle, or the actor's current-project setting
    /// with `--current`. An unknown handle is a successful envelope with
    /// outcome `not_found`, never an error. A `personal:<actor>` address
    /// always resolves to a derived project.
    Show(ProjectShowArgs),
    /// List the decisions in one project, oldest first, paged. Give a shared
    /// handle, or a personal address (`personal:<actor>`) to see what is still
    /// in a personal project and not yet shared: every session of one agent
    /// tool lists together (`personal:agent:claude`), each decision showing its
    /// session. An unregistered handle is a successful envelope with outcome
    /// `not_found`, never an empty list.
    Decisions(ProjectDecisionsArgs),
    /// Set or clear the actor's current project: a one-time, per-machine
    /// setting consulted (below any folder marker or rig anchor) for
    /// captures with no repo context, such as chat. Local to this
    /// `--hivemind-dir` and actor — never a ledger fact. Setting an
    /// unregistered handle is refused with a register hint.
    Use(ProjectUseArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ProjectRegisterArgs {
    /// Handle to register.
    pub handle: String,

    #[arg(long = "display-name")]
    pub display_name: Option<String>,

    #[arg(long)]
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ProjectLinkArgs {
    #[arg(long = "from")]
    pub from: String,

    #[arg(long = "to")]
    pub to: String,

    #[arg(long, value_enum)]
    pub kind: ProjectLinkKindArg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ProjectLinkKindArg {
    PartOf,
    DependsOn,
}

impl ProjectLinkKindArg {
    pub(crate) const fn as_project_link_kind(self) -> EventProjectLinkKind {
        match self {
            Self::PartOf => EventProjectLinkKind::PartOf,
            Self::DependsOn => EventProjectLinkKind::DependsOn,
        }
    }
}

/// How a capture's `--project` was determined, as a caller may claim it. `personal_fallback`
/// (recorded by HiveMind when no project is given) and `moved` (recorded by a move) are not
/// choices here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ProjectSourceArg {
    Stated,
    FolderMarker,
    Rig,
    CurrentProject,
    Job,
}

impl ProjectSourceArg {
    pub(crate) const fn as_project_source(self) -> EventProjectSource {
        match self {
            Self::Stated => EventProjectSource::Stated,
            Self::FolderMarker => EventProjectSource::FolderMarker,
            Self::Rig => EventProjectSource::Rig,
            Self::CurrentProject => EventProjectSource::CurrentProject,
            Self::Job => EventProjectSource::Job,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct ProjectAnchorArgs {
    #[arg(long)]
    pub handle: String,

    #[arg(long = "kind", value_enum)]
    pub anchor_kind: ProjectAnchorKindArg,

    #[arg(long)]
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ProjectAnchorKindArg {
    Folder,
    Rig,
    Jira,
    Linear,
    Github,
    Channel,
}

impl ProjectAnchorKindArg {
    pub(crate) const fn as_project_anchor_kind(self) -> EventProjectAnchorKind {
        match self {
            Self::Folder => EventProjectAnchorKind::Folder,
            Self::Rig => EventProjectAnchorKind::Rig,
            Self::Jira => EventProjectAnchorKind::Jira,
            Self::Linear => EventProjectAnchorKind::Linear,
            Self::Github => EventProjectAnchorKind::Github,
            Self::Channel => EventProjectAnchorKind::Channel,
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct ProjectListArgs {
    #[arg(long, default_value_t = 25)]
    pub limit: usize,

    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ProjectShowArgs {
    /// Project handle, or a personal address (personal:<actor-id>). Omit
    /// this and pass --current instead to show the current-project setting.
    pub handle: Option<String>,

    /// Show the actor's current-project setting instead of a specific handle.
    #[arg(long)]
    pub current: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ProjectDecisionsArgs {
    /// Project handle, or a personal address (personal:<actor-id>).
    pub handle: String,

    #[arg(long, default_value_t = 25)]
    pub limit: usize,

    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ProjectUseArgs {
    /// Handle to set as the current project. Omit this and pass --clear
    /// instead to clear the setting.
    pub handle: Option<String>,

    /// Clear the current-project setting instead of setting it.
    #[arg(long)]
    pub clear: bool,
}

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
    /// Only these kinds of finding (comma-separated): bet_past_check_date, premise_superseded,
    /// premise_rejected, assumption_refuted, bet_failed, evidence_not_rechecked. Default: all.
    #[arg(long = "kind", value_delimiter = ',')]
    pub kinds: Vec<String>,

    /// Maximum tickets to file per run (1–50). Prevents flooding Linear.
    #[arg(long, default_value_t = 10)]
    pub limit: usize,

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
pub struct ExportArgs {
    /// Export format. `markdown` is the only supported value today.
    #[arg(long, value_enum)]
    pub format: ExportFormat,

    /// Directory to write the export into. Created if missing. The export
    /// owns `<out>/INDEX.md` and every `.md` file under `<out>/projects/`
    /// (plus any left in `<out>/decisions/` by the layout before decisions
    /// were grouped per project); nothing else under `<out>` is touched.
    #[arg(long)]
    pub out: PathBuf,

    /// Only export this project's decisions: a registered handle, or a
    /// `personal:<actor>` address. An unknown handle writes nothing and
    /// reports `outcome=not_found`.
    #[arg(long)]
    pub project: Option<String>,

    /// Only include decisions with a ledger timestamp at or after this
    /// RFC3339 instant.
    #[arg(long)]
    pub since: Option<String>,

    /// Restrict to decisions carrying any of these topic keys.
    #[arg(long = "topic", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    /// Restrict to decisions with one of these derived statuses.
    #[arg(long = "status", value_delimiter = ',')]
    pub statuses: Vec<QueryDecisionStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum ExportFormat {
    Markdown,
}

#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Address to bind. Defaults to loopback, so the server is reachable only
    /// from this host; pass 0.0.0.0 (or a specific interface address) to
    /// accept connections from other hosts.
    #[arg(long, env = "HIVEMIND_BIND", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    pub bind: IpAddr,

    /// Port to listen on.
    #[arg(long, short = 'p', env = "HIVEMIND_PORT", default_value_t = 8080)]
    pub port: u16,

    /// Allow serving WITHOUT authentication on a non-loopback --bind address.
    /// Without this flag, development mode (no HIVEMIND_API_KEY and no
    /// HIVEMIND_DATABASE_URL) refuses to start on anything but loopback.
    #[arg(long)]
    pub allow_unauthenticated_remote: bool,
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

    /// Only list batches from this ingest session. Session-grouped
    /// classification (hivemind-zdsh.18) uses this so one classify-queue run
    /// at session end only sees, and later classifies, its own session's
    /// pending batches.
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ClassifyQueueSubmitArgs {
    /// Batch ID(s) to classify (from `classify-queue list` output),
    /// comma-separated. Pass more than one to submit a single classification
    /// covering several batches from the same session in one model call
    /// (hivemind-zdsh.18 cadence: one call per session end, not one per
    /// batch).
    #[arg(long = "batch-id", value_delimiter = ',', required = true)]
    pub batch_id: Vec<String>,

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

    /// When a `capture_decision` or `supersede_decision` call names no `project`, work it
    /// out from where this server runs: the `.hivemind-project` files of the folders the
    /// uncommitted change touches (several projects are recorded for the nearest project
    /// they are all part of, or saved to the personal project when they share none), else
    /// the nearest one walking up from its working directory, then the project anchored to
    /// the city rig (`GC_RIG`), then this actor's `hivemind project use` setting. A call
    /// that names a `project` still wins. Off by default, so a bare server files unnamed
    /// captures under the personal project.
    #[arg(long = "project-from-context")]
    pub project_from_context: bool,
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
pub struct MoveArgs {
    /// Free-text description to resolve to the decision being moved (fluent alternative to
    /// --decision). A bare `#N` refers to candidate N from the previous ambiguous resolver output.
    pub description: Option<String>,

    #[arg(long = "decision")]
    pub decision_id: Option<String>,

    /// Select candidate N when a description resolves ambiguously.
    #[arg(long = "pick")]
    pub pick: Option<usize>,

    /// Narrow resolution to decisions carrying this topic key.
    #[arg(long = "topic")]
    pub topic: Option<String>,

    /// Where the decision goes: a registered project handle, or your own personal address.
    /// Where it is now is read from the ledger, never typed.
    #[arg(long = "to")]
    pub to: String,

    /// Why it belongs there; kept with the move.
    #[arg(long)]
    pub reason: Option<String>,
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

    /// Self-contained why, readable without the source conversation: at least 20 characters
    /// and 4 words, and not a bare reference into an external numbered list like "1a" or
    /// "2. a".
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

    /// Registered project handle to file the superseding decision under. Without it the new
    /// decision inherits the old decision's project.
    #[arg(long = "project")]
    pub project: Option<String>,

    /// How `--project` was determined (default: stated). Requires `--project`.
    #[arg(long = "project-source", value_enum, requires = "project")]
    pub project_source: Option<ProjectSourceArg>,

    /// When `--project` is absent, work the project out from where this runs: the
    /// `.hivemind-project` files of the folders the uncommitted change touches, else the
    /// nearest one walking up from the working directory, then the project anchored to the
    /// city rig (`GC_RIG`), then this actor's `hivemind project use` setting. Each records
    /// how it was determined. A change touching several projects is recorded for the nearest
    /// project they are all part of. When none applies, or they share no parent, the new
    /// decision still inherits the old decision's project. `--project` wins over all of
    /// these.
    #[arg(long = "project-from-context")]
    pub project_from_context: bool,

    #[command(flatten)]
    pub grounding: GroundingArgs,
}

/// `hivemind ground`: give an existing decision what it rests on. Takes the grounding flags of
/// `emit decision.capture` (`--rests-on-decision`, `--rests-on-evidence` with `--evidence-source`,
/// `--rests-on-assumption`, `--bet`) except `--confidence`, which is the decider's own words at
/// capture and cannot be added afterwards.
#[derive(Debug, Clone, Args)]
pub struct GroundArgs {
    /// Free-text description to resolve to the decision being grounded (fluent alternative to
    /// --id). A bare `#N` refers to candidate N from the previous ambiguous resolver output.
    pub description: Option<String>,

    #[arg(long = "id")]
    pub decision_id: Option<String>,

    /// Select candidate N when a description resolves ambiguously.
    #[arg(long = "pick")]
    pub pick: Option<usize>,

    /// Narrow resolution to decisions carrying this topic key.
    #[arg(long = "topic")]
    pub topic: Option<String>,

    /// Existing evidence ids this decision rests on (a node recorded earlier).
    #[arg(long = "evidence", value_delimiter = ',')]
    pub evidence_ids: Vec<String>,

    /// Existing hypothesis ids this decision rests on (a node recorded earlier).
    #[arg(long = "hypotheses", value_delimiter = ',')]
    pub hypothesis_ids: Vec<String>,

    #[command(flatten)]
    pub grounding: GroundingArgs,
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

    /// Self-contained why, readable without the source conversation: at least 20 characters
    /// and 4 words, and not a bare reference into an external numbered list like "1a" or
    /// "2. a".
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
    DecisionCapture(Box<EmitDecisionCaptureArgs>),
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

    #[command(flatten)]
    pub grounding: GroundingArgs,
}

/// "What does this decision rest on?" — asked at every capture. Name at least one of:
/// a decision we already made (`--rests-on-decision`), something observed and where
/// (`--rests-on-evidence` with `--evidence-source`), something we assume
/// (`--rests-on-assumption`), or, when there is nothing yet, a declared bet (`--bet`). An id
/// of a node that already exists also counts (`--evidence`, `--hypotheses`). A capture that
/// names none is refused and nothing is written. The decider's own words are not a grounding:
/// they go in `--quote`.
#[derive(Debug, Clone, Args)]
pub struct GroundingArgs {
    /// A decision this one follows from, named the way you would describe it, as `#N` from the
    /// previous ambiguous candidate list, or by `decision-...` id. Repeatable. An ambiguous or
    /// unmatched description refuses the capture and writes nothing.
    #[arg(long = "rests-on-decision", value_name = "DESCRIPTION|#N|DECISION_ID")]
    pub rests_on_decisions: Vec<String>,

    /// Something observed that this decision rests on: the observation itself, not the decider's
    /// opinion of it. Repeatable; creates the evidence in the same call. Pair each with
    /// `--evidence-source`.
    #[arg(long = "rests-on-evidence", value_name = "CONTENT")]
    pub rests_on_evidence: Vec<String>,

    /// Where the matching `--rests-on-evidence` was observed (URL, file@commit, test run,
    /// measurement). Repeatable and index-aligned: give one per `--rests-on-evidence`, or none.
    #[arg(long = "evidence-source", value_name = "REF")]
    pub evidence_sources: Vec<String>,

    /// Something assumed that this decision rests on. Repeatable; creates an assumption in the
    /// same call.
    #[arg(long = "rests-on-assumption", value_name = "STATEMENT")]
    pub rests_on_assumptions: Vec<String>,

    /// Nothing yet: declare this decision a bet. Give the statement being bet on, or leave it
    /// off to record `Judgement call: <title>`.
    #[arg(long = "bet", value_name = "STATEMENT", num_args = 0..=1)]
    pub bet: Option<Option<String>>,

    /// What would change our mind about the `--bet`, in the decider's own words.
    #[arg(long = "would-change-if", requires = "bet")]
    pub would_change_if: Option<String>,

    /// When to check whether the `--bet` paid off. RFC3339 timestamp or YYYY-MM-DD date.
    #[arg(long = "check-by", requires = "bet")]
    pub check_by: Option<String>,

    /// Confidence in the decider's own words. Omit when they expressed none. Capture only:
    /// `ground` refuses it, because it cannot be added to a decision that already exists.
    #[arg(long = "confidence", value_parser = ["low", "medium", "high"])]
    pub confidence: Option<String>,
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

    /// Self-contained why, readable without the source conversation: at least 20 characters
    /// and 4 words, and not a bare reference into an external numbered list like "1a" or
    /// "2. a" — pair `--quote` with `--question` instead of embedding one.
    #[arg(long)]
    pub rationale: String,

    #[arg(long = "topic-keys", value_delimiter = ',')]
    pub topic_keys: Vec<String>,

    #[arg(long = "options", value_delimiter = ',')]
    pub option_ids: Vec<String>,

    #[arg(long = "chose")]
    pub chosen_option_id: Option<String>,

    /// Actor who actually made the decision, when it differs from the recording actor
    /// (`--actor`/`--actor-id`). Requires `--chose`. Immediately advances the decision to
    /// `accepted` via a `decision.accepted` event from this actor — e.g. an agent recording a
    /// decision a human made: `--actor-id agent:claude:session --decided-by human:alex`.
    /// Mutually exclusive with `--still-proposed`.
    #[arg(long = "decided-by")]
    pub decided_by: Option<String>,

    /// The human whose delegated scope this decision falls within, when the recording agent
    /// decided it for itself (`--actor-id agent:...`): the self-acceptance carries this
    /// marker, so "an agent decided within a human's delegation" reads differently from "an
    /// agent decided alone" (no marker). Must be `human:<name>`. Requires `--chose`;
    /// conflicts with `--still-proposed` and with a `--decided-by` naming anyone but the
    /// recording actor. A standing delegation is the same value repeated on each capture in
    /// scope — there is no grant object.
    #[arg(long = "delegated-by")]
    pub delegated_by: Option<String>,

    /// Keep the decision at `proposed` even though `--chose` is set, for a genuine open
    /// recommendation awaiting someone else's decision. By default (this flag absent), a
    /// chosen option means the decision was already made: it auto-accepts immediately after
    /// proposing, from `--decided-by` when given, otherwise self-accepted from the recording
    /// actor.
    #[arg(long = "still-proposed")]
    pub still_proposed: bool,

    #[arg(long = "hypotheses", value_delimiter = ',')]
    pub hypothesis_ids: Vec<String>,

    #[arg(long = "evidence", value_delimiter = ',')]
    pub evidence_ids: Vec<String>,

    /// Verbatim words of the decider, self-contained — not a bare reference like "1a" into
    /// an external numbered list. Requires `--question`. A quote with no stated question is
    /// unreadable once the source conversation is gone (hivemind-zdsh.13).
    #[arg(long = "quote", requires = "question")]
    pub quote: Option<String>,

    /// The question `--quote` answers, spelled out in the capturer's own words. Requires
    /// `--quote`.
    #[arg(long = "question", requires = "quote")]
    pub question: Option<String>,

    /// Registered project handle to file this decision under. An unknown handle is refused
    /// with the register command. Without it the decision is saved to the recorder's
    /// personal project and the reply says so. HiveMind checks the handle; it never works
    /// out the project for you.
    #[arg(long = "project")]
    pub project: Option<String>,

    /// How `--project` was determined (default: stated). Requires `--project`.
    #[arg(long = "project-source", value_enum, requires = "project")]
    pub project_source: Option<ProjectSourceArg>,

    /// When `--project` is absent, work the project out from where this runs: the
    /// `.hivemind-project` files of the folders the uncommitted change touches, else the
    /// nearest one walking up from the working directory, then the project anchored to the
    /// city rig (`GC_RIG`), then this actor's `hivemind project use` setting. Each records
    /// how it was determined. A change touching several projects is recorded for the nearest
    /// project they are all part of; with none in common it is saved to the personal project
    /// and the reply names them. When nothing applies the decision is saved to the personal
    /// project and the reply reminds you the folder is not attached. `--project` wins over
    /// all of these.
    #[arg(long = "project-from-context")]
    pub project_from_context: bool,
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

    /// assumption (default) or bet — a declared gap with nothing behind it yet.
    #[arg(long, value_enum, default_value_t = EmitHypothesisKind::Assumption)]
    pub kind: EmitHypothesisKind,

    /// When to check whether the bet paid off. RFC3339 timestamp or YYYY-MM-DD date.
    #[arg(long = "check-by")]
    pub check_by: Option<String>,

    /// What would change our mind, in the decider's own words.
    #[arg(long = "would-change-if")]
    pub would_change_if: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EmitHypothesisKind {
    Assumption,
    Bet,
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
    /// Links `--from` (the decision) to `--to` (the decision it follows from) — a premise
    /// in the broad sense, distinct from `Supersedes` (the parent still stands).
    #[value(alias = "follows_from")]
    FollowsFrom,
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
    /// "Why was this decided?" — the decision's title, rationale, chosen and rejected options,
    /// who decided, and whether it still holds, plus its one-hop graph (actors, options,
    /// evidence, premises, supersession) with every node labelled. Takes a description or
    /// question as well as --id. Friendlier alias: `why`.
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
    /// The quality profile of one decision: seven dimensions, each with its level and reasons
    /// (or why it was not assessed), attention lines and provenance. No score, no tier.
    #[command(name = "score_decision")]
    ScoreDecision(QueryScoreDecisionArgs),
    /// One page of attention findings: decisions that need a look (a bet past its check date, a
    /// premise that changed, evidence nobody re-checked), each with the dimensions it bears on.
    #[command(name = "scan_decision_quality")]
    ScanDecisionQuality(QueryScanDecisionQualityArgs),
    /// Flag decisions carrying a caller-named "foreign" topic key — a decision
    /// tagged with another ledger's name most likely belongs there instead.
    /// Read-only: report only, never moves anything (see hivemind-s15q C1).
    #[command(name = "scan_misfiled_decisions")]
    ScanMisfiledDecisions(QueryScanMisfiledDecisionsArgs),
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

    /// Ask from this project (a registered handle or a personal address): decisions come from
    /// the project first, then the project it is part of (inherited constraints), then the
    /// projects it depends on, each labelled, and the answer says where it stopped. An unknown
    /// handle is refused. Without it the whole tenant is searched.
    #[arg(long = "project")]
    pub project: Option<String>,
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

    /// Ask from this project (a registered handle or a personal address): matches come from
    /// the project first, then the project it is part of (inherited constraints), then the
    /// projects it depends on, each labelled, and the answer says where it stopped. An unknown
    /// handle is refused. Without it the whole tenant is searched.
    #[arg(long = "project")]
    pub project: Option<String>,
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
    /// Decision ID to profile.
    #[arg(long = "id")]
    pub decision_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct QueryScanDecisionQualityArgs {
    /// Only these kinds of finding (comma-separated): bet_past_check_date, premise_superseded,
    /// premise_rejected, assumption_refuted, bet_failed, evidence_not_rechecked. Default: all.
    #[arg(long = "kind", value_delimiter = ',')]
    pub kinds: Vec<String>,

    /// Days after which the newest evidence linked to a decision counts as not re-checked.
    /// Default 90.
    #[arg(long = "evidence-window-days")]
    pub evidence_window_days: Option<u32>,

    /// Maximum findings to return (1–1000, default 25).
    #[arg(long, default_value_t = SCAN_DEFAULT_LIMIT)]
    pub limit: usize,

    /// Pagination cursor: the `next_cursor` of a previous response.
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryScanMisfiledDecisionsArgs {
    /// Topic keys that indicate a decision belongs to a different ledger
    /// (e.g. another rig's name). At least one is required.
    #[arg(long = "foreign-topic", value_delimiter = ',')]
    pub foreign_topic_keys: Vec<String>,

    /// Maximum results to return (1–1000, default 25).
    #[arg(long, default_value_t = 25)]
    pub limit: usize,

    /// Pagination cursor from a previous response.
    #[arg(long)]
    pub cursor: Option<String>,
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
