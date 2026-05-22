use std::collections::BTreeSet;

use serde::Serialize;

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::Result;

use super::shared::{query_error, query_timer_start, required_string};
use super::QueryResponse;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SupersessionChain {
    pub decision_ids: Vec<String>,
    pub input_index: usize,
}

pub fn get_supersession_chain(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<SupersessionChain>> {
    let started = query_timer_start();
    if decision_id.trim().is_empty() {
        return Err(query_error("decision_id must not be empty").into());
    }

    let mut visited = BTreeSet::new();
    visited.insert(decision_id.to_owned());

    let mut older = Vec::new();
    let mut cursor = decision_id.to_owned();
    loop {
        let older_neighbors = supersession_neighbors(graph, &cursor, WalkDirection::Older)?;
        let Some(next) = choose_single_neighbor(&cursor, older_neighbors)? else {
            break;
        };
        if !visited.insert(next.clone()) {
            return Err(query_error(format!("cycle detected on edge {cursor} -> {next}")).into());
        }
        older.push(next.clone());
        cursor = next;
    }

    let mut newer = Vec::new();
    let mut cursor = decision_id.to_owned();
    loop {
        let newer_neighbors = supersession_neighbors(graph, &cursor, WalkDirection::Newer)?;
        let Some(next) = choose_single_neighbor(&cursor, newer_neighbors)? else {
            break;
        };
        if !visited.insert(next.clone()) {
            return Err(query_error(format!("cycle detected on edge {next} -> {cursor}")).into());
        }
        newer.push(next.clone());
        cursor = next;
    }

    older.reverse();
    let input_index = older.len();
    let mut decision_ids = older;
    decision_ids.push(decision_id.to_owned());
    decision_ids.extend(newer);

    Ok(QueryResponse {
        result_count: decision_ids.len(),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: SupersessionChain {
            decision_ids,
            input_index,
        },
    })
}

enum WalkDirection {
    Older,
    Newer,
}

fn supersession_neighbors(
    graph: &impl GraphView,
    decision_id: &str,
    direction: WalkDirection,
) -> Result<Vec<String>> {
    let cypher = match direction {
        WalkDirection::Older => {
            "MATCH (d:`Decision` {id: $id})-[:`SUPERSEDES`]->(other:`Decision`) RETURN other.id AS id ORDER BY other.id;"
        }
        WalkDirection::Newer => {
            "MATCH (other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision` {id: $id}) RETURN other.id AS id ORDER BY other.id;"
        }
    };
    let rows = graph.query(
        cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(required_string(&row, "id")?);
    }
    Ok(ids)
}

fn choose_single_neighbor(current: &str, neighbors: Vec<String>) -> Result<Option<String>> {
    if neighbors.len() <= 1 {
        return Ok(neighbors.into_iter().next());
    }
    Err(query_error(format!(
        "supersession chain branched at {current}: {} candidates",
        neighbors.len()
    ))
    .into())
}
