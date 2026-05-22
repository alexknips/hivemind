use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::error::QueryError;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

pub(crate) const MAX_QUERY_RESULTS: usize = 1000;
pub(crate) const DEFAULT_SEARCH_LIMIT: usize = 25;

pub(crate) fn decision_node_exists(graph: &impl GraphView, decision_id: &str) -> Result<bool> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(!rows.is_empty())
}

pub(crate) fn normalized_query(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(crate) fn query_terms(query: Option<&str>) -> Vec<String> {
    query.map_or_else(Vec::new, |query| {
        query
            .split_whitespace()
            .map(|term| term.to_ascii_lowercase())
            .collect()
    })
}

pub(crate) fn normalized_filter_values(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn normalized_statuses<T: Copy + Ord>(values: &[T]) -> Vec<T> {
    values
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn normalized_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_SEARCH_LIMIT
    } else {
        limit.min(MAX_QUERY_RESULTS)
    }
}

pub(crate) fn parse_cursor(cursor: Option<&str>) -> Result<usize> {
    match cursor {
        None => Ok(0),
        Some(cursor) => cursor.parse::<usize>().map_err(|error| {
            query_error(format!("cursor must be a non-negative offset: {error}")).into()
        }),
    }
}

pub(crate) fn node_rows(
    graph: &impl GraphView,
    kind: NodeKind,
) -> Result<BTreeMap<String, GraphRow>> {
    let table = kind.table_name();
    let cypher = match kind {
        NodeKind::Decision => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.title AS title, node.rationale AS rationale, node.topic_keys AS topic_keys, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::DecisionRequest => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.reason AS reason, node.priority AS priority, node.required_owner_id AS required_owner_id, node.authority_class AS authority_class, node.requested_by AS requested_by, node.client_request_id AS client_request_id, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Actor => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Evidence => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.content AS content, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Option => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.label AS label, node.description AS description, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Hypothesis => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.statement AS statement, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Blocker => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.blocked_actor_id AS blocked_actor_id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.blocked_ref AS blocked_ref, node.blocked_ref_type AS blocked_ref_type, node.reason AS reason, node.priority AS priority, node.last_progress_at AS last_progress_at, node.required_owner_id AS required_owner_id, node.reported_at AS reported_at, node.reported_event_origin AS reported_event_origin, node.resolved_at AS resolved_at, node.resolution_event_id AS resolution_event_id, node.resolution_reason AS resolution_reason, node.resolved_event_origin AS resolved_event_origin, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Notification => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.blocker_id AS blocker_id, node.recipient_actor_id AS recipient_actor_id, node.channel AS channel, node.threshold_rule AS threshold_rule, node.source_event_ids AS source_event_ids, node.dedupe_key AS dedupe_key, node.sent_at AS sent_at, node.ack_at AS ack_at, node.snooze_until AS snooze_until, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
    };

    let mut rows_by_id = BTreeMap::new();
    for mut row in graph.query(&cypher, &GraphParams::new())? {
        let id = required_string(&row, "id")?;
        row.insert("id".to_owned(), GraphValue::String(id.clone()));
        rows_by_id.insert(id, row);
    }
    Ok(rows_by_id)
}

pub(crate) fn relation_edges_by_kind(
    graph: &impl GraphView,
) -> Result<BTreeMap<RelationKind, Vec<(String, String)>>> {
    let mut edges = BTreeMap::new();
    for relation in RelationKind::ALL {
        edges.insert(relation, relation_edges(graph, relation)?);
    }
    Ok(edges)
}

pub(crate) fn relation_edges(
    graph: &impl GraphView,
    relation: RelationKind,
) -> Result<Vec<(String, String)>> {
    let (from_kind, to_kind) = relation.endpoints();
    let from_table = from_kind.table_name();
    let to_table = to_kind.table_name();
    let relation_table = relation.table_name();
    let rows = graph.query(
        &format!(
            "MATCH (from:`{from_table}`)-[:`{relation_table}`]->(to:`{to_table}`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;"
        ),
        &GraphParams::new(),
    )?;

    let mut edges = Vec::with_capacity(rows.len());
    for row in rows {
        edges.push((
            required_string(&row, "from_id")?,
            required_string(&row, "to_id")?,
        ));
    }
    edges.sort();
    Ok(edges)
}

pub(crate) fn relation_targets(
    edges: &BTreeMap<RelationKind, Vec<(String, String)>>,
    relations: &[RelationKind],
    from_id: &str,
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for relation in relations {
        if let Some(relation_edges) = edges.get(relation) {
            ids.extend(
                relation_edges
                    .iter()
                    .filter(|(from, _)| from == from_id)
                    .map(|(_, to)| to.clone()),
            );
        }
    }
    ids.into_iter().collect()
}

pub(crate) fn relation_sources(
    edges: &BTreeMap<RelationKind, Vec<(String, String)>>,
    relation: RelationKind,
    to_id: &str,
) -> Vec<String> {
    edges
        .get(&relation)
        .into_iter()
        .flat_map(|relation_edges| relation_edges.iter())
        .filter(|(_, to)| to == to_id)
        .map(|(from, _)| from.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn neighbor_pairs(
    graph: &impl GraphView,
    root_kind: NodeKind,
    root_id: &str,
    relation: RelationKind,
    other_kind: NodeKind,
    direction: Direction,
) -> Result<Vec<(String, Option<i64>)>> {
    let root_table = root_kind.table_name();
    let relation_table = relation.table_name();
    let other_table = other_kind.table_name();
    let cypher = match direction {
        Direction::Outgoing => format!(
            "MATCH (a:`{root_table}` {{id: $id}})-[r:`{relation_table}`]->(b:`{other_table}`) RETURN b.id AS id, r.event_origin AS event_origin ORDER BY b.id;"
        ),
        Direction::Incoming => format!(
            "MATCH (a:`{root_table}` {{id: $id}})<-[r:`{relation_table}`]-(b:`{other_table}`) RETURN b.id AS id, r.event_origin AS event_origin ORDER BY b.id;"
        ),
    };
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(root_id.to_owned()))]),
    )?;

    let mut pairs = Vec::with_capacity(rows.len());
    for row in rows {
        let id = required_string(&row, "id")?;
        let event_origin = optional_int(&row, "event_origin");
        pairs.push((id, event_origin));
    }
    Ok(pairs)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    Incoming,
    Outgoing,
}

