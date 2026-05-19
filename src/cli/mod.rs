use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::commands::Commands;
use crate::error::{CliError, CommandError};
use crate::events::{EventProvenance, RelationKind as EventRelationKind};
use crate::ledger::{EventLedger, SqliteEventLedger};
use crate::projector::{
    rebuild_graph, GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind,
    RelationKind as GraphRelationKind,
};
use crate::queries::{
    derive_decision_status, derive_hypothesis_status, get_decision, get_relevant_decisions,
    get_supersession_chain, DecisionStatus, HypothesisStatus,
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
    Emit(EmitArgs),
    Query(QueryArgs),
    Dump(DumpArgs),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QueryDecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Contested,
    Superseded,
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
        Command::Query(query) => run_query(cli, query),
        Command::Dump(dump) => run_dump(cli, dump),
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

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = DumpGraph::default();
            rebuild_graph(&ledger, &graph)?;
            run_query_with_graph(&graph, query)
        }
        GraphBackend::Kuzu => run_query_with_kuzu(&ledger, &cli.hivemind_dir, query),
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
    };

    Ok(json)
}

fn run_dump(cli: &Cli, dump: &DumpArgs) -> Result<String> {
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = DumpGraph::default();
            rebuild_graph(&ledger, &graph)?;
            run_dump_with_graph(&graph, dump)
        }
        GraphBackend::Kuzu => run_dump_with_kuzu(&ledger, &cli.hivemind_dir, dump),
    }
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
            NodeKind::Hypothesis => {
                let statement =
                    graph_property_string(properties, "statement").unwrap_or_else(|| id.clone());
                let status = hypothesis_status_name(derive_hypothesis_status(graph, id)?);
                format!("{statement}\\nstatus: {status}")
            }
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

