//! Contested decisions: where actors disagree, listed so nobody has to go looking (hivemind-bbnw.7).
//!
//! Two situations put a decision on this list, and the list says which:
//!
//! - **Disagreement**: at least one actor accepted the decision and at least one rejected it, and
//!   nobody superseded it. This is the `contested` status, the same rule
//!   [`derive_decision_status`](super::derive_decision_status) applies.
//! - **Conflicting answers**: the decision is accepted and current, and another accepted, current
//!   decision answers the same question with a different chosen option. Both decisions are listed,
//!   each naming the other (the `conflicting_answer` reason of `still_holds`).
//!
//! Nothing here picks a side, ranks the parties, or closes a disagreement: it reports who said
//! what (AGENTS.md §6: disagreement is preserved, never collapsed). Derived from the graph at
//! read time, so a supersession or a change of mind leaves the list on the next call.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::projector::{GraphView, NodeKind};
use crate::Result;

use super::project_label::ProjectLabels;
use super::question::{choice_key, conflicting_answer_sets, QuestionAnswer};
use super::shared::{
    node_rows, normalized_limit, normalized_query, optional_int, optional_string, parse_cursor,
    query_timer_start,
};
use super::status::{DecisionStandings, DecisionStatus};
use super::timeline::asked_and_decided_at;
use super::QueryResponse;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContestedDecisionsRequest {
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContestedDecisionsResults {
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub items: Vec<ContestedDecisionView>,
}

/// Why a decision is listed, and who is on each side.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Contest {
    /// Some actors accepted the decision and some rejected it.
    Disagreement {
        accepted_by: Vec<String>,
        rejected_by: Vec<String>,
    },
    /// Accepted answers to one question that choose differently: this decision and each of
    /// `conflicts_with`.
    ConflictingAnswers {
        question_id: String,
        question: String,
        conflicts_with: Vec<QuestionAnswer>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContestedDecisionView {
    pub decision_id: String,
    pub title: String,
    /// Stable link segment (`/decisions/<slug>`); absent on a decision predating slugs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Address of the project the decision is filed under (see `DecisionView::project`).
    pub project: Option<String>,
    /// What a person calls that project.
    pub project_label: String,
    /// `contested` for a disagreement, `accepted` for a conflicting answer.
    pub status: DecisionStatus,
    /// When the question this decision answers was first explicitly asked, if ever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<DateTime<Utc>>,
    /// When the decision was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
    pub contest: Contest,
}

/// Decisions in contest, oldest first (the ledger offset of the decision's own record, then its
/// id), one page at a time. `truncated` and `next_cursor` continue.
pub fn get_contested_decisions(
    graph: &impl GraphView,
    request: &ContestedDecisionsRequest,
) -> Result<QueryResponse<ContestedDecisionsResults>> {
    let started = query_timer_start();
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let contested = contested_decisions(graph)?;
    let total_matches = contested.len();
    let page: Vec<Contested> = contested.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(page.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    // Only the page is filled in: the decision's words and project, and its ask and record times.
    let decisions = node_rows(graph, NodeKind::Decision)?;
    let labels = ProjectLabels::from_graph(graph)?;
    let mut items = Vec::with_capacity(page.len());
    for found in page {
        let row = decisions.get(&found.decision_id);
        let project = row.and_then(|row| optional_string(row, "project"));
        let (asked_at, decided_at) = match row {
            Some(row) => asked_and_decided_at(graph, row)?,
            None => (None, None),
        };
        items.push(ContestedDecisionView {
            title: row
                .and_then(|row| optional_string(row, "title"))
                .unwrap_or_default(),
            slug: row.and_then(|row| optional_string(row, "slug")),
            project_label: labels.label_of(project.as_deref()),
            project,
            status: found.status,
            asked_at,
            decided_at,
            contest: found.contest,
            decision_id: found.decision_id,
        });
    }

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: ContestedDecisionsResults {
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

/// A decision in contest before its words are filled in.
struct Contested {
    /// The ledger offset of the decision's own record, for ordering.
    origin: Option<i64>,
    decision_id: String,
    status: DecisionStatus,
    contest: Contest,
}

/// Every decision in contest, oldest first: the offset of its own record, then its id.
fn contested_decisions(graph: &impl GraphView) -> Result<Vec<Contested>> {
    let standings = DecisionStandings::load(graph)?;
    let decisions = node_rows(graph, NodeKind::Decision)?;
    let origin_of = |decision_id: &str| {
        decisions
            .get(decision_id)
            .and_then(|row| optional_int(row, "event_origin"))
    };

    let mut found = Vec::new();
    for decision_id in decisions.keys() {
        if standings.status_of(decision_id) != DecisionStatus::Contested {
            continue;
        }
        found.push(Contested {
            origin: origin_of(decision_id),
            decision_id: decision_id.clone(), // ubs:ignore: the list owns its ids
            status: DecisionStatus::Contested,
            contest: Contest::Disagreement {
                accepted_by: standings
                    .deciders_of(decision_id)
                    .into_iter()
                    .map(|decider| decider.id)
                    .collect(),
                rejected_by: standings.rejecters_of(decision_id).to_vec(),
            },
        });
    }
    for set in conflicting_answer_sets(graph)? {
        for answer in &set.answers {
            let own_choice = choice_key(answer);
            let conflicts_with = set
                .answers
                .iter()
                .filter(|other| choice_key(other) != own_choice)
                .cloned()
                .collect();
            found.push(Contested {
                origin: origin_of(&answer.decision_id),
                decision_id: answer.decision_id.clone(), // ubs:ignore: the list owns its ids
                status: DecisionStatus::Accepted,
                contest: Contest::ConflictingAnswers {
                    question_id: set.question_id.clone(), // ubs:ignore: one per answer of the question
                    question: set.text.clone(), // ubs:ignore: one per answer of the question
                    conflicts_with,
                },
            });
        }
    }

    found.sort_by(|left, right| {
        (left.origin, &left.decision_id).cmp(&(right.origin, &right.decision_id))
    });
    Ok(found)
}

#[cfg(test)]
mod tests;
