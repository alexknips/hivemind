//! Later grounding: give a decision that already exists what it rests on, after the fact
//! (hivemind-gwhr.4).
//!
//! The `ground` verbs (CLI, MCP) resolve a caller's words into the same `GroundingPlan` the
//! capture verbs build and hand it to `Commands::ground_decision_with_plan`. What differs from
//! `propose_grounded_decision` is what the audit trail needs: the decision is not created here,
//! so every node and edge recorded names the *grounder* as actor and carries no causation link
//! to the proposal — which is how a reader tells "at capture" from "attributed later" (see
//! `Grounding`). The invariants are listed in the `commands` module header.

use crate::error::CommandError;
use crate::events::EventId;
use crate::ledger::EventLedger;
use crate::util::{require_non_empty, require_valid_actor_id};
use crate::Result;

use super::grounding::{plan_grounding_nodes, IdMode};
use super::{
    require_not_own_premise, AnsweredQuestion, Commands, DecisionId, GroundInput, GroundingPlan,
    RestsOn,
};

/// Result of `Commands::ground_decision_with_plan` / `Commands::ground_and_answer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedAddition {
    pub decision_id: DecisionId,
    /// The `FOLLOWS_FROM` / `BASED_ON` / `ASSUMES` events appended, in `rests_on` order.
    pub relation_event_ids: Vec<EventId>,
    /// What was recorded: decisions, then evidence, then assumptions, then the bet.
    pub rests_on: Vec<RestsOn>,
    /// Premise decisions already superseded or rejected when named.
    pub premise_stale: Vec<DecisionId>,
    /// The question the decision was linked to, when the call named one (`ground --answers`).
    pub question: Option<AnsweredQuestion>,
}

impl<L: EventLedger> Commands<'_, L> {
    /// Record that `decision_id` rests on `plan`, attributed to `actor_id`: the plan's new
    /// evidence, assumptions and bet are recorded first, then every premise decision, evidence
    /// item and hypothesis is linked to the decision without causation.
    ///
    /// Every refusal happens before the first write, so a rejected call leaves no orphan node
    /// behind. Refusing a premise that would close a `FOLLOWS_FROM` loop needs the graph and is
    /// the calling verb's rule (`crate::grounding`).
    pub fn ground_decision_with_plan(
        &self,
        actor_id: &str,
        decision_id: &str,
        plan: &GroundingPlan,
    ) -> Result<GroundedAddition> {
        self.ground_and_answer(actor_id, decision_id, plan, None)
    }

    /// `ground_decision_with_plan`, and/or link the decision to the question `answers` names
    /// (hivemind-zdsh.16): the backfill for a decision captured without saying which question
    /// it answers. An empty `plan` is refused unless `answers` is given; a call that names both
    /// resolves both before the first write.
    pub fn ground_and_answer(
        &self,
        actor_id: &str,
        decision_id: &str,
        plan: &GroundingPlan,
        answers: Option<&str>,
    ) -> Result<GroundedAddition> {
        let names_grounding = !plan.is_empty();
        if names_grounding || answers.is_none() {
            self.validate_grounding_plan(actor_id, plan)?;
        } else {
            require_valid_actor_id(actor_id)?;
        }
        require_non_empty("decision_id", decision_id)?;
        let Some(proposal) = self.decision_proposal_snapshot(decision_id)? else {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        };
        for premise_id in &plan.premise_decision_ids {
            require_not_own_premise(decision_id, premise_id)?;
        }
        for evidence_id in &plan.evidence_ids {
            self.require_evidence_exists(evidence_id)?;
        }
        for hypothesis_id in &plan.hypothesis_ids {
            self.require_hypothesis_exists(hypothesis_id)?;
        }
        let planned = plan_grounding_nodes(plan, &proposal.title, IdMode::Random)?;
        let answer_plan = answers
            .map(|text| self.plan_question_answer(decision_id, text))
            .transpose()?;

        let relation_event_ids = if names_grounding {
            self.record_planned_nodes(actor_id, &planned)?;
            self.ground_decision(GroundInput {
                actor_id,
                decision_id,
                premise_decision_ids: &plan.premise_decision_ids,
                evidence_ids: &planned.evidence_ids,
                hypothesis_ids: &planned.hypothesis_ids,
            })?
        } else {
            Vec::new()
        };
        let question = answer_plan
            .map(|answer_plan| self.record_planned_question_answer(actor_id, &answer_plan))
            .transpose()?;

        Ok(GroundedAddition {
            decision_id: decision_id.to_owned(),
            relation_event_ids,
            rests_on: planned.rests_on(&plan.premise_decision_ids),
            premise_stale: self.stale_premises(&plan.premise_decision_ids)?,
            question,
        })
    }
}

#[cfg(test)]
mod tests;