fn graph_edges(graph: &impl GraphView) -> Result<BTreeSet<DumpEdge>> {
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
            edges.insert(DumpEdge {
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
        NodeKind::Actor => "node.id AS id",
        NodeKind::Evidence => "node.id AS id, node.content AS content",
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
        NodeKind::Actor => {}
        NodeKind::Evidence => insert_if_present(&mut properties, row, "content"),
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
        NodeKind::Actor => "#d5f5e3",
        NodeKind::Evidence => "#fcf3cf",
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
struct DumpEdge {
    relation: GraphRelationKind,
    from_kind: NodeKind,
    from_id: String,
    to_kind: NodeKind,
    to_id: String,
}

#[derive(Debug, Default)]
struct DumpGraph {
    nodes: Mutex<BTreeMap<(NodeKind, String), GraphProperties>>,
    edges: Mutex<BTreeSet<DumpEdge>>,
}

impl GraphView for DumpGraph {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
        let key = (kind, id.to_owned());
        let mut nodes = self.nodes_lock()?;
        let mut existing = nodes
            .get(&(kind, id.to_owned()))
            .cloned()
            .unwrap_or_default();
        existing.extend(properties.clone());
        nodes.insert(key, existing);
        Ok(())
    }

    fn upsert_edge(
        &self,
        kind: GraphRelationKind,
        from_id: &str,
        to_id: &str,
        _properties: &GraphProperties,
    ) -> Result<()> {
        let (from_kind, to_kind) = kind.endpoints();
        let mut edges = self.edges_lock()?;
        edges.insert(DumpEdge {
            relation: kind,
            from_kind,
            from_id: from_id.to_owned(),
            to_kind,
            to_id: to_id.to_owned(),
        });
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        if cypher.contains("RETURN count(rel) AS count;") {
            let relation = query_relation(cypher)?;
            let id = required_param_string(params, "id")?;
            let incoming = cypher.contains("<-[rel:");
            let edges = self.edges_snapshot()?;
            let count = edges
                .iter()
                .filter(|edge| {
                    if edge.relation != relation {
                        return false;
                    }
                    if incoming {
                        edge.to_id == id
                    } else {
                        edge.from_id == id
                    }
                })
                .count();
            let count = i64::try_from(count)
                .map_err(|error| CliError::InvalidInput(format!("count overflow: {error}")))?;
            return Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(count),
            )])]);
        }

        if cypher.contains("RETURN node.id AS id") {
            let kind = query_node_kind(cypher)?;
            let nodes = self.nodes_snapshot()?;
            let mut rows = nodes
                .iter()
                .filter_map(|((node_kind, id), properties)| {
                    if *node_kind != kind {
                        return None;
                    }
                    let mut row =
                        GraphRow::from([("id".to_owned(), GraphValue::String(id.clone()))]);
                    row.extend(properties.clone());
                    Some(row)
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| {
                required_row_string(left, "id")
                    .unwrap_or_default()
                    .cmp(&required_row_string(right, "id").unwrap_or_default())
            });
            return Ok(rows);
        }

        if cypher.contains("RETURN from.id AS from_id, to.id AS to_id") {
            let relation = query_relation(cypher)?;
            let mut rows = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation)
                .map(|edge| {
                    GraphRow::from([
                        ("from_id".to_owned(), GraphValue::String(edge.from_id)),
                        ("to_id".to_owned(), GraphValue::String(edge.to_id)),
                    ])
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| {
                let left_key = format!(
                    "{}:{}",
                    required_row_string(left, "from_id").unwrap_or_default(),
                    required_row_string(left, "to_id").unwrap_or_default()
                );
                let right_key = format!(
                    "{}:{}",
                    required_row_string(right, "from_id").unwrap_or_default(),
                    required_row_string(right, "to_id").unwrap_or_default()
                );
                left_key.cmp(&right_key)
            });
            return Ok(rows);
        }

        if cypher.contains("RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys LIMIT 1;") {
            let decision_id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if let Some(properties) = nodes.get(&(NodeKind::Decision, decision_id.to_owned())) {
                return Ok(vec![GraphRow::from([
                    ("id".to_owned(), GraphValue::String(decision_id.to_owned())),
                    (
                        "title".to_owned(),
                        graph_property_or_default(properties, "title"),
                    ),
                    (
                        "rationale".to_owned(),
                        graph_property_or_default(properties, "rationale"),
                    ),
                    (
                        "topic_keys".to_owned(),
                        graph_property_or_default(properties, "topic_keys"),
                    ),
                ])]);
            }
            return Ok(Vec::new());
        }

        if cypher.contains("RETURN count(d) AS count;") {
            let topic = required_param_string(params, "topic")?;
            let nodes = self.nodes_snapshot()?;
            let count = nodes
                .iter()
                .filter(|((kind, _), properties)| {
                    *kind == NodeKind::Decision
                        && topic_keys(properties)
                            .iter()
                            .any(|candidate| candidate == topic)
                })
                .count();
            let count = i64::try_from(count)
                .map_err(|error| CliError::InvalidInput(format!("count overflow: {error}")))?;
            return Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(count),
            )])]);
        }

        if cypher.contains("WHERE $topic IN d.topic_keys RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys ORDER BY d.id LIMIT 1000;") {
            let topic = required_param_string(params, "topic")?;
            let nodes = self.nodes_snapshot()?;
            let mut decisions = nodes
                .iter()
                .filter_map(|((kind, id), properties)| {
                    if *kind != NodeKind::Decision
                        || !topic_keys(properties).iter().any(|candidate| candidate == topic)
                    {
                        return None;
                    }
                    Some(GraphRow::from([
                        ("id".to_owned(), GraphValue::String(id.clone())),
                        (
                            "title".to_owned(),
                            graph_property_or_default(properties, "title"),
                        ),
                        (
                            "rationale".to_owned(),
                            graph_property_or_default(properties, "rationale"),
                        ),
                        (
                            "topic_keys".to_owned(),
                            graph_property_or_default(properties, "topic_keys"),
                        ),
                    ]))
                })
                .collect::<Vec<_>>();
            decisions.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
            decisions.truncate(1000);
            return Ok(decisions);
        }

        if cypher.contains("RETURN n.id AS") {
            let relation = query_relation(cypher)?;
            let decision_id = required_param_string(params, "id")?;
            let alias = if cypher.contains("AS option_id") {
                "option_id"
            } else if cypher.contains("AS evidence_id") {
                "evidence_id"
            } else if cypher.contains("AS hypothesis_id") {
                "hypothesis_id"
            } else {
                return Err(CliError::InvalidInput(format!(
                    "unknown neighbor alias in query: {cypher}"
                ))
                .into());
            };
            let mut ids = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation && edge.from_id == decision_id)
                .map(|edge| edge.to_id)
                .collect::<Vec<_>>();
            ids.sort();
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([(alias.to_owned(), GraphValue::String(id))]))
                .collect());
        }

        if cypher.contains("MATCH (d:`Decision` {id: $id})-[:`SUPERSEDES`]->(other:`Decision`)") {
            let id = required_param_string(params, "id")?;
            let mut older = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == GraphRelationKind::Supersedes && edge.from_id == id)
                .map(|edge| edge.to_id)
                .collect::<Vec<_>>();
            older.sort();
            return Ok(older
                .into_iter()
                .map(|value| GraphRow::from([("id".to_owned(), GraphValue::String(value))]))
                .collect());
        }

        if cypher.contains("MATCH (other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision` {id: $id})") {
            let id = required_param_string(params, "id")?;
            let mut newer = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == GraphRelationKind::Supersedes && edge.to_id == id)
                .map(|edge| edge.from_id)
                .collect::<Vec<_>>();
            newer.sort();
            return Ok(newer
                .into_iter()
                .map(|value| GraphRow::from([("id".to_owned(), GraphValue::String(value))]))
                .collect());
        }

        Err(CliError::InvalidInput(format!("unsupported query: {cypher}")).into())
    }

    fn wipe(&self) -> Result<()> {
        self.nodes_lock()?.clear();
        self.edges_lock()?.clear();
        Ok(())
    }
}

