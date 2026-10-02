//! Transcript captures: a capture's own turn, and the ask a request turn makes (hivemind-bbnw.8).
//!
//! The classifier (layer 3) says which turn each capture came from
//! (`CaptureItem::source_turn_id`) and, on a decision or a decision-request, the question in the
//! words it was asked in (`CaptureItem::question`). This module is the write layer's half and it
//! never judges: it checks the named turn exists, reads that turn's own time, and applies
//! mechanical rules. The invariants are listed in the `commands` module header.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{normalize_question_text, CaptureItem, EventId, EventType};
use crate::ledger::EventLedger;
use crate::util::{require_non_empty, require_valid_actor_id};
use crate::Result;

use super::question::QuestionEventUuids;
use super::{payload_value_as_str, CommandContext, Commands};

/// One explicit ask a request turn makes, resolved and checked before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TranscriptAsk {
    /// Who asked: the actor the classifier named on the request, else, when the request turn is
    /// an assistant's, whoever submitted the batch (the agent that spoke the turn).
    actor_id: String,
    text: String,
    /// The request turn's own time.
    ts: DateTime<Utc>,
}

impl TranscriptAsk {
    /// The ask's event uuid, derived from the tenant, the question, the time and the asker, so the
    /// same request turn seen again is the same event.
    fn event_uuid(&self, tenant_id: &str) -> Uuid {
        let stable_name = format!(
            "{tenant_id}\0transcript-ask\0{}\0{}\0{}",
            normalize_question_text(&self.text),
            self.ts.to_rfc3339(),
            self.actor_id
        );
        Uuid::new_v5(&Uuid::NAMESPACE_URL, stable_name.as_bytes())
    }
}

/// A recorded decision capture that states the question it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapturedAnswer {
    /// The capture's position among those recorded: the index its node id carries
    /// (`capture:<event>:<index>`).
    index: usize,
    question: String,
    /// The decision's own turn time, when it has one.
    ts: Option<DateTime<Utc>>,
}

impl CapturedAnswer {
    /// The recorded decision's node id.
    fn decision_id(&self, batch_event_id: EventId) -> String {
        format!("capture:{batch_event_id}:{}", self.index)
    }

    /// The uuid the question events of this answer derive from, stable per classification.
    fn event_uuid(&self, classification_uuid: Uuid) -> Uuid {
        Uuid::new_v5(
            &classification_uuid,
            format!("capture:{}", self.index).as_bytes(),
        )
    }
}

/// The decision captures among `captures` (as recorded) that state a question.
pub(super) fn captured_answers(captures: &[CaptureItem]) -> Vec<CapturedAnswer> {
    captures
        .iter()
        .enumerate()
        .filter(|(_, capture)| capture.kind == "decision")
        .filter_map(|(index, capture)| {
            Some(CapturedAnswer {
                index,
                question: capture.question.clone()?,
                ts: capture.source_ts,
            })
        })
        .collect()
}

/// What one received turn holds that a capture can use.
struct ReceivedTurn {
    /// The speaker's role as the shipper recorded it (`user`, `assistant`, ...).
    role: String,
    /// The turn's own time, `None` when the shipper sent none.
    ts: Option<DateTime<Utc>>,
}

/// What one received batch holds that a capture can name.
struct ReceivedBatch {
    submitter: String,
    turns: HashMap<String, ReceivedTurn>,
}

/// The role of a turn an agent spoke. Only such a turn lets the batch's submitter stand in for
/// an asker the classifier did not name: the submitter is that agent, so crediting it is a
/// recorded fact. A human's turn is not, and no other role says who spoke.
const ASSISTANT_ROLE: &str = "assistant";

/// Checks what one capture says about its turn and question, before anything is looked up.
fn check_capture(index: usize, capture: &CaptureItem) -> Result<()> {
    if let Some(turn_id) = &capture.source_turn_id {
        require_non_empty("source_turn_id", turn_id)?;
    }
    let Some(question) = &capture.question else {
        return Ok(());
    };
    if !matches!(capture.kind.as_str(), "decision" | "decision-request") {
        return Err(CommandError::Validation(format!(
            "capture {index} is a {}, and only a decision or a decision-request can carry a question: leave question out",
            capture.kind
        ))
        .into());
    }
    if normalize_question_text(question).is_empty() {
        return Err(CommandError::Validation(format!(
            "capture {index} has a question with no words in it: write the question as it was asked, or leave question out"
        ))
        .into());
    }
    Ok(())
}

/// The first of `batch_ids`, in order, whose received batch holds `turn_id`, with that turn.
fn find_turn<'a>(
    received: &'a HashMap<String, ReceivedBatch>,
    batch_ids: &[String],
    index: usize,
    turn_id: &str,
) -> Result<(&'a ReceivedBatch, &'a ReceivedTurn)> {
    batch_ids
        .iter()
        .find_map(|batch_id| {
            let batch = received.get(batch_id.as_str())?;
            Some((batch, batch.turns.get(turn_id)?))
        })
        .ok_or_else(|| {
            CommandError::Validation(format!(
                "capture {index} names turn {turn_id}, which is not a turn of the batches this classification covers: name one of their turn ids, or leave source_turn_id out"
            ))
            .into()
        })
}

