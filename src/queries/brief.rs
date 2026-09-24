//! `DecisionBrief`: the shared output contract for fluent follow-up verbs.
//!
//! No existing query response leads with "the decision, then why, then who, then does it still
//! hold" — the pieces exist but are scattered across three deterministic Layer-2 query functions.
//! `get_decision_brief` composes them; it does not duplicate their logic and performs no writes,
//! no ranking, and no LLM calls. See docs/AGENT_FLUENT_QUERYING.md §4.
//!
//! Known gap, stated rather than papered over (AGENTS.md §6, no invented confidence): there is no
//! per-option rejection rationale in the graph schema today, only the decision-level `rationale`.
//! `rejected_options` therefore share that single rationale rather than carrying a distinct
//! reason each — a capture-side schema change would be required to do better, and is out of
//! scope for this bead.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::projector::{GraphParams, GraphValue, GraphView, NodeKind};
use crate::Result;

use super::context::{get_decision_context, ReviewShape};
use super::decision::get_decision;
use super::grounding::{grounding_of_at, GroundingItem, GroundingState, UncheckedBet};
use super::outcome::{get_decision_outcome_at, OutcomeReason};
use super::shared::{node_row, optional_datetime, optional_string, query_error, query_timer_start};
use super::status::DecisionStatus;
use super::QueryResponse;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OptionLabel {
    pub option_id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecidedBy {
    /// The actor who *recorded* this decision (the `PROPOSED_BY` actor) -- often a scribe
    /// writing down someone else's call, not necessarily the one who made it. See
    /// `decider_ids` for who actually decided (hivemind-zdsh.9).
    pub proposer_id: Option<String>,
    /// The actor(s) who actually *decided* (`ACCEPTED_BY` targets). Empty when the decision
    /// hasn't been accepted by anyone yet (`review == Unreviewed`); equal to `proposer_id`
    /// on self-acceptance.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decider_ids: Vec<String>,
    pub source: String,
    pub source_ref: Option<String>,
    pub review: ReviewShape,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StillHolds {
    pub held_up: bool,
    pub reasons: Vec<OutcomeReason>,
    /// Bets whose check date has passed with no evidence either way. Attention, not staleness:
    /// `held_up` is unaffected.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unchecked: Vec<UncheckedBet>,
}

/// Leads with the decision, then why, who decided, and whether it still holds. IDs are output
/// handles for follow-up (`--id`, `--pick`), never required reading to understand the answer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionBrief {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    /// Verbatim words of the decider, self-contained. Always present together with
    /// `question` (hivemind-zdsh.13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// The question `quote` answers, spelled out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// Display-only; `event_origin` stays canonical for resolver ranking (SEARCH_DESIGN.md).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_option: Option<OptionLabel>,
    pub rejected_options: Vec<OptionLabel>,
    pub decided_by: DecidedBy,
    /// What the decision rests on, each item with its state and whether it was named at capture
    /// or attributed later.
    pub rests_on: Vec<GroundingItem>,
    /// Grounded, a declared bet, or nothing declared ("never asked").
    pub grounding_state: GroundingState,
    /// `low`, `medium` or `high`, in the decider's own words at capture; absent when not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expressed_confidence: Option<String>,
    /// How many other decisions follow from this one.
    pub dependents_count: usize,
    pub still_holds: StillHolds,
    pub topic_keys: Vec<String>,
    pub status: DecisionStatus,
}

/// Compose `get_decision` + `get_decision_context` + `get_decision_outcome` plus an
/// `Option.label` lookup and what the decision rests on into one answer. Deterministic graph
/// reads, no write, no LLM. Reads the clock once, to say whether a bet's check date has passed;
/// see `get_decision_brief_at`.
pub fn get_decision_brief(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionBrief>>> {
    get_decision_brief_at(graph, decision_id, Utc::now())
}

/// `get_decision_brief` as of `now`.
pub fn get_decision_brief_at(
    graph: &impl GraphView,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<QueryResponse<Option<DecisionBrief>>> {
    let started = query_timer_start();

    let Some(decision) = get_decision(graph, decision_id)?.data else {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: None,
        });
    };

    let context = get_decision_context(graph, decision_id)?
        .data
        .ok_or_else(|| query_error("decision exists but has no context"))?;
    let outcome = get_decision_outcome_at(graph, decision_id, now)?
        .data
        .ok_or_else(|| query_error("decision exists but has no outcome"))?;
    let (occurred_at, expressed_confidence) = decision_capture_facts(graph, decision_id)?;
    let grounding = grounding_of_at(graph, decision_id, now)?;

    let chosen_option = match &decision.chosen_option_id {
        Some(option_id) => Some(resolve_option_label(graph, option_id)?),
        None => None,
    };
    let mut rejected_options = Vec::with_capacity(decision.option_ids.len());
    for option_id in &decision.option_ids {
        if decision.chosen_option_id.as_deref() == Some(option_id.as_str()) {
            continue;
        }
        rejected_options.push(resolve_option_label(graph, option_id)?);
    }

    let brief = DecisionBrief {
        decision_id: decision.id,
        title: decision.title,
        rationale: decision.rationale,
        quote: decision.quote,
        question: decision.question,
        occurred_at,
        chosen_option,
        rejected_options,
        decided_by: DecidedBy {
            proposer_id: context.proposer_id,
            decider_ids: context.accepted_by,
            source: context.source,
            source_ref: context.source_ref,
            review: context.review,
        },
        rests_on: grounding.items,
        grounding_state: grounding.state,
        expressed_confidence,
        dependents_count: grounding.dependents_count,
        still_holds: StillHolds {
            held_up: outcome.held_up,
            reasons: outcome.reasons,
            unchecked: outcome.unchecked,
        },
        topic_keys: decision.topic_keys,
        status: decision.status,
    };

    Ok(QueryResponse {
        result_count: 1,
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: Some(brief),
    })
}

fn resolve_option_label(graph: &impl GraphView, option_id: &str) -> Result<OptionLabel> {
    let rows = graph.query(
        "MATCH (o:`Option` {id: $id}) RETURN o.label AS label LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(option_id.to_owned()))]),
    )?;
    let label = rows
        .first()
        .and_then(|row| optional_string(row, "label"))
        .unwrap_or_else(|| option_id.to_owned());
    Ok(OptionLabel {
        option_id: option_id.to_owned(),
        label,
    })
}

/// When the decision was captured (display-only) and the confidence its decider expressed then.
fn decision_capture_facts(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<(Option<DateTime<Utc>>, Option<String>)> {
    match node_row(graph, NodeKind::Decision, decision_id)? {
        Some(row) => Ok((
            optional_datetime(&row, "occurred_at")?,
            optional_string(&row, "expressed_confidence"),
        )),
        None => Ok((None, None)),
    }
}

#[cfg(test)]
mod tests;