impl DumpGraph {
    fn nodes_lock(&self) -> Result<MutexGuard<'_, BTreeMap<(NodeKind, String), GraphProperties>>> {
        self.nodes
            .lock()
            .map_err(|error| CliError::InvalidInput(format!("node lock poisoned: {error}")).into())
    }

    fn edges_lock(&self) -> Result<MutexGuard<'_, BTreeSet<DumpEdge>>> {
        self.edges
            .lock()
            .map_err(|error| CliError::InvalidInput(format!("edge lock poisoned: {error}")).into())
    }

    fn nodes_snapshot(&self) -> Result<BTreeMap<(NodeKind, String), GraphProperties>> {
        Ok(self.nodes_lock()?.clone())
    }

    fn edges_snapshot(&self) -> Result<BTreeSet<DumpEdge>> {
        Ok(self.edges_lock()?.clone())
    }
}

fn query_relation(cypher: &str) -> Result<GraphRelationKind> {
    for relation in GraphRelationKind::ALL {
        if cypher.contains(&format!("`{}`", relation.table_name())) {
            return Ok(relation);
        }
    }
    Err(CliError::InvalidInput(format!("unknown relation in query: {cypher}")).into())
}

fn query_node_kind(cypher: &str) -> Result<NodeKind> {
    for kind in NodeKind::ALL {
        if cypher.contains(&format!("`{}`", kind.table_name())) {
            return Ok(kind);
        }
    }
    Err(CliError::InvalidInput(format!("unknown node kind in query: {cypher}")).into())
}

fn required_param_string<'a>(params: &'a GraphParams, key: &str) -> Result<&'a str> {
    match params.get(key) {
        Some(GraphValue::String(value)) => Ok(value),
        _ => Err(CliError::InvalidInput(format!("missing string param: {key}")).into()),
    }
}

fn graph_property_or_default(properties: &GraphProperties, key: &str) -> GraphValue {
    properties.get(key).cloned().unwrap_or(GraphValue::Null)
}

fn topic_keys(properties: &GraphProperties) -> Vec<String> {
    match properties.get("topic_keys") {
        Some(GraphValue::StringList(values)) => values.clone(),
        _ => Vec::new(),
    }
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
            cli.command,
            Command::Emit(EmitArgs {
                command: EmitCommand::EvidenceRecorded(_)
            })
        ));
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
