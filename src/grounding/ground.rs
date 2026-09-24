//! The rule of `ground` that needs the graph (hivemind-gwhr.4): a premise must not close a loop.
//!
//! The commands layer refuses a premise that does not exist and a decision that names itself,
//! from the ledger alone. Whether a premise *already rests on* the decision being grounded takes a
//! walk over the projected `FOLLOWS_FROM` edges, so it is a verb-level rule shared by the CLI and
//! MCP `ground` verbs and applied before anything is written.

use std::fmt::Write as _;

use crate::commands::GroundingPlan;
use crate::projector::GraphView;
use crate::queries::{follows_from_path, get_decision};
use crate::Result;

const SELF_PREMISE_REFUSAL: &str = "a decision cannot be its own premise; nothing was written";

/// The refusal for the first premise in `plan` that would close a loop of decisions resting on
/// each other, or `None` when every premise is safe. The message names the chain that makes it a
/// loop; nothing has been written when a caller returns it.
pub(crate) fn premise_cycle_refusal(
    graph: &impl GraphView,
    decision_id: &str,
    plan: &GroundingPlan,
) -> Result<Option<String>> {
    if plan
        .premise_decision_ids
        .iter()
        .any(|premise_id| premise_id == decision_id)
    {
        return Ok(Some(SELF_PREMISE_REFUSAL.to_owned()));
    }
    for premise_id in &plan.premise_decision_ids {
        if let Some(chain) = follows_from_path(graph, premise_id, decision_id)? {
            return cycle_message(graph, decision_id, premise_id, &chain).map(Some);
        }
    }
    Ok(None)
}

/// `"<premise>" already rests on "<decision>" (<chain>), so ...`: the refusal for a premise whose
/// `chain` of `FOLLOWS_FROM` links leads back to the decision being grounded.
fn cycle_message(
    graph: &impl GraphView,
    decision_id: &str,
    premise_id: &str,
    chain: &[String],
) -> Result<String> {
    let mut message = String::new();
    push_decision_label(&mut message, graph, premise_id)?;
    message.push_str(" already rests on ");
    push_decision_label(&mut message, graph, decision_id)?;
    message.push_str(" (");
    for (index, link) in chain.iter().enumerate() {
        if index > 0 {
            message.push_str(" rests on ");
        }
        push_decision_label(&mut message, graph, link)?;
    }
    message.push_str("), so making it a premise would close a loop of decisions resting on each other; nothing was written");
    Ok(message)
}

/// `"<title>" (<id>)`, or just the id for a decision the graph has no title for.
fn push_decision_label(out: &mut String, graph: &impl GraphView, decision_id: &str) -> Result<()> {
    match get_decision(graph, decision_id)?.data {
        Some(decision) => {
            let _ = write!(out, "\"{}\" ({decision_id})", decision.title);
        }
        None => out.push_str(decision_id),
    }
    Ok(())
}