pub(crate) fn relation_count(
    graph: &impl GraphView,
    relation: RelationKind,
    direction: Direction,
    node_kind: NodeKind,
    node_id: &str,
) -> Result<u64> {
    let relation_table = relation.table_name();
    let node_table = node_kind.table_name();
    let cypher = match direction {
        Direction::Incoming => format!(
            "MATCH (node:`{node_table}` {{id: $id}})<-[rel:`{relation_table}`]-() RETURN count(rel) AS count;"
        ),
        Direction::Outgoing => format!(
            "MATCH (node:`{node_table}` {{id: $id}})-[rel:`{relation_table}`]->() RETURN count(rel) AS count;"
        ),
    };
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(node_id.to_owned()))]),
    )?;
    read_count(rows, relation_table)
}

pub(crate) fn neighbor_ids(
    graph: &impl GraphView,
    decision_id: &str,
    relation: RelationKind,
    to_kind: NodeKind,
    alias: &str,
) -> Result<Vec<String>> {
    let relation_table = relation.table_name();
    let to_table = to_kind.table_name();
    let cypher = format!(
        "MATCH (d:`Decision` {{id: $id}})-[:`{relation_table}`]->(n:`{to_table}`) RETURN n.id AS {alias} ORDER BY n.id;"
    );
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(required_string(&row, alias)?);
    }
    Ok(ids)
}

pub(crate) fn required_string(row: &GraphRow, key: &str) -> Result<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Ok(value.clone()),
        _ => Err(query_error(format!("row missing string field: {key}")).into()),
    }
}

pub(crate) fn optional_string(row: &GraphRow, key: &str) -> Option<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

pub(crate) fn optional_int(row: &GraphRow, key: &str) -> Option<i64> {
    match row.get(key) {
        Some(GraphValue::Int(value)) => Some(*value),
        _ => None,
    }
}

pub(crate) fn required_datetime(row: &GraphRow, key: &str) -> Result<DateTime<Utc>> {
    optional_datetime(row, key)?
        .ok_or_else(|| query_error(format!("row missing timestamp field: {key}")).into())
}

pub(crate) fn optional_datetime(row: &GraphRow, key: &str) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = optional_string(row, key) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(&value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|error| query_error(format!("invalid timestamp in {key}: {error}")).into())
}

pub(crate) fn optional_string_list(row: &GraphRow, key: &str) -> Vec<String> {
    match row.get(key) {
        Some(GraphValue::StringList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

pub(crate) fn read_count(rows: Vec<GraphRow>, relation_table: &str) -> Result<u64> {
    let value = rows
        .first()
        .and_then(|row| row.get("count"))
        .ok_or_else(|| query_error(format!("{relation_table} count query returned no count")))?;

    match value {
        GraphValue::Int(value) if *value >= 0 => u64::try_from(*value).map_err(|error| {
            query_error(format!("{relation_table} count was invalid: {error}")).into()
        }),
        GraphValue::Int(value) => {
            Err(query_error(format!("{relation_table} count was negative: {value}")).into())
        }
        other => Err(query_error(format!(
            "{relation_table} count had unexpected value: {other:?}"
        ))
        .into()),
    }
}

pub(crate) fn query_error(error: impl std::fmt::Display) -> QueryError {
    QueryError::Execution(error.to_string())
}

pub(crate) fn query_timer_start() -> Instant {
    // ubs:ignore: Instant measures query latency only; it does not generate secrets.
    Instant::now()
}
