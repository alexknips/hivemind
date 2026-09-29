//! Question reads: which decisions answer one question (hivemind-zdsh.16).
//!
//! A decision answers a question when an `ANSWERS` edge links it to a `Question` node, and every
//! decision whose question text matches (lowercase, whitespace collapsed, trailing punctuation
//! dropped) shares that one node. So "the same question answered again" is a supersession chain,
//! and two accepted answers that choose different options are a conflict that is visible here
//! instead of invisible. Pure graph reads: exact match only, no ranking, no model, and nothing
//! is ever resolved for the reader (AGENTS.md §6: disagreement is preserved, never collapsed).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::events::normalize_question_text;
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::brief::resolve_option_label;
use super::shared::{
    neighbor_ids, neighbor_pairs, node_row, node_rows, optional_datetime, optional_int,
    optional_string, query_timer_start, relation_edges, Direction,
};
use super::status::{derive_decision_status, DecisionStatus};
use super::QueryResponse;

/// One decision that answers a question, with where it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuestionAnswer {
    pub decision_id: String,
    pub title: String,
    pub status: DecisionStatus,
    /// The label of the option the decision chose; absent while it chooses nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_option: Option<String>,
}

/// A question and every decision that answers it, in the order they were recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuestionAnswers {
    pub question_id: String,
    pub text: String,
    pub answers: Vec<QuestionAnswer>,
}

