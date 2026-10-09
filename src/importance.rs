//! Which decisions carry the most impact, and who decided them.
//!
//! Layer 3, beside [`crate::quality_profile`] and never part of it: importance is a separate
//! axis, so nothing here touches the seven quality dimensions and nothing returns a composite
//! number. The read is `rank_decisions_by_importance`, on the stdio MCP server, the HTTP MCP
//! endpoint, `GET /v1/decisions/importance` and `hivemind query rank_decisions_by_importance`.
//! It reads what the ledger states, with no model and no network, and writes nothing.
//!
//! # What a decision's importance is read from
//!
//! A decision HAS A BASIS when the record gives a reason it matters. Today the one such reason
//! the record states for most decisions is **reach**: other decisions follow from it
//! (`FOLLOWS_FROM`), counted directly and through chains of such decisions
//! ([`MAX_CHAIN_HOPS`] hops, cycle-safe; `chain_capped` says when the cap bit). It is the same
//! count `dependents_count` gives in the decision brief, except that a decision which follows from
//! itself is not counted as resting on itself.
//!
//! * **Actionability** is the decision's standing: one that is superseded or rejected is not in
//!   force and leaves the default list. It is counted in `left_out`, never dropped silently, and
//!   `include_not_in_force` brings it back.
//! * **Irreversibility** has no recorded source. The decision record does not say what undoing a
//!   decision would cost, so it is never guessed from the text.
//! * A **model's** numbers (`decision.scored` with an `importance` object) are shown on the row as
//!   `model_judged` and **never move the rank**: the list comes out the same on a ledger with no
//!   model and no API key, and every position is explained by something a reader can open.
//!
//! # The order
//!
//! No weights, no product, no invented constant. Decisions with a basis come first, the one with
//! the most decisions resting on it (chains included) leading, then the one with the most resting
//! on it directly, then the newest by ledger offset, then by id, so a tie is never random.
//! Decisions with no basis follow, unranked, reading `not_assessed`, newest first. They are not
//! scored zero: nothing resting on a decision is a fact about the graph, not a verdict on the
//! decision (a new decision has had no time to be built on).
//!
//! # Who decided
//!
//! Each row names the decision's deciders (`ACCEPTED_BY`) and what kind of actor they are, read
//! from the actor-id prefix; the recorder (`PROPOSED_BY`) is shown on its own and is never read as
//! the decider. A decision nobody accepted reads `none_recorded`: nothing is inferred. The report
//! tallies the ranked decisions by kind only, never per person or per agent.
//!
//! # Bounded
//!
//! One page is at most [`MAX_PAGE_SIZE`] rows. `truncated` says whether more follow and
//! `data.next_cursor` resumes after the last row of the page. The cursor is a position in the
//! order, not an offset: a decision recorded between two calls that sorts after it is not skipped
//! and none is repeated.

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::QueryError;
use crate::projector::GraphView;
use crate::queries::{
    get_importance_facts, Decider, DecisionFacts, DecisionStatus, ImportanceFacts,
    ModelJudgedFacts, QueryResponse,
};
use crate::Result;

/// Rows per page when the request does not say.
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// The most rows one page holds.
pub const MAX_PAGE_SIZE: usize = 50;

/// How many hops of `FOLLOWS_FROM` "through chains" follows before it stops and says so.
pub const MAX_CHAIN_HOPS: usize = 5;

/// How many of the directly following decisions a row names. `direct` is the full count.
pub const DIRECT_IDS_SHOWN: usize = 10;

/// Which decisions to list, as a surface states it.
#[derive(Clone, Debug, Default)]
pub struct ImportanceRequest {
    /// Also list decisions that are superseded or rejected. Default false.
    pub include_not_in_force: bool,
    /// Rows per page; [`DEFAULT_PAGE_SIZE`] when 0, and never more than [`MAX_PAGE_SIZE`].
    pub limit: usize,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

/// Whether the record gives a reason the decision matters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    /// Something rests on it. The rank says how much.
    Ranked,
    /// Nothing the record states says it matters. Not a score of zero.
    NotAssessed,
}

/// What follows from a decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestsOnIt {
    /// Decisions that follow from this one directly.
    pub direct: usize,
    /// Decisions that follow from it directly or through a chain of decisions that do, within
    /// [`MAX_CHAIN_HOPS`] hops. Never counts the decision itself.
    pub through_chains: usize,
    /// True when decisions follow beyond the last hop, so `through_chains` is a lower bound.
    pub chain_capped: bool,
    /// Up to [`DIRECT_IDS_SHOWN`] of the directly following decisions, by id.
    pub direct_ids: Vec<String>,
}

