use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::commands::Commands;
use crate::error::{CliError, CommandError};
use crate::events::RelationKind;
use crate::ledger::SqliteEventLedger;
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

    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
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
    BasedOn,
}

#[derive(Debug, Clone, Args)]
pub struct QueryArgs {
    #[arg(value_name = "OP")]
    pub operation: String,

    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
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
        Command::Query(query) => {
            if query.operation.trim().is_empty() {
                return Err(
                    CliError::InvalidInput("query operation must not be empty".to_owned()).into(),
                );
            }

            format_output(
                cli.json,
                &OutputEnvelope::new(
                    "query",
                    "stub",
                    format!(
                        "stub query '{}', {} arg(s)",
                        query.operation,
                        query.args.len()
                    ),
                ),
            )
        }
        Command::Dump(dump) => format_output(
            cli.json,
            &OutputEnvelope::new(
                "dump",
                "stub",
                format!("stub dump format={}", dump_format_name(dump.format)),
            ),
        ),
    }
}

fn run_emit(cli: &Cli, emit: &EmitArgs) -> Result<String> {
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new(&ledger);

    let output = match &emit.command {
        EmitCommand::DecisionProposed(args) => {
            let mut option_ids = Vec::with_capacity(args.option_ids.len());
            let mut chosen_option_id = None;
            for option_label in &args.option_ids {
                let option_id = commands.record_option(
                    &cli.actor,
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

            let decision_id = commands.propose_decision(
                &cli.actor,
                &args.title,
                &args.rationale,
                &args.topic_keys,
                &option_ids,
                chosen_option_id.as_deref(),
                &args.hypothesis_ids,
                &args.evidence_ids,
            )?;
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
                    RelationKind::Supports,
                    &cli.actor,
                )?,
                EmitRelationKind::Refutes => commands.relate_evidence_to_hypothesis(
                    &args.from_id,
                    &args.to_id,
                    RelationKind::Refutes,
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

fn validate_global_flags(cli: &Cli) -> Result<()> {
    if cli.actor.trim().is_empty() {
        return Err(CliError::InvalidInput("--actor must not be empty".to_owned()).into());
    }

    Ok(())
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

const fn dump_format_name(format: DumpFormat) -> &'static str {
    match format {
        DumpFormat::Dot => "dot",
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
        assert!(matches!(
            cli.command,
            Command::Emit(EmitArgs {
                command: EmitCommand::EvidenceRecorded(_)
            })
        ));
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
}