/// How `answers_to` is asked which question: by node id, or by the question's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestionKey<'a> {
    Id(&'a str),
    /// Matched exactly on the normalized text, the same rule capture reuses a node by.
    Text(&'a str),
}

/// An answer with the ledger offset of its decision, for ordering "newer" and "in event order".
struct OrderedAnswer {
    event_origin: i64,
    answer: QuestionAnswer,
}

/// Every decision that answers the question `key` names, in event order (the offset of the
/// decision's own proposal, then its id), or `None` when no such question exists. Every status is
/// included: a superseded answer is still part of the history of the question.
pub fn answers_to(
    graph: &impl GraphView,
    key: QuestionKey<'_>,
) -> Result<QueryResponse<Option<QuestionAnswers>>> {
    let started = query_timer_start();
    let found = match key {
        QuestionKey::Id(question_id) => node_row(graph, NodeKind::Question, question_id.trim())?
            .map(|row| {
                (
                    question_id.trim().to_owned(),
                    optional_string(&row, "text").unwrap_or_default(),
                )
            }),
        QuestionKey::Text(text) => {
            let wanted = normalize_question_text(text);
            if wanted.is_empty() {
                None
            } else {
                node_rows(graph, NodeKind::Question)?
                    .into_iter()
                    .find(|(_, row)| {
                        optional_string(row, "normalized_text").as_deref() == Some(wanted.as_str())
                    })
                    .map(|(id, row)| (id, optional_string(&row, "text").unwrap_or_default()))
            }
        }
    };

    let data = match found {
        Some((question_id, text)) => {
            let answers = ordered_answers(graph, &question_id)?
                .into_iter()
                .map(|ordered| ordered.answer)
                .collect();
            Some(QuestionAnswers {
                question_id,
                text,
                answers,
            })
        }
        None => None,
    };
    Ok(QueryResponse {
        result_count: usize::from(data.is_some()),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

/// The id of the question a decision answers, when it names one. A decision answers one question
/// (the write layer refuses a second); if a ledger somehow holds more, the first by id is read.
pub(super) fn question_id_of(graph: &impl GraphView, decision_id: &str) -> Result<Option<String>> {
    Ok(neighbor_pairs(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::Answers,
        NodeKind::Question,
        Direction::Outgoing,
    )?
    .into_iter()
    .next()
    .map(|(question_id, _)| question_id))
}

/// The words of the question a decision answers: its own recorded text, else — for a decision
/// linked to a question after the fact (`ground --answers`) — the text of its question node.
pub(super) fn question_text(
    graph: &impl GraphView,
    own_text: Option<String>,
    question_id: Option<&str>,
) -> Result<Option<String>> {
    match (own_text, question_id) {
        (None, Some(question_id)) => Ok(node_row(graph, NodeKind::Question, question_id)?
            .and_then(|row| optional_string(&row, "text"))),
        (own_text, _) => Ok(own_text),
    }
}

/// The earliest explicit ask for the question `question_id`, if any — the `asked_at` a reader of
/// an answered decision is shown alongside its own `occurred_at` (hivemind-bbnw.4). A question
/// with no `Ask` node (every decision that has ever named it did so via `--question`, never
/// `hivemind ask`) has no ask time; `None` says so rather than guessing.
pub(super) fn earliest_ask(
    graph: &impl GraphView,
    question_id: &str,
) -> Result<Option<DateTime<Utc>>> {
    let mut earliest: Option<DateTime<Utc>> = None;
    for (_, row) in node_rows(graph, NodeKind::Ask)? {
        if optional_string(&row, "question_id").as_deref() != Some(question_id) {
            continue;
        }
        if let Some(asked_at) = optional_datetime(&row, "asked_at")? {
            earliest = Some(match earliest {
                Some(current) if current <= asked_at => current,
                _ => asked_at,
            });
        }
    }
    Ok(earliest)
}

/// One explicit ask of a question, read from its `Ask` node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AskFact {
    /// The request id: the `question.asked` event's uuid, what `capture --answers` takes.
    pub request_id: String,
    pub asked_at: DateTime<Utc>,
    pub requested_by: Option<String>,
    /// The ledger offset of the `question.asked` event (the node's own, real on every backend).
    pub event_origin: Option<i64>,
}

/// Every explicit ask of the question `question_id`, in ledger order. A question nobody asked
/// (only ever named by a capture's `--question`) has none: nothing is guessed.
pub(super) fn asks_for_question(graph: &impl GraphView, question_id: &str) -> Result<Vec<AskFact>> {
    let mut asks = Vec::new();
    for (request_id, row) in node_rows(graph, NodeKind::Ask)? {
        if optional_string(&row, "question_id").as_deref() != Some(question_id) {
            continue;
        }
        let Some(asked_at) = optional_datetime(&row, "asked_at")? else {
            continue;
        };
        let requested_by = neighbor_pairs(
            graph,
            NodeKind::Ask,
            &request_id,
            RelationKind::AskedBy,
            NodeKind::Actor,
            Direction::Outgoing,
        )?
        .into_iter()
        .next()
        .map(|(actor_id, _)| actor_id);
        asks.push(AskFact {
            request_id,
            asked_at,
            requested_by,
            event_origin: optional_int(&row, "event_origin"),
        });
    }
    asks.sort_by(|left, right| {
        (left.event_origin, &left.request_id).cmp(&(right.event_origin, &right.request_id))
    });
    Ok(asks)
}

/// A question that accepted, current decisions answer in different ways.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConflictingAnswers {
    pub question_id: String,
    pub text: String,
    /// Every accepted, non-superseded answer that chose an option, in event order. At least two
    /// of them chose differently; that is why the question is listed.
    pub answers: Vec<QuestionAnswer>,
}

/// Every question whose accepted, non-superseded answers choose different options, sorted by
/// question id. The same rule as `conflicting_answer_ids`, read for all questions at once: only
/// a question two or more decisions answer can conflict, so only those are walked. Nothing is
/// resolved for the reader (AGENTS.md §6).
pub(super) fn conflicting_answer_sets(graph: &impl GraphView) -> Result<Vec<ConflictingAnswers>> {
    let mut answering: BTreeMap<String, usize> = BTreeMap::new();
    for (_decision_id, question_id) in relation_edges(graph, RelationKind::Answers)? {
        *answering.entry(question_id).or_default() += 1;
    }
    let questions = node_rows(graph, NodeKind::Question)?;
    let mut sets = Vec::new();
    for (question_id, answer_count) in answering {
        if answer_count < 2 {
            continue;
        }
        let answers: Vec<QuestionAnswer> = ordered_answers(graph, &question_id)?
            .into_iter()
            .map(|ordered| ordered.answer)
            .filter(|answer| answer.status == DecisionStatus::Accepted)
            .filter(|answer| choice_key(answer).is_some())
            .collect();
        let choices: BTreeSet<String> = answers.iter().filter_map(choice_key).collect();
        if choices.len() < 2 {
            continue;
        }
        let text = questions
            .get(&question_id)
            .and_then(|row| optional_string(row, "text"))
            .unwrap_or_default();
        sets.push(ConflictingAnswers {
            question_id,
            text,
            answers,
        });
    }
    Ok(sets)
}

/// The other decisions that answer the question `decision_id` answers, in event order. Empty
/// when the decision names no question node or nobody else answers it.
pub(super) fn other_answers(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Vec<QuestionAnswer>> {
    let Some(question_id) = question_id_of(graph, decision_id)? else {
        return Ok(Vec::new());
    };
    Ok(ordered_answers(graph, &question_id)?
        .into_iter()
        .map(|ordered| ordered.answer)
        .filter(|answer| answer.decision_id != decision_id)
        .collect())
}

/// The other decisions that answer the same question, are accepted and not superseded, and
/// choose a different option than this decision does. Only meaningful when this decision is
/// itself accepted; otherwise empty. Reported on both sides, never resolved for the reader.
pub(super) fn conflicting_answer_ids(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Vec<String>> {
    let Some(question_id) = question_id_of(graph, decision_id)? else {
        return Ok(Vec::new());
    };
    let answers = ordered_answers(graph, &question_id)?;
    let own = answers
        .iter()
        .find(|ordered| ordered.answer.decision_id == decision_id)
        .filter(|ordered| ordered.answer.status == DecisionStatus::Accepted)
        .and_then(|ordered| choice_key(&ordered.answer));
    let Some(own_choice) = own else {
        return Ok(Vec::new());
    };
    Ok(answers
        .into_iter()
        .filter(|other| {
            other.answer.decision_id != decision_id
                && other.answer.status == DecisionStatus::Accepted
                && choice_key(&other.answer).is_some_and(|choice| choice != own_choice)
        })
        .map(|other| other.answer.decision_id)
        .collect())
}

/// Accepted answers to the question `decision_id` answers that were recorded after it, in event
/// order: what a reader who matched this decision should also see.
pub(super) fn newer_accepted_answers(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Vec<QuestionAnswer>> {
    let Some(question_id) = question_id_of(graph, decision_id)? else {
        return Ok(Vec::new());
    };
    let answers = ordered_answers(graph, &question_id)?;
    let Some(own_origin) = answers
        .iter()
        .find(|ordered| ordered.answer.decision_id == decision_id)
        .map(|ordered| ordered.event_origin)
    else {
        return Ok(Vec::new());
    };
    Ok(answers
        .into_iter()
        .filter(|ordered| {
            ordered.event_origin > own_origin
                && ordered.answer.decision_id != decision_id
                && ordered.answer.status == DecisionStatus::Accepted
        })
        .map(|ordered| ordered.answer)
        .collect())
}

/// What an answer chose, compared case- and space-insensitively so "Postgres" and "postgres"
/// are one choice.
pub(super) fn choice_key(answer: &QuestionAnswer) -> Option<String> {
    answer
        .chosen_option
        .as_deref()
        .map(|label| {
            label
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|key| !key.is_empty())
}

fn ordered_answers(graph: &impl GraphView, question_id: &str) -> Result<Vec<OrderedAnswer>> {
    let decision_ids = neighbor_pairs(
        graph,
        NodeKind::Question,
        question_id,
        RelationKind::Answers,
        NodeKind::Decision,
        Direction::Incoming,
    )?;
    let mut answers: Vec<OrderedAnswer> = Vec::with_capacity(decision_ids.len());
    for (decision_id, _) in decision_ids {
        // The same link asserted by more than one event is still one answer.
        if answers
            .iter()
            .any(|known| known.answer.decision_id == decision_id)
        {
            continue;
        }
        let row = node_row(graph, NodeKind::Decision, &decision_id)?;
        let event_origin = row
            .as_ref()
            .and_then(|row| optional_int(row, "event_origin"))
            .unwrap_or(0);
        let title = row
            .as_ref()
            .and_then(|row| optional_string(row, "title"))
            .unwrap_or_default();
        let chosen_option = neighbor_ids(
            graph,
            &decision_id,
            RelationKind::Chose,
            NodeKind::Option,
            "option_id",
        )?
        .into_iter()
        .next()
        .map(|option_id| resolve_option_label(graph, &option_id).map(|option| option.label))
        .transpose()?;
        let status = derive_decision_status(graph, &decision_id)?;
        answers.push(OrderedAnswer {
            event_origin,
            answer: QuestionAnswer {
                decision_id,
                title,
                status,
                chosen_option,
            },
        });
    }
    answers.sort_by(|left, right| {
        (left.event_origin, &left.answer.decision_id)
            .cmp(&(right.event_origin, &right.answer.decision_id))
    });
    Ok(answers)
}

#[cfg(test)]
mod tests;
