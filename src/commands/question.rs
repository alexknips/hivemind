//! Questions: a decision names or creates the question it answers (hivemind-zdsh.16).
//!
//! A capture (or a later `ground --answers`) hands its question text to this module, which
//! resolves it to a `Question` node by exact match on `events::normalize_question_text` within
//! the tenant: an existing node is reused, otherwise `question.recorded` creates one. The
//! decision is then linked with `relation.added ANSWERS`. Deterministic: no ranking, no model,
//! nothing fuzzier than the normalized text. The rules are listed in the `commands` module
//! header.

use serde::Serialize;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{
    normalize_question_text, Event, EventId, EventPayload, EventType, QuestionRecordedPayload,
    RelationKind,
};
use crate::ledger::EventLedger;
use crate::util::{require_non_empty, require_valid_actor_id};
use crate::Result;

use super::{payload_value_as_str, payload_value_matches, Commands, DecisionId};

pub type QuestionId = String;

/// The question a decision answers, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnsweredQuestion {
    pub question_id: QuestionId,
    /// True when an earlier capture already recorded this question and the decision was linked
    /// to that node; false when this call created the node.
    pub reused: bool,
}

/// Everything about naming a question that can be refused, resolved before the first write:
/// which node the text names (an existing one, or one to create) and whether the decision is
/// already linked to it. `Commands::record_question_answer` performs the writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswerPlan {
    decision_id: DecisionId,
    question_id: QuestionId,
    /// The words to record when no node matches yet; `None` when one does.
    new_question_text: Option<String>,
    already_linked: bool,
}

impl QuestionAnswerPlan {
    pub fn question_id(&self) -> &str {
        &self.question_id
    }

    /// True when the decision already answers this question, so recording it writes nothing.
    pub fn already_linked(&self) -> bool {
        self.already_linked
    }
}

/// The words of a question must contain at least one word once trailing punctuation is dropped:
/// "?" names nothing.
pub(super) fn require_question_text(question: &str) -> Result<()> {
    require_non_empty("question", question)?;
    if normalize_question_text(question).is_empty() {
        return Err(CommandError::Validation(
            "question must contain words, not only punctuation".to_owned(),
        )
        .into());
    }
    Ok(())
}

/// The id a new question gets: derived from the tenant and the normalized text, so two captures
/// racing to create the same question create the same node rather than two.
fn question_id_for(tenant_id: &str, normalized_text: &str) -> QuestionId {
    let stable_name = format!("{tenant_id}\0question\0{normalized_text}");
    format!(
        "question-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, stable_name.as_bytes())
    )
}

/// The event uuids of the two events one capture appends for its question. Derived from the
/// proposal's own uuid so an identical retry of a capture is deduplicated by the ledger rather
/// than recording the link twice.
#[derive(Debug, Clone, Copy)]
pub(super) struct QuestionEventUuids {
    pub(super) question_recorded: Uuid,
    pub(super) answers: Uuid,
}

impl QuestionEventUuids {
    pub(super) fn derived_from(proposal: Uuid) -> Self {
        Self {
            question_recorded: Uuid::new_v5(&proposal, b"question.recorded"),
            answers: Uuid::new_v5(&proposal, b"answers"),
        }
    }

    pub(super) fn random() -> Self {
        Self {
            question_recorded: Uuid::new_v4(),
            answers: Uuid::new_v4(),
        }
    }
}

