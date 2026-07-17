use std::time::Instant;

use serde::Serialize;

use crate::projector::{GraphParams, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{neighbor_ids, optional_string, optional_string_list, required_string};
use super::status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
use super::QueryResponse;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HypothesisContext {
    pub id: String,
    pub status: HypothesisStatus,
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
}

pub fn get_decision(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionView>>> {
    let started = Instant::now();
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let data = if let Some(row) = rows.first() {
        let id = required_string(row, "id")?;
        let title = optional_string(row, "title").unwrap_or_default();
        let rationale = optional_string(row, "rationale").unwrap_or_default();
        let topic_keys = optional_string_list(row, "topic_keys");
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
        let hypothesis_ids = neighbor_ids(
            graph,
            &id,
            RelationKind::PremisedOn,
            NodeKind::Hypothesis,
            "hypothesis_id",
        )?;
        let mut hypotheses = Vec::with_capacity(hypothesis_ids.len());
        for hypothesis_id in hypothesis_ids {
            hypotheses.push(HypothesisContext {
                status: derive_hypothesis_status(graph, &hypothesis_id)?,
                id: hypothesis_id,
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