/// What kind of actor decided, from the actor-id prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeciderKind {
    /// Every decider is a person (`human:`).
    Person,
    /// Every decider is an agent (`agent:`), with nobody's delegation recorded.
    Agent,
    /// Every decider is an agent and the decision records the person whose delegation it fell
    /// within.
    AgentWithinDelegation,
    /// People and agents both accepted it.
    Mixed,
    /// The deciders carry neither prefix.
    Unknown,
    /// Nobody accepted it. The recorder is not read as the decider.
    NoneRecorded,
}

impl DeciderKind {
    /// The wire name (`agent_within_delegation`), the one serialization gives.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Agent => "agent",
            Self::AgentWithinDelegation => "agent_within_delegation",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
            Self::NoneRecorded => "none_recorded",
        }
    }
}

/// Who decided one decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecidedBy {
    pub kind: DeciderKind,
    /// Who accepted it, sorted by actor id.
    pub deciders: Vec<Decider>,
    /// The person whose delegation an agent's acceptance fell within.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<String>,
    /// Who rejected it, when anyone did (a contested decision lists both sides).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected_by: Vec<String>,
}

/// A model's judgement of the importance factors, shown beside the record and never used to rank.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelJudged {
    pub stakes: f64,
    pub irreversibility: f64,
    pub actionability: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    /// The ledger offset of the assessment, where the model's explanations are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assessed_at_origin: Option<i64>,
}

/// One decision in the list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImportanceRow {
    /// Position among the ranked decisions (1 is the most impactful); absent when not assessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
    pub decision_id: String,
    pub title: String,
    pub project: Option<String>,
    pub project_label: String,
    pub status: DecisionStatus,
    pub occurred_at: Option<DateTime<Utc>>,
    pub importance: Importance,
    pub rests_on_it: RestsOnIt,
    pub decided_by: DecidedBy,
    /// Who recorded it. Often a scribe; never the decider.
    pub recorded_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_judged: Option<ModelJudged>,
    /// The basis for the position, in words.
    pub reasons: Vec<String>,
}

/// Decisions left out of the default list because they are no longer in force.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LeftOut {
    pub superseded: usize,
    pub rejected: usize,
}

/// How the ranked decisions divide by who decided, by kind only.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DeciderTally {
    pub person: usize,
    pub agent: usize,
    pub agent_within_delegation: usize,
    pub mixed: usize,
    pub unknown: usize,
    pub none_recorded: usize,
}

/// One page of the list, with the counts that say how much of it this page is.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImportanceReport {
    /// Decisions with a basis, all pages together.
    pub ranked_total: usize,
    /// Decisions with no basis, all pages together.
    pub not_assessed_total: usize,
    /// What the default list leaves out; zero when `include_not_in_force` was asked.
    pub left_out: LeftOut,
    /// Who decided the ranked decisions, all pages together.
    pub ranked_decided_by: DeciderTally,
    pub decisions: Vec<ImportanceRow>,
    /// Where the next page resumes; present exactly when the response is `truncated`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One page of the decisions ordered by importance. Read-only.
pub fn rank_decisions_by_importance(
    graph: &impl GraphView,
    request: &ImportanceRequest,
) -> Result<QueryResponse<ImportanceReport>> {
    // ubs:ignore: Instant measures response latency only; it does not generate secrets.
    let started = Instant::now();
    let facts = get_importance_facts(graph)?;
    let report = rank(&facts, request)?;
    Ok(QueryResponse {
        result_count: report.decisions.len(),
        truncated: report.next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: report,
    })
}

/// A decision with what rests on it, before it is ordered.
struct Candidate<'a> {
    id: &'a str,
    facts: &'a DecisionFacts,
    direct: &'a [String],
    through_chains: usize,
    chain_capped: bool,
}

impl Candidate<'_> {
    fn has_basis(&self) -> bool {
        self.through_chains > 0
    }

    /// The position in the order, and what a cursor is. Unique per decision (the id ends it).
    fn key(&self) -> OrderKey<'_> {
        (
            u8::from(!self.has_basis()),
            Reverse(self.through_chains),
            Reverse(self.direct.len()),
            Reverse(self.facts.event_origin.unwrap_or(i64::MIN)),
            self.id,
        )
    }
}

type OrderKey<'a> = (u8, Reverse<usize>, Reverse<usize>, Reverse<i64>, &'a str);

