//! CLI side of `hivemind ground` (hivemind-gwhr.4): give an existing decision what it rests on,
//! after the fact, attributed to whoever runs it.
//!
//! Every refusal — nothing named, an ambiguous or unmatched target or premise, a premise that
//! would close a loop — happens before the first write.

use std::fmt::Write as _;

use crate::cli::args::{Cli, GroundArgs};
use crate::cli::render::{format_ground_output, GroundCommandOutput};
use crate::commands::{CommandContext, Commands};
use crate::error::CliError;
use crate::grounding::{premise_cycle_refusal, GroundingSpec, PremiseTarget};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::get_decision;
use crate::Result;

use super::grounding::{grounding_spec_from_args, resolve_declared_grounding};
use super::{
    candidate_handle_index, cli_tenant, fluent_write_provenance, open_ledger,
    resolve_fluent_target, FluentResolution,
};

/// The refusal for a call that names nothing the decision rests on: the four ways to answer and
/// the existing-node escape hatches.
const GROUND_REFUSAL: &str = "nothing to ground: name at least one thing the decision rests on; nothing was written. Answer with at least one of: \
--rests-on-decision <description|#N|decision-id> (a decision we already made), \
--rests-on-evidence <what was observed> with --evidence-source <where> (something observed), \
--rests-on-assumption <statement> (something we assume), or \
--bet [statement] with optional --would-change-if / --check-by (nothing yet: a declared bet). \
Existing nodes also count: --evidence <id>, --hypotheses <id>";

/// `--confidence` is a capture flag: the decider's own words when the decision was made.
const CONFIDENCE_REFUSAL: &str = "--confidence is the decider's own words at capture and cannot be added to a decision that already exists; drop it. Nothing was written";

pub(super) fn run_ground(cli: &Cli, args: &GroundArgs) -> Result<String> {
    if args.grounding.confidence.is_some() {
        return Err(CliError::InvalidInput(CONFIDENCE_REFUSAL.to_owned()).into());
    }

    let tenant_id = cli_tenant(cli)?;
    let ledger = open_ledger(cli)?;

    // The grounding flags are read before the target is resolved: resolving a description
    // rewrites the `#N` continuation file a premise may point at.
    let spec = grounding_spec_from_args(
        &cli.hivemind_dir,
        &args.grounding,
        &args.evidence_ids,
        &args.hypothesis_ids,
    )?;
    if spec.is_empty() {
        return Err(CliError::InvalidInput(GROUND_REFUSAL.to_owned()).into());
    }

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
    let target = resolve_fluent_target(
        &cli.hivemind_dir,
        !cli.json,
        &graph,
        args.decision_id.as_deref(),
        args.description.as_deref(),
        args.pick,
        args.topic.as_deref(),
    )?;
    let decision_id = match target {
        FluentResolution::Id(decision_id) => decision_id,
        FluentResolution::Output(output) => {
            // The listing just printed replaced the candidate list a `#N` premise handle was
            // read from; a re-run that kept the handle would silently mean a different decision.
            if let Some(handles) = premise_handles(args, &spec) {
                return Err(CliError::InvalidInput(format!(
                    "the decision to ground is ambiguous or unmatched, and its candidate list replaced the one your `#N` premise handle(s) pointed at; nothing was written. Re-run naming the premise(s) by decision id: {handles}\n{output}"
                ))
                .into());
            }
            return Ok(output);
        }
    };

    let resolved = resolve_declared_grounding(&cli.hivemind_dir, &ledger, &tenant_id, spec)?;
    if let Some(refusal) = premise_cycle_refusal(&graph, &decision_id, &resolved.plan)? {
        return Err(CliError::InvalidInput(refusal).into());
    }

    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id, fluent_write_provenance(&cli.actor)),
    );
    let added = commands.ground_decision_with_plan(&cli.actor, &decision_id, &resolved.plan)?;
    let decision_title = get_decision(&graph, &decision_id)?
        .data
        .map(|decision| decision.title);

    format_ground_output(
        cli.json,
        &GroundCommandOutput {
            decision_id: added.decision_id,
            decision_title,
            actor_id: cli.actor.clone(),
            relation_event_ids: added.relation_event_ids,
            rests_on: resolved.label(added.rests_on),
            premise_stale: added.premise_stale,
        },
    )
}

/// The `#N` premise handles of this call, each as `'#N' = <decision id>`, or `None` when the call
/// named no premise by handle. `spec.decisions` holds one entry per `--rests-on-decision`, in
/// order, so it zips with the raw flag values.
fn premise_handles(args: &GroundArgs, spec: &GroundingSpec) -> Option<String> {
    let mut handles = String::new();
    for (raw, premise) in args
        .grounding
        .rests_on_decisions
        .iter()
        .zip(&spec.decisions)
    {
        let raw = raw.trim();
        if candidate_handle_index(raw).is_none() {
            continue;
        }
        if let PremiseTarget::Id(decision_id) = &premise.target {
            if !handles.is_empty() {
                handles.push_str(", ");
            }
            let _ = write!(handles, "'{raw}' = {decision_id}");
        }
    }
    (!handles.is_empty()).then_some(handles)
}
