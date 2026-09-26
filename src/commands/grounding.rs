//! Grounded capture: record what a decision rests on in the same call that proposes it.
//!
//! The verbs (CLI, MCP, REST) resolve a caller's words into a `GroundingPlan` — premise
//! decisions already resolved to ids, plus the new evidence / assumptions / bet to create —
//! and hand it to `Commands::propose_grounded_decision` or `Commands::supersede`. The
//! invariants they enforce are listed in the `commands` module header.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::HypothesisKind;
use crate::ledger::EventLedger;
use crate::util::require_non_empty;
use crate::Result;

use super::{
    bet_statement, generate_entity_id, require_optional_non_empty, AnsweredQuestion, Commands,
    DecisionId, DecisionPlacement, DecisionProposalInput, EvidenceId, Grounding, HypothesisId,
};

/// The refusal for a capture that names nothing it rests on. The verbs wrap this with the four
/// ways to answer; the commands layer states only the rule.
pub const GROUNDING_REQUIRED_MESSAGE: &str =
    "grounding must name at least one premise decision, evidence item or hypothesis (a bet counts)";

/// A new evidence item named at capture: recorded in the same call as the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvidence {
    pub content: String,
    /// Where it was observed (URL, file@commit, test run, measurement).
    pub source: Option<String>,
}

/// A declared bet — "nothing yet" — recorded as a hypothesis of kind `bet`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewBet {
    /// Absent or blank records `Judgement call: <decision title>`.
    pub statement: Option<String>,
    pub would_change_if: Option<String>,
    pub check_by: Option<DateTime<Utc>>,
}

/// What a capture rests on, after the verb resolved every description to an id. Owned data:
/// the verbs build it from wire input, then lend it to the commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroundingPlan {
    /// Premise decisions, each linked `FOLLOWS_FROM`. Must exist; may be stale.
    pub premise_decision_ids: Vec<DecisionId>,
    /// Pre-existing evidence, each linked `BASED_ON`.
    pub evidence_ids: Vec<EvidenceId>,
    /// Pre-existing hypotheses, each linked `ASSUMES`.
    pub hypothesis_ids: Vec<HypothesisId>,
    /// Evidence created by this call, then linked `BASED_ON`.
    pub new_evidence: Vec<NewEvidence>,
    /// Assumptions (hypotheses of kind `assumption`) created by this call, then linked `ASSUMES`.
    pub new_assumptions: Vec<String>,
    /// A bet created by this call, then linked `ASSUMES`.
    pub bet: Option<NewBet>,
}

