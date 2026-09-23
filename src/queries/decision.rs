//! Single-decision retrieval with full provenance, status derivation, and related-node expansion.

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::events::HypothesisKind;
use crate::projector::{GraphParams, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::grounding::{
    grounding_state_of, hypothesis_facts, premise_decision_ids, rests_on_clause, GroundingState,
};
use super::shared::{
    neighbor_ids, optional_string, optional_string_list, premised_on_hypothesis_ids,
    required_string,
};
use super::status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
use super::QueryResponse;

/// An assumption or a declared bet a decision premises on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HypothesisContext {
    pub id: String,
    pub status: HypothesisStatus,
    /// `assumption` (did it hold?) or `bet` (did we check?).
    pub kind: HypothesisKind,
    /// When to check a bet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_by: Option<DateTime<Utc>>,
    /// What would change our mind, in the decider's own words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub would_change_if: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionView {
    pub id: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub status: DecisionStatus,
    pub chosen_option_id: Option<String>,
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypotheses: Vec<HypothesisContext>,
    /// Prior decisions this decision follows from (`FOLLOWS_FROM`): a premise in the broad
    /// sense. Ids only; `get_decision_brief` carries the states and labels.
    pub premise_decision_ids: Vec<String>,
    /// Verbatim words of the decider, self-contained. Always present together with
    /// `question` (hivemind-zdsh.13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// The question `quote` answers, spelled out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

/// The single-decision row shape every backend's `query` recognizes (memory.rs matches this
/// text verbatim), shared so `get_decision_title` rides the same supported shape.
const DECISION_ROW_QUERY: &str = "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys, d.quote AS quote, d.question AS question LIMIT 1;";

impl DecisionView {
    /// Grounded, a declared bet, or nothing declared, from the premise, evidence and hypothesis
    /// ids this view carries. Only meaningful on a full view (`get_decision`, search results);
    /// the shallow views `get_relevant_decisions` returns carry none of them.
    pub fn grounding_state(&self) -> GroundingState {
        grounding_state_of(
            self.premise_decision_ids.len(),
            self.evidence_ids.len(),
            self.hypotheses.iter().map(|hypothesis| hypothesis.kind),
        )
    }

    /// The one-line digest form: `rests on: decision x1, evidence x2, bet (check by 2026-10-15)`.
    pub fn rests_on_clause(&self) -> String {
        let assumptions = self
            .hypotheses
            .iter()
            .filter(|hypothesis| hypothesis.kind == HypothesisKind::Assumption)
            .count();
        let bets: Vec<Option<DateTime<Utc>>> = self
            .hypotheses
            .iter()
            .filter(|hypothesis| hypothesis.kind == HypothesisKind::Bet)
            .map(|hypothesis| hypothesis.check_by)
            .collect();
        rests_on_clause(
            self.premise_decision_ids.len(),
            self.evidence_ids.len(),
            assumptions,
            &bets,
        )
    }
}

pub fn get_decision(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionView>>> {
    let started = Instant::now();
    let rows = graph.query(
        DECISION_ROW_QUERY,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let data = if let Some(row) = rows.first() {
        let id = required_string(row, "id")?;
        let title = optional_string(row, "title").unwrap_or_default();
        let rationale = optional_string(row, "rationale").unwrap_or_default();
        let topic_keys = optional_string_list(row, "topic_keys");
        let quote = optional_string(row, "quote");
        let question = optional_string(row, "question");
        let status = derive_decision_status(graph, &id)?;
        let option_ids = neighbor_ids(
            graph,
            &id,
            RelationKind::HasOption,
            NodeKind::Option,
            "option_id",
        )?;
        let chosen_option_id = neighbor_ids(
            graph,
            &id,
            RelationKind::Chose,
            NodeKind::Option,
            "option_id",
        )?
        .into_iter()
        .next();
        let evidence_ids = neighbor_ids(
            graph,
            &id,
            RelationKind::BasedOn,
            NodeKind::Evidence,
            "evidence_id",
        )?;
        let premise_decision_ids = premise_decision_ids(graph, &id)?;
        let hypothesis_ids = premised_on_hypothesis_ids(graph, &id)?;
        let mut hypotheses = Vec::with_capacity(hypothesis_ids.len());
        for hypothesis_id in hypothesis_ids {
            let facts = hypothesis_facts(graph, &hypothesis_id)?;
            hypotheses.push(HypothesisContext {
                status: derive_hypothesis_status(graph, &hypothesis_id)?,
                id: hypothesis_id,
                kind: facts.kind,
                check_by: facts.check_by,
                would_change_if: facts.would_change_if,
            });
        }

        Some(DecisionView {
            id,
            title,
            rationale,
            topic_keys,
            status,
            chosen_option_id,
            option_ids,
            evidence_ids,
            hypotheses,
            premise_decision_ids,
            quote,
            question,
        })
    } else {
        None
    };

    Ok(QueryResponse {
        result_count: usize::from(data.is_some()),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

pub fn get_hypothesis_statement(
    graph: &impl GraphView,
    hypothesis_id: &str,
) -> crate::Result<Option<String>> {
    let rows = graph.query(
        "MATCH (h:`Hypothesis` {id: $id}) RETURN h.statement AS statement LIMIT 1;",
        &GraphParams::from([(
            "id".to_owned(),
            GraphValue::String(hypothesis_id.to_owned()),
        )]),
    )?;
    Ok(rows
        .first()
        .and_then(|row| optional_string(row, "statement")))
}

/// A decision's title alone, for labelling a neighbouring decision without `get_decision`'s
/// status and edge lookups.
pub(super) fn get_decision_title(
    graph: &impl GraphView,
    decision_id: &str,
) -> crate::Result<Option<String>> {
    let rows = graph.query(
        DECISION_ROW_QUERY,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(rows.first().and_then(|row| optional_string(row, "title")))
}

pub(super) fn get_evidence_content(
    graph: &impl GraphView,
    evidence_id: &str,
) -> crate::Result<Option<String>> {
    let rows = graph.query(
        "MATCH (e:`Evidence` {id: $id}) RETURN e.content AS content LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(evidence_id.to_owned()))]),
    )?;
    Ok(rows.first().and_then(|row| optional_string(row, "content")))
}
