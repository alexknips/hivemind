use std::time::Instant;

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::decision::DecisionView;
use super::shared::{
    optional_string, optional_string_list, query_error, read_count, required_string,
    MAX_QUERY_RESULTS,
};
use super::status::{derive_decision_status, DecisionStatus};
use super::QueryResponse;

pub fn get_relevant_decisions(
    graph: &impl GraphView,
    topic: &str,
    status_filter: Option<DecisionStatus>,
) -> Result<QueryResponse<Vec<DecisionView>>> {
    let started = Instant::now();
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(query_error("topic must not be empty").into());
    }

    let normalized_topic = topic.to_owned();
    let count_rows = graph.query(
        "MATCH (d:`Decision`) WHERE $topic IN d.topic_keys RETURN count(d) AS count;",
        &GraphParams::from([(
            "topic".to_owned(),
            GraphValue::String(normalized_topic.clone()),
        )]),
    )?;
    let total_count = read_count(count_rows, "Decision")? as usize;
    let truncated = total_count > MAX_QUERY_RESULTS;

    let decision_rows = graph.query(
        "MATCH (d:`Decision`) WHERE $topic IN d.topic_keys RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys ORDER BY d.id LIMIT 1000;",
        &GraphParams::from([("topic".to_owned(), GraphValue::String(normalized_topic))]),
    )?;

    let mut decisions = Vec::new();
    for row in decision_rows {
        let id = required_string(&row, "id")?;
        let status = derive_decision_status(graph, &id)?;
        if status_filter.is_some_and(|expected| expected != status) {
            continue;
        }

        decisions.push(DecisionView {
            id,
            title: optional_string(&row, "title").unwrap_or_default(),
            rationale: optional_string(&row, "rationale").unwrap_or_default(),
            topic_keys: optional_string_list(&row, "topic_keys"),
            status,
            chosen_option_id: None,
            option_ids: Vec::new(),
            evidence_ids: Vec::new(),
            hypotheses: Vec::new(),
        });
    }

    Ok(QueryResponse {
        result_count: decisions.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: decisions,
    })
}
