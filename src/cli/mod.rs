use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::error::{CliError, CommandError};
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
    #[arg(value_name = "COMMAND")]
    pub command: String,

    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
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

    let output = match &cli.command {
        Command::Emit(command) => {
            if command.command.trim().is_empty() {
                return Err(
                    CliError::InvalidInput("emit command must not be empty".to_owned()).into(),
                );
            }

            StubOutput {
                subcommand: "emit",
                actor: &cli.actor,
                hivemind_dir: &cli.hivemind_dir,
                detail: format!(
                    "stub emit '{}', {} arg(s)",
                    command.command,
                    command.args.len()
                ),
            }
        }
        Command::Query(query) => {
            if query.operation.trim().is_empty() {
                return Err(
                    CliError::InvalidInput("query operation must not be empty".to_owned()).into(),
                );
            }

            StubOutput {
                subcommand: "query",
                actor: &cli.actor,
                hivemind_dir: &cli.hivemind_dir,
                detail: format!(
                    "stub query '{}', {} arg(s)",
                    query.operation,
                    query.args.len()
                ),
            }
        }
        Command::Dump(dump) => StubOutput {
            subcommand: "dump",
            actor: &cli.actor,
            hivemind_dir: &cli.hivemind_dir,
            detail: format!("stub dump format={}", dump_format_name(dump.format)),
        },
    };

    if cli.json {
        serde_json::to_string(&output).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        Ok(format!(
            "{} [{}] actor={} dir={}",
            output.subcommand,
            output.detail,
            output.actor,
            output.hivemind_dir.display()
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
struct StubOutput<'a> {
    subcommand: &'a str,
    actor: &'a str,
    hivemind_dir: &'a PathBuf,
    detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_flags_and_emit_stub() {
        let cli = Cli::parse_from([
            "hivemind",
            "--actor",
            "agent-1",
            "--json",
            "--hivemind-dir",
            "./state",
            "-vv",
            "emit",
            "decision.proposed",
        ]);

        assert_eq!(cli.actor, "agent-1");
        assert!(cli.json);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.hivemind_dir, PathBuf::from("./state"));
        assert!(matches!(cli.command, Command::Emit(_)));
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