impl<L: EventLedger> Commands<'_, L> {
    /// Resolve `text` to the question it names and check every refusal, writing nothing.
    /// `decision_id` must exist; a decision answers one question, so naming a different one than
    /// it already answers is refused, and naming the same one again is a no-op.
    pub fn plan_question_answer(
        &self,
        decision_id: &str,
        text: &str,
    ) -> Result<QuestionAnswerPlan> {
        require_non_empty("decision_id", decision_id)?;
        require_question_text(text)?;
        self.require_decision_exists(decision_id)?;
        let mut plan = self.question_answer_plan(decision_id, text)?;
        let linked = self.questions_answered_by(decision_id)?;
        if let Some(other) = linked.iter().find(|linked| **linked != plan.question_id) {
            return Err(CommandError::Invariant(format!(
                "decision {decision_id} already answers question {other}; a decision answers one question"
            ))
            .into());
        }
        plan.already_linked = !linked.is_empty();
        Ok(plan)
    }

    /// The plan for a decision that is being proposed in this same call, so it has no earlier
    /// link and does not exist in the ledger yet.
    pub(super) fn question_answer_plan(
        &self,
        decision_id: &str,
        text: &str,
    ) -> Result<QuestionAnswerPlan> {
        require_question_text(text)?;
        let normalized = normalize_question_text(text);
        let existing = self.find_question(&normalized)?;
        let (question_id, new_question_text) = match existing {
            Some(question_id) => (question_id, None),
            None => (
                question_id_for(self.context.tenant_id.as_str(), &normalized),
                Some(text.trim().to_owned()),
            ),
        };
        Ok(QuestionAnswerPlan {
            decision_id: decision_id.to_owned(),
            question_id,
            new_question_text,
            already_linked: false,
        })
    }

    /// Record a planned answer: `question.recorded` when the node is new, then `ANSWERS`
    /// (decision -> question). `causation` is the proposal event for a capture and `None` for a
    /// later `ground --answers`, which is how "at capture" and "attributed later" stay
    /// distinguishable. Returns the events appended (none when the link already existed).
    pub(super) fn record_question_answer(
        &self,
        actor_id: &str,
        plan: &QuestionAnswerPlan,
        causation: Option<EventId>,
        uuids: QuestionEventUuids,
    ) -> Result<(AnsweredQuestion, Vec<EventId>)> {
        require_valid_actor_id(actor_id)?;
        let answered = AnsweredQuestion {
            question_id: plan.question_id.clone(),
            reused: plan.new_question_text.is_none(),
        };
        if plan.already_linked {
            return Ok((answered, Vec::new()));
        }
        let mut event_ids = Vec::with_capacity(2);
        if let Some(text) = &plan.new_question_text {
            let event = self.event_with_uuid(
                actor_id,
                EventPayload::QuestionRecorded(QuestionRecordedPayload {
                    question_id: plan.question_id.clone(),
                    text: text.clone(),
                }),
                causation,
                uuids.question_recorded,
            )?;
            event_ids.push(self.append_event(event)?);
        }
        event_ids.push(self.append_relation_event_with_uuid(
            actor_id,
            causation.unwrap_or(0),
            RelationKind::Answers,
            &plan.decision_id,
            &plan.question_id,
            uuids.answers,
        )?);
        Ok((answered, event_ids))
    }

    /// Plan and record, in one step: a decision that already exists answers the question `text`
    /// names, attributed to `actor_id` with no causation link to the proposal.
    pub fn answer_question(
        &self,
        actor_id: &str,
        decision_id: &str,
        text: &str,
    ) -> Result<AnsweredQuestion> {
        let plan = self.plan_question_answer(decision_id, text)?;
        self.record_planned_question_answer(actor_id, &plan)
    }

    /// Record a plan made by `plan_question_answer`, attributed to `actor_id` (the grounder, not
    /// the decision's proposer) with no causation link.
    pub fn record_planned_question_answer(
        &self,
        actor_id: &str,
        plan: &QuestionAnswerPlan,
    ) -> Result<AnsweredQuestion> {
        self.record_question_answer(actor_id, plan, None, QuestionEventUuids::random())
            .map(|(answered, _)| answered)
    }

    /// The `Question` node whose text normalizes to `normalized_text`, first recorded first.
    fn find_question(&self, normalized_text: &str) -> Result<Option<QuestionId>> {
        self.find_in_events(|event| {
            if event.event_type != EventType::QuestionRecorded {
                return None;
            }
            let text = payload_value_as_str(event, "text")?;
            if normalize_question_text(text) != normalized_text {
                return None;
            }
            payload_value_as_str(event, "question_id").map(str::to_owned)
        })
    }

    /// The questions `decision_id` is linked to with `ANSWERS`, in ledger order.
    fn questions_answered_by(&self, decision_id: &str) -> Result<Vec<QuestionId>> {
        let mut questions: Vec<QuestionId> = Vec::new();
        self.for_each_event(|event: &Event| {
            if event.event_type == EventType::RelationAdded
                && payload_value_matches(event, "relation", "ANSWERS")
                && payload_value_matches(event, "from_id", decision_id)
            {
                if let Some(question_id) = payload_value_as_str(event, "to_id") {
                    if !questions.iter().any(|known| known == question_id) {
                        questions.push(question_id.to_owned());
                    }
                }
            }
        })?;
        Ok(questions)
    }
}

#[cfg(test)]
mod tests;