impl GroundingPlan {
    /// True when the plan names nothing at all — the one shape every capture verb refuses.
    pub fn is_empty(&self) -> bool {
        self.premise_decision_ids.is_empty()
            && self.evidence_ids.is_empty()
            && self.hypothesis_ids.is_empty()
            && self.new_evidence.is_empty()
            && self.new_assumptions.is_empty()
            && self.bet.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestsOnKind {
    Decision,
    Evidence,
    Assumption,
    Bet,
}

/// One thing a decision rests on, as recorded. `label` is the human-readable form (a decision's
/// title, an evidence item's content, a hypothesis statement); the verbs fill in the decision
/// titles they resolved, the commands fill in the nodes they created. Ids-only for a
/// pre-existing evidence or hypothesis id the caller named directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestsOn {
    pub kind: RestsOnKind,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Result of `Commands::propose_grounded_decision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedProposal {
    pub decision_id: DecisionId,
    /// Where the decision was filed and how that was determined.
    pub placement: DecisionPlacement,
    /// Decisions first, then evidence, then assumptions, then the bet — the order of the
    /// `rests on` table in the grounding design.
    pub rests_on: Vec<RestsOn>,
    /// Premise decisions already superseded or rejected when named.
    pub premise_stale: Vec<DecisionId>,
    /// The question the decision answers, when the capture named one.
    pub question: Option<AnsweredQuestion>,
}

/// How new grounding nodes get their ids.
#[derive(Debug, Clone, Copy)]
pub(super) enum IdMode<'a> {
    /// Fresh random ids: a plain capture never repeats.
    Random,
    /// Ids derived from `seed` and each node's content, so an identical retry names the same
    /// nodes (supersede's idempotency).
    Deterministic(&'a str),
}

impl IdMode<'_> {
    fn id(self, prefix: &str, discriminator: &str) -> String {
        match self {
            Self::Random => generate_entity_id(prefix),
            Self::Deterministic(seed) => {
                let stable_name = format!("{seed}\0{prefix}\0{discriminator}");
                format!(
                    "{prefix}-{}",
                    Uuid::new_v5(&Uuid::NAMESPACE_URL, stable_name.as_bytes())
                )
            }
        }
    }
}

/// A plan with ids assigned to every node it will create. Pure: nothing is recorded until
/// `Commands::record_planned_nodes`.
///
/// Each id is stored once. The id lists carry the caller's pre-existing ids first and the ids of
/// the nodes this plan creates after them, so `new_evidence` and `new_assumptions` line up with
/// the tail of `evidence_ids` and `hypothesis_ids` (the bet's id is the last hypothesis id).
pub(super) struct PlannedNodes {
    /// Pre-existing ids first, then new ones, in plan order.
    pub(super) evidence_ids: Vec<EvidenceId>,
    /// Pre-existing ids first, then new assumptions, then the bet.
    pub(super) hypothesis_ids: Vec<HypothesisId>,
    /// How many leading entries of `evidence_ids` the caller named rather than this plan created.
    existing_evidence: usize,
    /// How many leading entries of `hypothesis_ids` the caller named.
    existing_hypotheses: usize,
    new_evidence: Vec<NewEvidence>,
    new_assumptions: Vec<String>,
    /// (resolved statement, bet)
    new_bet: Option<(String, NewBet)>,
}

impl PlannedNodes {
    /// (id, evidence) for every evidence item this plan creates.
    fn new_evidence_nodes(&self) -> impl Iterator<Item = (&EvidenceId, &NewEvidence)> {
        self.evidence_ids
            .iter()
            .skip(self.existing_evidence)
            .zip(&self.new_evidence)
    }

    /// (id, statement) for every assumption this plan creates.
    fn new_assumption_nodes(&self) -> impl Iterator<Item = (&HypothesisId, &String)> {
        self.hypothesis_ids
            .iter()
            .skip(self.existing_hypotheses)
            .zip(&self.new_assumptions)
    }

    /// (id, resolved statement, bet) for the bet this plan creates, if any.
    fn new_bet_node(&self) -> Option<(&HypothesisId, &str, &NewBet)> {
        let (statement, bet) = self.new_bet.as_ref()?;
        let id = self.hypothesis_ids.last()?;
        Some((id, statement.as_str(), bet))
    }

    /// The full `rests_on`: `premise_decision_ids` (no labels — the verb that resolved them
    /// knows their titles) followed by everything this plan records: evidence, then
    /// assumptions, then the bet.
    pub(super) fn rests_on(&self, premise_decision_ids: &[DecisionId]) -> Vec<RestsOn> {
        let existing_evidence = self.evidence_ids.iter().take(self.existing_evidence);
        let existing_hypotheses = self.hypothesis_ids.iter().take(self.existing_hypotheses);
        let mut rests_on = Vec::with_capacity(premise_decision_ids.len() + self.evidence_ids.len());
        rests_on.extend(premise_decision_ids.iter().map(|id| RestsOn {
            kind: RestsOnKind::Decision,
            id: id.clone(),
            label: None,
        }));
        rests_on.extend(existing_evidence.map(|id| RestsOn {
            kind: RestsOnKind::Evidence,
            id: id.clone(),
            label: None,
        }));
        rests_on.extend(self.new_evidence_nodes().map(|(id, evidence)| RestsOn {
            kind: RestsOnKind::Evidence,
            id: id.clone(),
            label: Some(evidence.content.clone()),
        }));
        rests_on.extend(existing_hypotheses.map(|id| RestsOn {
            kind: RestsOnKind::Assumption,
            id: id.clone(),
            label: None,
        }));
        rests_on.extend(self.new_assumption_nodes().map(|(id, statement)| RestsOn {
            kind: RestsOnKind::Assumption,
            id: id.clone(),
            label: Some(statement.clone()),
        }));
        rests_on.extend(self.new_bet_node().map(|(id, statement, _)| RestsOn {
            kind: RestsOnKind::Bet,
            id: id.clone(),
            label: Some(statement.to_owned()),
        }));
        rests_on
    }
}

/// Assign ids to every node `plan` will create. `decision_title` is only read when the bet
/// names no statement of its own.
pub(super) fn plan_grounding_nodes(
    plan: &GroundingPlan,
    decision_title: &str,
    mode: IdMode<'_>,
) -> Result<PlannedNodes> {
    let mut evidence_ids = plan.evidence_ids.clone();
    evidence_ids.extend(
        plan.new_evidence
            .iter()
            .enumerate()
            .map(|(index, evidence)| {
                mode.id(
                    "evidence",
                    &format!(
                        "{index}\0{}\0{}",
                        evidence.content,
                        evidence.source.as_deref().unwrap_or_default()
                    ),
                )
            }),
    );

    let mut hypothesis_ids = plan.hypothesis_ids.clone();
    hypothesis_ids.extend(
        plan.new_assumptions
            .iter()
            .enumerate()
            .map(|(index, statement)| {
                mode.id("hypothesis", &format!("assumption\0{index}\0{statement}"))
            }),
    );

    let new_bet = match &plan.bet {
        Some(bet) => {
            let statement = bet_statement(bet.statement.as_deref(), decision_title)?.into_owned();
            hypothesis_ids.push(mode.id(
                "hypothesis",
                &format!(
                    "bet\0{statement}\0{}\0{}",
                    bet.would_change_if.as_deref().unwrap_or_default(),
                    bet.check_by.map(|at| at.to_rfc3339()).unwrap_or_default()
                ),
            ));
            Some((statement, bet.clone()))
        }
        None => None,
    };

    Ok(PlannedNodes {
        evidence_ids,
        hypothesis_ids,
        existing_evidence: plan.evidence_ids.len(),
        existing_hypotheses: plan.hypothesis_ids.len(),
        new_evidence: plan.new_evidence.clone(),
        new_assumptions: plan.new_assumptions.clone(),
        new_bet,
    })
}

impl<L: EventLedger> Commands<'_, L> {
    /// Propose a decision that rests on `plan`, recording the plan's new evidence, assumptions
    /// and bet first. `input` must leave `grounding` at `NotAsked` and `hypothesis_ids` /
    /// `evidence_ids` empty — the plan is the single source of grounding.
    ///
    /// Every refusal happens before the first write (see the `commands` module header), so a
    /// rejected capture leaves no orphan node behind.
    pub fn propose_grounded_decision(
        &self,
        input: DecisionProposalInput<'_>,
        plan: &GroundingPlan,
    ) -> Result<GroundedProposal> {
        if !matches!(input.grounding, Grounding::NotAsked)
            || !input.hypothesis_ids.is_empty()
            || !input.evidence_ids.is_empty()
        {
            return Err(CommandError::Validation(
                "propose_grounded_decision takes its grounding from the plan; leave grounding NotAsked and hypothesis_ids/evidence_ids empty".to_owned(),
            )
            .into());
        }

        self.validate_grounding_plan(input.actor_id, plan)?;
        let planned = plan_grounding_nodes(plan, input.title, IdMode::Random)?;
        // `validate_proposal` only knows the ids that already exist, so it sees the plan's
        // pre-existing ids; the new nodes are recorded below, after every refusal.
        self.validate_proposal(&DecisionProposalInput {
            hypothesis_ids: &plan.hypothesis_ids,
            evidence_ids: &plan.evidence_ids,
            ..input
        })?;
        super::require_aligned_option_labels(input.option_ids, input.option_labels)?;

        self.record_planned_nodes(input.actor_id, &planned)?;
        let (decision_id, placement, event_ids) =
            self.propose_decision_detailed(DecisionProposalInput {
                grounding: Grounding::Declared {
                    premise_decision_ids: &plan.premise_decision_ids,
                    evidence_ids: &planned.evidence_ids,
                    hypothesis_ids: &planned.hypothesis_ids,
                },
                hypothesis_ids: &planned.hypothesis_ids,
                evidence_ids: &planned.evidence_ids,
                ..input
            })?;

        Ok(GroundedProposal {
            decision_id,
            placement,
            rests_on: planned.rests_on(&plan.premise_decision_ids),
            premise_stale: event_ids.premise_stale,
            question: event_ids.question,
        })
    }

    /// Everything about a plan that can be refused without the decision's id or title: it names
    /// something, its new nodes are well-formed, and its premises exist.
    pub(super) fn validate_grounding_plan(
        &self,
        actor_id: &str,
        plan: &GroundingPlan,
    ) -> Result<()> {
        crate::util::require_valid_actor_id(actor_id)?;
        if plan.is_empty() {
            return Err(CommandError::Validation(GROUNDING_REQUIRED_MESSAGE.to_owned()).into());
        }
        for evidence in &plan.new_evidence {
            require_non_empty("rests-on evidence content", &evidence.content)?;
            require_optional_non_empty("evidence source", evidence.source.as_deref())?;
        }
        for statement in &plan.new_assumptions {
            require_non_empty("rests-on assumption statement", statement)?;
        }
        if let Some(bet) = &plan.bet {
            require_optional_non_empty("would_change_if", bet.would_change_if.as_deref())?;
        }
        for premise_id in &plan.premise_decision_ids {
            self.require_decision_exists(premise_id)?;
        }
        Ok(())
    }

    /// Record every node `planned` creates. Deterministic ids may already exist from an earlier
    /// attempt at the same supersede; those are not recorded twice.
    pub(super) fn record_planned_nodes(
        &self,
        actor_id: &str,
        planned: &PlannedNodes,
    ) -> Result<()> {
        for (id, evidence) in planned.new_evidence_nodes() {
            if self.evidence_exists(id)? {
                continue;
            }
            self.record_evidence_with_id(
                actor_id,
                id,
                &evidence.content,
                evidence.source.as_deref(),
                Uuid::new_v4(),
            )?;
        }
        for (id, statement) in planned.new_assumption_nodes() {
            if self.hypothesis_exists(id)? {
                continue;
            }
            self.record_hypothesis_with_id(
                actor_id,
                id,
                statement,
                HypothesisKind::Assumption,
                None,
                None,
                Uuid::new_v4(),
            )?;
        }
        if let Some((id, statement, bet)) = planned.new_bet_node() {
            if !self.hypothesis_exists(id)? {
                self.record_hypothesis_with_id(
                    actor_id,
                    id,
                    statement,
                    HypothesisKind::Bet,
                    bet.check_by,
                    bet.would_change_if.as_deref(),
                    Uuid::new_v4(),
                )?;
            }
        }
        Ok(())
    }

    /// The subset of `premise_decision_ids` that is superseded or rejected right now.
    pub(super) fn stale_premises(
        &self,
        premise_decision_ids: &[DecisionId],
    ) -> Result<Vec<DecisionId>> {
        let mut stale = Vec::new();
        for premise_id in premise_decision_ids {
            if self.decision_is_stale(premise_id)? {
                stale.push(premise_id.as_str());
            }
        }
        Ok(stale.into_iter().map(ToOwned::to_owned).collect())
    }
}
