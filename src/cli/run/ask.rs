//! CLI side of `hivemind ask` (hivemind-bbnw.4): record that you are explicitly asking a
//! question, before any decision answers it. Resolved and created exactly like a capture's
//! `--question`: an existing `Question` node is reused, otherwise `question.recorded` creates
//! one. Unlike answering, asking is never suppressed as a duplicate — the same question can be
//! asked more than once, each its own outstanding request.

use crate::cli::args::{AskArgs, Cli};
use crate::cli::render::{format_ask_output, AskCommandOutput};
use crate::commands::{CommandContext, Commands};
use crate::Result;

use super::{cli_tenant, fluent_write_provenance, open_ledger};

pub(super) fn run_ask(cli: &Cli, args: &AskArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = open_ledger(cli)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id, fluent_write_provenance(&cli.actor)),
    );

    let plan = commands.plan_ask(&args.text)?;
    let recorded = commands.record_ask(&cli.actor, &plan)?;

    format_ask_output(
        cli.json,
        &AskCommandOutput {
            request_id: recorded.request_id,
            question_id: recorded.question_id,
            text: args.text.trim().to_owned(),
            actor_id: cli.actor.clone(),
            reused: recorded.reused,
        },
    )
}
