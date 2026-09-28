//! Waiting requests: explicit asks (hivemind-bbnw.4) with no answering decision yet.
//!
//! A request is "waiting" when its `Ask` node's `question_id` has no `Question` node with an
//! incoming `ANSWERS` edge — derived from the graph at read time, never a stored flag, so a
//! decision answering the question later is reflected the next call, not this one plus a write
//! somewhere else (AGENTS.md §3: query/read layer, no writes).

use std::collections::BTreeSet;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{
    node_rows, normalized_limit, normalized_query, optional_string, parse_cursor,
    relation_edges_by_kind, relation_targets, required_datetime, required_string,
};
use super::QueryResponse;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaitingRequestsRequest {
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WaitingRequestsResults {
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub items: Vec<WaitingRequestView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WaitingRequestView {
    pub request_id: String,
    pub question_id: String,
    pub text: String,
    pub asked_at: DateTime<Utc>,
    pub requested_by: Option<String>,
    pub source_ref: Option<String>,
}

pub fn get_waiting_requests(
    graph: &impl GraphView,
    request: &WaitingRequestsRequest,
) -> Result<QueryResponse<WaitingRequestsResults>> {
    let started = Instant::now();
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let requests = waiting_requests(graph)?;
    let total_matches = requests.len();
    let items: Vec<WaitingRequestView> = requests.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: WaitingRequestsResults {
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

fn waiting_requests(graph: &impl GraphView) -> Result<Vec<WaitingRequestView>> {
    let ask_rows = node_rows(graph, NodeKind::Ask)?;
    let edges = relation_edges_by_kind(graph)?;
    let answered_question_ids: BTreeSet<String> = edges
        .get(&RelationKind::Answers)
        .into_iter()
        .flatten()
        .map(|(_decision_id, question_id)| question_id.clone())
        .collect();

    let mut requests = Vec::new();
    for (id, row) in ask_rows {
        let question_id = required_string(&row, "question_id")?;
        if answered_question_ids.contains(&question_id) {
            continue;
        }
        let requested_by = relation_targets(&edges, &[RelationKind::AskedBy], &id)
            .into_iter()
            .next();
        requests.push(WaitingRequestView {
            request_id: id,
            question_id,
            text: required_string(&row, "text")?,
            asked_at: required_datetime(&row, "asked_at")?,
            requested_by,
            source_ref: optional_string(&row, "source_ref"),
        });
    }
    requests.sort_by(|left, right| {
        (left.asked_at, &left.request_id).cmp(&(right.asked_at, &right.request_id))
    });
    Ok(requests)
}