/// A position in the order: the key of the last row a page returned.
#[derive(Deserialize, Serialize)]
struct Cursor {
    tier: u8,
    through_chains: usize,
    direct: usize,
    event_origin: i64,
    decision_id: String,
}

impl Cursor {
    fn key(&self) -> OrderKey<'_> {
        (
            self.tier,
            Reverse(self.through_chains),
            Reverse(self.direct),
            Reverse(self.event_origin),
            self.decision_id.as_str(),
        )
    }
}

fn rank(facts: &ImportanceFacts, request: &ImportanceRequest) -> Result<ImportanceReport> {
    let limit = if request.limit == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        request.limit.min(MAX_PAGE_SIZE)
    };
    let after = request.cursor.as_deref().map(parse_cursor).transpose()?;

    let mut left_out = LeftOut::default();
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for (id, decision) in &facts.decisions {
        match decision.status {
            DecisionStatus::Superseded if !request.include_not_in_force => {
                left_out.superseded += 1;
                continue;
            }
            DecisionStatus::Rejected if !request.include_not_in_force => {
                left_out.rejected += 1;
                continue;
            }
            _ => {}
        }
        let direct = facts
            .direct_dependents
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let (through_chains, chain_capped) = reach_through_chains(facts, id);
        candidates.push(Candidate {
            id,
            facts: decision,
            direct,
            through_chains,
            chain_capped,
        });
    }
    candidates.sort_unstable_by(|a, b| a.key().cmp(&b.key()));

    let ranked_total = candidates
        .iter()
        .filter(|candidate| candidate.has_basis())
        .count();
    let mut ranked_decided_by = DeciderTally::default();
    for candidate in candidates.iter().filter(|candidate| candidate.has_basis()) {
        ranked_decided_by.add(decider_kind(candidate.facts));
    }

    let start = after.as_ref().map_or(0, |after| {
        let after = after.key();
        candidates.partition_point(|candidate| candidate.key() <= after)
    });
    let page: Vec<&Candidate<'_>> = candidates.iter().skip(start).take(limit).collect();
    let next_cursor = if start + page.len() < candidates.len() {
        page.last().map(|last| cursor_of(last)).transpose()?
    } else {
        None
    };

    let decisions = page
        .iter()
        .enumerate()
        .map(|(offset, candidate)| row_of(candidate, start + offset + 1))
        .collect();

    Ok(ImportanceReport {
        ranked_total,
        not_assessed_total: candidates.len() - ranked_total,
        left_out,
        ranked_decided_by,
        decisions,
        next_cursor,
    })
}

/// The distinct decisions that follow from `decision_id` within [`MAX_CHAIN_HOPS`] hops, and
/// whether the walk stopped with decisions still ahead. The visited set makes a cycle, or a
/// decision that follows from itself, count each decision once and never the decision itself.
fn reach_through_chains(facts: &ImportanceFacts, decision_id: &str) -> (usize, bool) {
    let Some(first) = facts.direct_dependents.get(decision_id) else {
        return (0, false);
    };
    let mut seen: BTreeSet<&str> = BTreeSet::from([decision_id]);
    let mut frontier: Vec<&str> = Vec::new();
    for follower in first {
        if seen.insert(follower) {
            frontier.push(follower);
        }
    }
    let mut hops = 1;
    while hops < MAX_CHAIN_HOPS && !frontier.is_empty() {
        let mut next = Vec::new();
        for id in &frontier {
            for follower in facts.direct_dependents.get(*id).into_iter().flatten() {
                if seen.insert(follower) {
                    next.push(follower.as_str());
                }
            }
        }
        frontier = next;
        hops += 1;
    }
    let capped = frontier.iter().any(|id| {
        facts
            .direct_dependents
            .get(*id)
            .into_iter()
            .flatten()
            .any(|follower| !seen.contains(follower.as_str()))
    });
    (seen.len() - 1, capped)
}