impl<L: EventLedger> Commands<'_, L> {
    /// Checks the turn and question every capture of one classification carries, reads each
    /// named turn's own time into `source_ts`, and returns the asks the request turns make.
    /// Refuses, with nothing written, a `source_turn_id` that is blank or names no turn of the
    /// batches covered, and a `question` on any kind but a decision or decision-request, or
    /// with no words in it.
    ///
    /// Whatever a submission carried in `source_ts` is replaced: only a received turn's own
    /// time counts. A request turn with no time makes no ask, because the ask would then be
    /// dated by when it was classified.
    ///
    /// The asker is the actor the classifier named on the request. When it named none, the turn's
    /// recorded role decides: an assistant's turn is credited to the batch's submitter (the agent
    /// that spoke it), and any other turn, a human's above all, makes no ask, because the batch
    /// does not record who that human is and a human's question is never credited to an agent.
    pub(super) fn plan_transcript_asks(
        &self,
        batch_ids: &[String],
        captures: &mut [CaptureItem],
    ) -> Result<Vec<TranscriptAsk>> {
        for (index, capture) in captures.iter_mut().enumerate() {
            capture.source_ts = None;
            check_capture(index, capture)?;
        }
        if captures
            .iter()
            .all(|capture| capture.source_turn_id.is_none())
        {
            return Ok(Vec::new());
        }

        let received = self.received_batches(batch_ids)?;
        let mut asks = Vec::new();
        for (index, capture) in captures.iter_mut().enumerate() {
            let Some(turn_id) = capture.source_turn_id.as_deref() else {
                continue;
            };
            let (batch, turn) = find_turn(&received, batch_ids, index, turn_id)?;
            capture.source_ts = turn.ts;
            let (Some(ts), Some(question), "decision-request") =
                (turn.ts, capture.question.as_deref(), capture.kind.as_str())
            else {
                continue;
            };
            let named_asker = capture
                .actor_id
                .as_deref()
                .filter(|actor_id| require_valid_actor_id(actor_id).is_ok());
            let asker = named_asker
                .or_else(|| (turn.role == ASSISTANT_ROLE).then_some(batch.submitter.as_str()));
            let Some(asker) = asker else {
                continue;
            };
            asks.push(TranscriptAsk {
                actor_id: asker.to_owned(),       // ubs:ignore: one owned asker per ask
                text: question.trim().to_owned(), // ubs:ignore: one owned text per ask
                ts,
            });
        }
        Ok(asks)
    }

    /// Writes each ask at its request turn's own time, by the one who asked. An ask written
    /// before is the same ask seen again (a re-ingested transcript carries the same turn times),
    /// so its event uuid is derived from the question, the time and the asker and the ledger
    /// deduplicates it; every other ask is never suppressed (see `record_ask`).
    pub(super) fn record_transcript_asks(&self, asks: &[TranscriptAsk]) -> Result<()> {
        for ask in asks {
            let commands = self.at_source_time(Some(ask.ts));
            let plan = commands.plan_ask(&ask.text)?;
            commands.record_ask_with_uuid(
                &ask.actor_id,
                &plan,
                ask.event_uuid(self.context.tenant_id.as_str()),
            )?;
        }
        Ok(())
    }

    /// Links each recorded decision capture that states a question to that question, the way
    /// `capture --question` links a decision: `question.recorded` when no node matches the
    /// normalized text yet, then `ANSWERS`. Written at the decision's own turn time when it has
    /// one, attributed to `actor_id` (the recorder) and caused by the classification event.
    pub(super) fn record_captured_answers(
        &self,
        actor_id: &str,
        batch_event_id: EventId,
        classification_uuid: Uuid,
        answers: &[CapturedAnswer],
    ) -> Result<()> {
        for answer in answers {
            let commands = self.at_source_time(answer.ts);
            let plan = commands
                .question_answer_plan(&answer.decision_id(batch_event_id), &answer.question)?;
            let uuids = QuestionEventUuids::derived_from(answer.event_uuid(classification_uuid));
            commands.record_question_answer(actor_id, &plan, Some(batch_event_id), uuids)?;
        }
        Ok(())
    }

    /// A handle that writes like this one, with every event it appends dated `ts` (the source's
    /// own time) when there is one.
    fn at_source_time(&self, ts: Option<DateTime<Utc>>) -> Commands<'_, L> {
        Commands::new_with_context(
            self.ledger,
            CommandContext {
                event_ts: ts.or(self.context.event_ts),
                ..self.context.clone()
            },
        )
    }

    /// The received batches among `batch_ids`: who submitted each and each turn's own time.
    fn received_batches(&self, batch_ids: &[String]) -> Result<HashMap<String, ReceivedBatch>> {
        let wanted: HashSet<&str> = batch_ids.iter().map(String::as_str).collect();
        let mut received: HashMap<String, ReceivedBatch> = HashMap::new();
        self.for_each_event(|event| {
            if event.event_type != EventType::IngestBatchReceived {
                return;
            }
            let Some(batch_id) = payload_value_as_str(event, "batch_id") else {
                return;
            };
            if !wanted.contains(batch_id) {
                return;
            }
            let batch = received
                .entry(batch_id.to_owned())
                .or_insert_with(|| ReceivedBatch {
                    submitter: event.actor_id.clone(),
                    turns: HashMap::new(),
                });
            let turns = event
                .payload
                .get("turns")
                .and_then(|turns| turns.as_array())
                .into_iter()
                .flatten();
            for turn in turns {
                let Some(turn_id) = turn.get("turn_id").and_then(|id| id.as_str()) else {
                    continue;
                };
                let role = turn.get("role").and_then(|role| role.as_str());
                let ts = turn
                    .get("ts")
                    .and_then(|ts| ts.as_str())
                    .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
                    .map(|ts| ts.with_timezone(&Utc));
                batch
                    .turns
                    .entry(turn_id.to_owned()) // ubs:ignore: one owned key per received turn
                    .or_insert_with(|| ReceivedTurn {
                        role: role.unwrap_or_default().to_owned(), // ubs:ignore: one owned role per received turn
                        ts,
                    });
            }
        })?;
        Ok(received)
    }
}

#[cfg(test)]
mod tests;