fn row_of(candidate: &Candidate<'_>, position: usize) -> ImportanceRow {
    let facts = candidate.facts;
    let importance = if candidate.has_basis() {
        Importance::Ranked
    } else {
        Importance::NotAssessed
    };
    let model_judged = facts.model_judged.as_ref().map(model_judged_view);
    let mut reasons = vec![reach_reason(candidate)];
    if let Some(judged) = &facts.model_judged {
        reasons.push(model_reason(judged));
    }
    ImportanceRow {
        rank: candidate.has_basis().then_some(position),
        decision_id: candidate.id.to_owned(),
        title: facts.title.clone(),
        project: facts.project.clone(),
        project_label: facts.project_label.clone(),
        status: facts.status,
        occurred_at: facts.occurred_at,
        importance,
        rests_on_it: RestsOnIt {
            direct: candidate.direct.len(),
            through_chains: candidate.through_chains,
            chain_capped: candidate.chain_capped,
            direct_ids: candidate
                .direct
                .iter()
                .take(DIRECT_IDS_SHOWN)
                .cloned()
                .collect(),
        },
        decided_by: DecidedBy {
            kind: decider_kind(facts),
            deciders: facts.deciders.clone(),
            delegated_by: facts.delegated_by.clone(),
            rejected_by: facts.rejected_by.clone(),
        },
        recorded_by: facts.recorded_by.clone(),
        model_judged,
        reasons,
    }
}

fn reach_reason(candidate: &Candidate<'_>) -> String {
    if !candidate.has_basis() {
        return "not assessed: no decision recorded follows from it, and the record states nothing else about its importance"
            .to_owned();
    }
    let all = candidate.through_chains;
    let direct = candidate.direct.len();
    let noun = |n: usize| if n == 1 { "decision" } else { "decisions" };
    let mut reason = if all == direct {
        format!(
            "{all} {} follow{} from it directly",
            noun(all),
            if all == 1 { "s" } else { "" }
        )
    } else {
        format!(
            "{all} {} follow{} from it: {direct} directly, the rest through chains of decisions that follow from it",
            noun(all),
            if all == 1 { "s" } else { "" }
        )
    };
    if candidate.chain_capped {
        reason.push_str(&format!(
            "; more follow beyond {MAX_CHAIN_HOPS} hops, so this is a lower bound"
        ));
    }
    reason
}

fn model_reason(judged: &ModelJudgedFacts) -> String {
    let by = judged
        .model
        .as_deref()
        .map_or(String::new(), |model| format!(" by {model}"));
    format!(
        "a model{by} judged stakes {}, irreversibility {} and actionability {}: shown, never used to rank",
        judged.stakes, judged.irreversibility, judged.actionability
    )
}

fn model_judged_view(judged: &ModelJudgedFacts) -> ModelJudged {
    ModelJudged {
        stakes: judged.stakes,
        irreversibility: judged.irreversibility,
        actionability: judged.actionability,
        model: judged.model.clone(),
        prompt_version: judged.prompt_version.clone(),
        assessed_at_origin: judged.assessed_at_origin,
    }
}

/// What kind of actor decided: read from the accepters alone, never from the recorder.
fn decider_kind(decision: &DecisionFacts) -> DeciderKind {
    let has = |kind: &str| decision.deciders.iter().any(|decider| decider.kind == kind);
    let (human, agent, unknown) = (has("human"), has("agent"), has("unknown"));
    match (human, agent, unknown) {
        (false, false, false) => DeciderKind::NoneRecorded,
        (true, false, false) => DeciderKind::Person,
        (false, true, false) if decision.delegated_by.is_some() => {
            DeciderKind::AgentWithinDelegation
        }
        (false, true, false) => DeciderKind::Agent,
        (false, false, true) => DeciderKind::Unknown,
        _ => DeciderKind::Mixed,
    }
}

impl DeciderTally {
    fn add(&mut self, kind: DeciderKind) {
        let slot = match kind {
            DeciderKind::Person => &mut self.person,
            DeciderKind::Agent => &mut self.agent,
            DeciderKind::AgentWithinDelegation => &mut self.agent_within_delegation,
            DeciderKind::Mixed => &mut self.mixed,
            DeciderKind::Unknown => &mut self.unknown,
            DeciderKind::NoneRecorded => &mut self.none_recorded,
        };
        *slot += 1;
    }
}

fn cursor_of(last: &Candidate<'_>) -> Result<String> {
    serde_json::to_string(&Cursor {
        tier: u8::from(!last.has_basis()),
        through_chains: last.through_chains,
        direct: last.direct.len(),
        event_origin: last.facts.event_origin.unwrap_or(i64::MIN),
        decision_id: last.id.to_owned(),
    })
    .map_err(|error| QueryError::Execution(format!("cursor could not be written: {error}")).into())
}

fn parse_cursor(cursor: &str) -> Result<Cursor> {
    serde_json::from_str(cursor).map_err(|_| {
        QueryError::Execution("cursor is not one an importance ranking returned".to_owned()).into()
    })
}

#[cfg(test)]
mod tests;
