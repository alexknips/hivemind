use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;

use crate::error::QueryError;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

const MAX_QUERY_RESULTS: usize = 1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Contested,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HypothesisStatus {
    Open,
    Supported,
    Refuted,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueryResponse<T> {
    pub result_count: usize,
    pub truncated: bool,
    pub latency_ms: u128,
    pub data: T,
}

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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SupersessionChain {
    pub decision_ids: Vec<String>,
    pub input_index: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborhoodRoot {
    pub id: String,
    pub kind: NodeKind,
    pub present: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborNode {
    pub id: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_status: Option<DecisionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hypothesis_status: Option<HypothesisStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborEdge {
    pub from: String,
    pub to: String,
    pub relation: RelationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_origin: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborhoodView {
    pub root: NeighborhoodRoot,
    pub nodes: Vec<NeighborNode>,
    pub edges: Vec<NeighborEdge>,
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
            RelationKind::Assumes,
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

pub fn get_supersession_chain(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<SupersessionChain>> {
    let started = Instant::now();
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NeighborhoodRequest {
    pub relations: Option<Vec<RelationKind>>,
}

impl NeighborhoodRequest {
    pub fn all() -> Self {
        Self { relations: None }
    }

    pub fn with_relations<I: IntoIterator<Item = RelationKind>>(relations: I) -> Self {
        Self {
            relations: Some(relations.into_iter().collect()),
        }
    }

    fn allows(&self, relation: RelationKind) -> bool {
        match &self.relations {
            None => true,
            Some(allowed) => allowed.contains(&relation),
        }
    }
}

const DECISION_HOP1_RELATIONS: [(RelationKind, NodeKind, Direction); 9] = [
    (
        RelationKind::ProposedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    ),
    (
        RelationKind::AcceptedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    ),
    (
        RelationKind::RejectedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    ),
    (
        RelationKind::HasOption,
        NodeKind::Option,
        Direction::Outgoing,
    ),
    (RelationKind::Chose, NodeKind::Option, Direction::Outgoing),
    (
        RelationKind::BasedOn,
        NodeKind::Evidence,
        Direction::Outgoing,
    ),
    (
        RelationKind::Assumes,
        NodeKind::Hypothesis,
        Direction::Outgoing,
    ),
    (
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Outgoing,
    ),
    (
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Incoming,
    ),
];

const HYPOTHESIS_HOP2_RELATIONS: [(RelationKind, NodeKind, Direction); 2] = [
    (
        RelationKind::Supports,
        NodeKind::Evidence,
        Direction::Incoming,
    ),
    (
        RelationKind::Refutes,
        NodeKind::Evidence,
        Direction::Incoming,
    ),
];

pub fn get_decision_neighborhood(
    graph: &impl GraphView,
    decision_id: &str,
    request: &NeighborhoodRequest,
) -> Result<QueryResponse<NeighborhoodView>> {
    let started = Instant::now();
    let decision_id = decision_id.trim();
    if decision_id.is_empty() {
        return Err(query_error("decision_id must not be empty").into());
    }

    let root_present = decision_node_exists(graph, decision_id)?;
    let root = NeighborhoodRoot {
        id: decision_id.to_owned(),
        kind: NodeKind::Decision,
        present: root_present,
    };

    if !root_present {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: NeighborhoodView {
                root,
                nodes: Vec::new(),
                edges: Vec::new(),
            },
        });
    }

    let mut edges: Vec<NeighborEdge> = Vec::new();
    let mut hypothesis_ids: BTreeSet<String> = BTreeSet::new();

    for (relation, other_kind, direction) in DECISION_HOP1_RELATIONS {
        if !request.allows(relation) {
            continue;
        }
        let pairs = neighbor_pairs(
            graph,
            NodeKind::Decision,
            decision_id,
            relation,
            other_kind,
            direction,
        )?;
        for (other_id, event_origin) in pairs {
            let (from, to) = match direction {
                Direction::Outgoing => (decision_id.to_owned(), other_id.clone()),
                Direction::Incoming => (other_id.clone(), decision_id.to_owned()),
            };
            edges.push(NeighborEdge {
                from,
                to,
                relation,
                event_origin,
            });
            if matches!(other_kind, NodeKind::Hypothesis) {
                hypothesis_ids.insert(other_id);
            }
        }
    }

    for hypothesis_id in &hypothesis_ids {
        for (relation, other_kind, direction) in HYPOTHESIS_HOP2_RELATIONS {
            if !request.allows(relation) {
                continue;
            }
            let pairs = neighbor_pairs(
                graph,
                NodeKind::Hypothesis,
                hypothesis_id,
                relation,
                other_kind,
                direction,
            )?;
            for (other_id, event_origin) in pairs {
                let (from, to) = match direction {
                    Direction::Outgoing => (hypothesis_id.clone(), other_id),
                    Direction::Incoming => (other_id, hypothesis_id.clone()),
                };
                edges.push(NeighborEdge {
                    from,
                    to,
                    relation,
                    event_origin,
                });
            }
        }
    }

    edges.sort_by(|a, b| {
        (a.relation, &a.from, &a.to, a.event_origin).cmp(&(
            b.relation,
            &b.from,
            &b.to,
            b.event_origin,
        ))
    });
    edges.dedup();

    let total_edges = edges.len();
    let truncated = total_edges > MAX_QUERY_RESULTS;
    if truncated {
        edges.truncate(MAX_QUERY_RESULTS);
    }

    let mut node_kinds: BTreeMap<String, NodeKind> = BTreeMap::new();
    node_kinds.insert(decision_id.to_owned(), NodeKind::Decision);

    for (relation, other_kind, direction) in DECISION_HOP1_RELATIONS {
        if !request.allows(relation) {
            continue;
        }
        for edge in &edges {
            if edge.relation != relation {
                continue;
            }
            let other_id = match direction {
                Direction::Outgoing => edge.to.clone(),
                Direction::Incoming => edge.from.clone(),
            };
            node_kinds.entry(other_id).or_insert(other_kind);
        }
    }

    for hypothesis_id in &hypothesis_ids {
        for (relation, other_kind, direction) in HYPOTHESIS_HOP2_RELATIONS {
            if !request.allows(relation) {
                continue;
            }
            for edge in &edges {
                if edge.relation != relation {
                    continue;
                }
                let endpoint = match direction {
                    Direction::Outgoing => &edge.from,
                    Direction::Incoming => &edge.to,
                };
                if endpoint != hypothesis_id {
                    continue;
                }
                let other_id = match direction {
                    Direction::Outgoing => edge.to.clone(),
                    Direction::Incoming => edge.from.clone(),
                };
                node_kinds.entry(other_id).or_insert(other_kind);
            }
        }
    }

    let mut nodes: Vec<NeighborNode> = Vec::with_capacity(node_kinds.len());
    for (id, kind) in node_kinds {
        let decision_status = if matches!(kind, NodeKind::Decision) {
            Some(derive_decision_status(graph, &id)?)
        } else {
            None
        };
        let hypothesis_status = if matches!(kind, NodeKind::Hypothesis) {
            Some(derive_hypothesis_status(graph, &id)?)
        } else {
            None
        };
        nodes.push(NeighborNode {
            id,
            kind,
            decision_status,
            hypothesis_status,
        });
    }
    nodes.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));

    let result_count = nodes.len() + edges.len();

    Ok(QueryResponse {
        result_count,
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: NeighborhoodView { root, nodes, edges },
    })
}

fn decision_node_exists(graph: &impl GraphView, decision_id: &str) -> Result<bool> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(!rows.is_empty())
}

fn neighbor_pairs(
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

pub fn derive_decision_status(graph: &impl GraphView, decision_id: &str) -> Result<DecisionStatus> {
    let superseded_count = relation_count(
        graph,
        RelationKind::Supersedes,
        Direction::Incoming,
        NodeKind::Decision,
        decision_id,
    )?;
    if superseded_count > 0 {
        return Ok(DecisionStatus::Superseded);
    }

    let accepted_count = relation_count(
        graph,
        RelationKind::AcceptedBy,
        Direction::Outgoing,
        NodeKind::Decision,
        decision_id,
    )?;
    let rejected_count = relation_count(
        graph,
        RelationKind::RejectedBy,
        Direction::Outgoing,
        NodeKind::Decision,
        decision_id,
    )?;

    match (accepted_count > 0, rejected_count > 0) {
        (true, true) => Ok(DecisionStatus::Contested),
        (true, false) => Ok(DecisionStatus::Accepted),
        (false, true) => Ok(DecisionStatus::Rejected),
        (false, false) => Ok(DecisionStatus::Proposed),
    }
}

pub fn derive_hypothesis_status(
    graph: &impl GraphView,
    hypothesis_id: &str,
) -> Result<HypothesisStatus> {
    let refuted_count = relation_count(
        graph,
        RelationKind::Refutes,
        Direction::Incoming,
        NodeKind::Hypothesis,
        hypothesis_id,
    )?;
    if refuted_count > 0 {
        return Ok(HypothesisStatus::Refuted);
    }

    let supported_count = relation_count(
        graph,
        RelationKind::Supports,
        Direction::Incoming,
        NodeKind::Hypothesis,
        hypothesis_id,
    )?;
    if supported_count > 0 {
        Ok(HypothesisStatus::Supported)
    } else {
        Ok(HypothesisStatus::Open)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Incoming,
    Outgoing,
}

fn relation_count(
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

fn neighbor_ids(
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

fn required_string(row: &GraphRow, key: &str) -> Result<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Ok(value.clone()),
        _ => Err(query_error(format!("row missing string field: {key}")).into()),
    }
}

fn optional_string(row: &GraphRow, key: &str) -> Option<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn optional_int(row: &GraphRow, key: &str) -> Option<i64> {
    match row.get(key) {
        Some(GraphValue::Int(value)) => Some(*value),
        _ => None,
    }
}

fn optional_string_list(row: &GraphRow, key: &str) -> Vec<String> {
    match row.get(key) {
        Some(GraphValue::StringList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn read_count(rows: Vec<GraphRow>, relation_table: &str) -> Result<u64> {
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

fn query_error(error: impl std::fmt::Display) -> QueryError {
    QueryError::Execution(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

    use crate::projector::GraphProperties;

    use super::*;

    #[derive(Debug, Default)]
    struct StatusGraph {
        edges: BTreeSet<(RelationKind, String, String)>,
        mutation_calls: Cell<usize>,
    }

    impl StatusGraph {
        fn with_edges(edges: &[(RelationKind, &str, &str)]) -> Self {
            Self {
                edges: edges
                    .iter()
                    .map(|(kind, from, to)| (*kind, (*from).to_owned(), (*to).to_owned()))
                    .collect(),
                mutation_calls: Cell::new(0),
            }
        }

        fn mutation_calls(&self) -> usize {
            self.mutation_calls.get()
        }
    }

    impl GraphView for StatusGraph {
        fn upsert_node(
            &self,
            _kind: NodeKind,
            _id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            self.mutation_calls.set(self.mutation_calls.get() + 1);
            Ok(())
        }

        fn upsert_edge(
            &self,
            _kind: RelationKind,
            _from_id: &str,
            _to_id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            self.mutation_calls.set(self.mutation_calls.get() + 1);
            Ok(())
        }

        fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
            let id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("id param missing").into()),
            };
            let relation = relation_from_query(cypher)?;
            let incoming = cypher.contains("<-[rel:");
            let count = self
                .edges
                .iter()
                .filter(|(kind, from, to)| {
                    *kind == relation && if incoming { to == id } else { from == id }
                })
                .count();
            let count = i64::try_from(count)
                .map_err(|error| query_error(format!("count overflow: {error}")))?;

            Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(count),
            )])])
        }

        fn wipe(&self) -> Result<()> {
            self.mutation_calls.set(self.mutation_calls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn derives_all_decision_status_cases() -> Result<()> {
        let cases = [
            (
                "proposed",
                StatusGraph::with_edges(&[(RelationKind::ProposedBy, "proposed", "actor:1")]),
                DecisionStatus::Proposed,
            ),
            (
                "accepted",
                StatusGraph::with_edges(&[(RelationKind::AcceptedBy, "accepted", "actor:1")]),
                DecisionStatus::Accepted,
            ),
            (
                "rejected",
                StatusGraph::with_edges(&[(RelationKind::RejectedBy, "rejected", "actor:1")]),
                DecisionStatus::Rejected,
            ),
            (
                "contested",
                StatusGraph::with_edges(&[
                    (RelationKind::AcceptedBy, "contested", "actor:1"),
                    (RelationKind::RejectedBy, "contested", "actor:2"),
                ]),
                DecisionStatus::Contested,
            ),
            (
                "superseded",
                StatusGraph::with_edges(&[
                    (RelationKind::AcceptedBy, "superseded", "actor:1"),
                    (RelationKind::RejectedBy, "superseded", "actor:2"),
                    (RelationKind::Supersedes, "newer", "superseded"),
                ]),
                DecisionStatus::Superseded,
            ),
        ];

        for (decision_id, graph, expected) in cases {
            assert_eq!(derive_decision_status(&graph, decision_id)?, expected);
            assert_eq!(graph.mutation_calls(), 0);
        }

        Ok(())
    }

    #[test]
    fn derives_all_hypothesis_status_cases() -> Result<()> {
        let cases = [
            ("open", StatusGraph::default(), HypothesisStatus::Open),
            (
                "supported",
                StatusGraph::with_edges(&[(RelationKind::Supports, "evidence:1", "supported")]),
                HypothesisStatus::Supported,
            ),
            (
                "refuted",
                StatusGraph::with_edges(&[
                    (RelationKind::Supports, "evidence:1", "refuted"),
                    (RelationKind::Refutes, "evidence:2", "refuted"),
                ]),
                HypothesisStatus::Refuted,
            ),
        ];

        for (hypothesis_id, graph, expected) in cases {
            assert_eq!(derive_hypothesis_status(&graph, hypothesis_id)?, expected);
            assert_eq!(graph.mutation_calls(), 0);
        }

        Ok(())
    }

    fn relation_from_query(cypher: &str) -> Result<RelationKind> {
        for relation in RelationKind::ALL {
            if cypher.contains(&format!("`{}`", relation.table_name())) {
                return Ok(relation);
            }
        }
        Err(query_error(format!("unknown relation in query: {cypher}")).into())
    }

    #[derive(Debug, Default)]
    struct FixtureGraph {
        decisions: BTreeMap<String, (String, String, Vec<String>)>,
        hypotheses: BTreeMap<String, String>,
        edges: BTreeSet<(RelationKind, String, String)>,
    }

    impl FixtureGraph {
        fn sample() -> Self {
            let mut graph = Self::default();
            graph.decisions.insert(
                "d1".to_owned(),
                (
                    "Pick queue".to_owned(),
                    "Need reliability".to_owned(),
                    vec!["infra".to_owned(), "latency".to_owned()],
                ),
            );
            graph.decisions.insert(
                "d2".to_owned(),
                (
                    "Keep sync path".to_owned(),
                    "Prefer simplicity".to_owned(),
                    vec!["infra".to_owned()],
                ),
            );
            graph
                .hypotheses
                .insert("h1".to_owned(), "Queue improves p95".to_owned());

            graph.edges.insert((
                RelationKind::ProposedBy,
                "d1".to_owned(),
                "actor:1".to_owned(),
            ));
            graph.edges.insert((
                RelationKind::AcceptedBy,
                "d1".to_owned(),
                "actor:2".to_owned(),
            ));
            graph
                .edges
                .insert((RelationKind::HasOption, "d1".to_owned(), "o1".to_owned()));
            graph
                .edges
                .insert((RelationKind::HasOption, "d1".to_owned(), "o2".to_owned()));
            graph
                .edges
                .insert((RelationKind::Chose, "d1".to_owned(), "o2".to_owned()));
            graph
                .edges
                .insert((RelationKind::BasedOn, "d1".to_owned(), "e1".to_owned()));
            graph
                .edges
                .insert((RelationKind::Assumes, "d1".to_owned(), "h1".to_owned()));
            graph
                .edges
                .insert((RelationKind::Supports, "e1".to_owned(), "h1".to_owned()));

            graph.edges.insert((
                RelationKind::ProposedBy,
                "d2".to_owned(),
                "actor:3".to_owned(),
            ));
            graph.edges.insert((
                RelationKind::RejectedBy,
                "d2".to_owned(),
                "actor:4".to_owned(),
            ));
            graph
        }
    }

    impl GraphView for FixtureGraph {
        fn upsert_node(
            &self,
            _kind: NodeKind,
            _id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            Ok(())
        }

        fn upsert_edge(
            &self,
            _kind: RelationKind,
            _from_id: &str,
            _to_id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            Ok(())
        }

        fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
            if cypher.contains("RETURN count(rel) AS count;") {
                let relation = relation_from_query(cypher)?;
                let incoming = cypher.contains("<-[rel:");
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let count = self
                    .edges
                    .iter()
                    .filter(|(kind, from, to)| {
                        *kind == relation && if incoming { to == id } else { from == id }
                    })
                    .count();
                return Ok(vec![GraphRow::from([(
                    "count".to_owned(),
                    GraphValue::Int(i64::try_from(count).unwrap_or(0)),
                )])]);
            }

            if cypher.contains("RETURN count(d) AS count;") {
                let topic = match params.get("topic") {
                    Some(GraphValue::String(topic)) => topic,
                    _ => return Err(query_error("missing topic param").into()),
                };
                let count = self
                    .decisions
                    .values()
                    .filter(|(_, _, topics)| topics.iter().any(|value| value == topic))
                    .count();
                return Ok(vec![GraphRow::from([(
                    "count".to_owned(),
                    GraphValue::Int(i64::try_from(count).unwrap_or(0)),
                )])]);
            }

            if cypher.contains("MATCH (d:`Decision` {id: $id}) RETURN d.id AS id") {
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                if let Some((title, rationale, topics)) = self.decisions.get(id) {
                    return Ok(vec![GraphRow::from([
                        ("id".to_owned(), GraphValue::String(id.clone())),
                        ("title".to_owned(), GraphValue::String(title.clone())),
                        (
                            "rationale".to_owned(),
                            GraphValue::String(rationale.clone()),
                        ),
                        (
                            "topic_keys".to_owned(),
                            GraphValue::StringList(topics.clone()),
                        ),
                    ])]);
                }
                return Ok(Vec::new());
            }

            if cypher.contains("WHERE $topic IN d.topic_keys") {
                let topic = match params.get("topic") {
                    Some(GraphValue::String(topic)) => topic,
                    _ => return Err(query_error("missing topic param").into()),
                };
                let mut rows = self
                    .decisions
                    .iter()
                    .filter(|(_, (_, _, topics))| topics.iter().any(|value| value == topic))
                    .map(|(id, (title, rationale, topics))| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            ("title".to_owned(), GraphValue::String(title.clone())),
                            (
                                "rationale".to_owned(),
                                GraphValue::String(rationale.clone()),
                            ),
                            (
                                "topic_keys".to_owned(),
                                GraphValue::StringList(topics.clone()),
                            ),
                        ])
                    })
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    let l = left.get("id");
                    let r = right.get("id");
                    format!("{l:?}").cmp(&format!("{r:?}"))
                });
                return Ok(rows);
            }

            for (relation, alias) in [
                (RelationKind::HasOption, "option_id"),
                (RelationKind::Chose, "option_id"),
                (RelationKind::BasedOn, "evidence_id"),
                (RelationKind::Assumes, "hypothesis_id"),
            ] {
                if cypher.contains(&format!("[:`{}`]", relation.table_name())) {
                    let decision_id = match params.get("id") {
                        Some(GraphValue::String(id)) => id,
                        _ => return Err(query_error("missing id param").into()),
                    };
                    let mut ids = self
                        .edges
                        .iter()
                        .filter(|(kind, from, _)| *kind == relation && from == decision_id)
                        .map(|(_, _, to)| to.clone())
                        .collect::<Vec<_>>();
                    ids.sort();
                    return Ok(ids
                        .into_iter()
                        .map(|value| {
                            GraphRow::from([(alias.to_owned(), GraphValue::String(value))])
                        })
                        .collect());
                }
            }

            if cypher.contains("[r:`") && cypher.contains("AS event_origin") {
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let relation = relation_from_query(cypher)?;
                let incoming = cypher.contains("<-[r:`");
                let mut neighbors: Vec<String> = self
                    .edges
                    .iter()
                    .filter(|(kind, from, to)| {
                        *kind == relation && if incoming { to == id } else { from == id }
                    })
                    .map(
                        |(_, from, to)| {
                            if incoming {
                                from.clone()
                            } else {
                                to.clone()
                            }
                        },
                    )
                    .collect();
                neighbors.sort();
                return Ok(neighbors
                    .into_iter()
                    .map(|other_id| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(other_id)),
                            ("event_origin".to_owned(), GraphValue::Null),
                        ])
                    })
                    .collect());
            }

            if cypher.contains("[:`SUPERSEDES`]->(other:`Decision`)") {
                let decision_id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let mut ids = self
                    .edges
                    .iter()
                    .filter(|(kind, from, _)| {
                        *kind == RelationKind::Supersedes && from == decision_id
                    })
                    .map(|(_, _, to)| to.clone())
                    .collect::<Vec<_>>();
                ids.sort();
                return Ok(ids
                    .into_iter()
                    .map(|id| GraphRow::from([("id".to_owned(), GraphValue::String(id))]))
                    .collect());
            }

            if cypher.contains("(other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision`") {
                let decision_id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let mut ids = self
                    .edges
                    .iter()
                    .filter(|(kind, _, to)| *kind == RelationKind::Supersedes && to == decision_id)
                    .map(|(_, from, _)| from.clone())
                    .collect::<Vec<_>>();
                ids.sort();
                return Ok(ids
                    .into_iter()
                    .map(|id| GraphRow::from([("id".to_owned(), GraphValue::String(id))]))
                    .collect());
            }

            Err(query_error(format!("unsupported query in fixture: {cypher}")).into())
        }

        fn wipe(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn get_decision_returns_neighbors_and_derived_status() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = get_decision(&graph, "d1")?;
        assert_eq!(response.result_count, 1);
        assert!(!response.truncated);
        let decision = response.data.expect("decision exists");
        assert_eq!(decision.id, "d1");
        assert_eq!(decision.status, DecisionStatus::Accepted);
        assert_eq!(decision.chosen_option_id.as_deref(), Some("o2"));
        assert_eq!(decision.option_ids, vec!["o1".to_owned(), "o2".to_owned()]);
        assert_eq!(decision.evidence_ids, vec!["e1".to_owned()]);
        assert_eq!(decision.hypotheses.len(), 1);
        assert_eq!(decision.hypotheses[0].id, "h1");
        assert_eq!(decision.hypotheses[0].status, HypothesisStatus::Supported);
        Ok(())
    }

    #[test]
    fn get_relevant_decisions_filters_by_status() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = get_relevant_decisions(&graph, "infra", Some(DecisionStatus::Rejected))?;
        assert_eq!(response.result_count, 1);
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, "d2");
        assert_eq!(response.data[0].status, DecisionStatus::Rejected);
        Ok(())
    }

    #[test]
    fn get_supersession_chain_walks_both_directions() -> Result<()> {
        let mut graph = FixtureGraph::sample();
        graph
            .edges
            .insert((RelationKind::Supersedes, "d2".to_owned(), "d1".to_owned()));
        graph
            .edges
            .insert((RelationKind::Supersedes, "d3".to_owned(), "d2".to_owned()));
        graph.decisions.insert(
            "d3".to_owned(),
            (
                "Newest".to_owned(),
                "latest".to_owned(),
                vec!["infra".to_owned()],
            ),
        );

        let chain = get_supersession_chain(&graph, "d2")?;
        assert_eq!(
            chain.data.decision_ids,
            vec!["d1".to_owned(), "d2".to_owned(), "d3".to_owned()]
        );
        assert_eq!(chain.data.input_index, 1);
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_returns_full_one_hop() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

        assert!(response.data.root.present);
        assert_eq!(response.data.root.id, "d1");
        assert_eq!(response.data.root.kind, NodeKind::Decision);

        let edge_relations: Vec<RelationKind> = response
            .data
            .edges
            .iter()
            .map(|edge| edge.relation)
            .collect();
        assert!(edge_relations.contains(&RelationKind::ProposedBy));
        assert!(edge_relations.contains(&RelationKind::AcceptedBy));
        assert!(edge_relations.contains(&RelationKind::HasOption));
        assert!(edge_relations.contains(&RelationKind::Chose));
        assert!(edge_relations.contains(&RelationKind::BasedOn));
        assert!(edge_relations.contains(&RelationKind::Assumes));
        // SUPPORTS arrives via 2-hop from the visible hypothesis h1 <- e1
        assert!(edge_relations.contains(&RelationKind::Supports));

        let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
        for expected in ["d1", "actor:1", "actor:2", "o1", "o2", "e1", "h1"] {
            assert!(node_ids.contains(&expected), "missing node {expected}");
        }

        let root_node = response
            .data
            .nodes
            .iter()
            .find(|n| n.id == "d1")
            .expect("root node present");
        assert_eq!(root_node.decision_status, Some(DecisionStatus::Accepted));

        let hypothesis_node = response
            .data
            .nodes
            .iter()
            .find(|n| n.id == "h1")
            .expect("hypothesis node present");
        assert_eq!(
            hypothesis_node.hypothesis_status,
            Some(HypothesisStatus::Supported)
        );

        let mut sorted = response.data.edges.clone();
        sorted.sort_by(|a, b| {
            (a.relation, &a.from, &a.to, a.event_origin).cmp(&(
                b.relation,
                &b.from,
                &b.to,
                b.event_origin,
            ))
        });
        assert_eq!(sorted, response.data.edges, "edges must be deterministic");

        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_filters_by_relation() -> Result<()> {
        let graph = FixtureGraph::sample();
        let request = NeighborhoodRequest::with_relations([RelationKind::ProposedBy]);
        let response = get_decision_neighborhood(&graph, "d1", &request)?;

        for edge in &response.data.edges {
            assert_eq!(edge.relation, RelationKind::ProposedBy);
        }
        let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(node_ids.contains(&"actor:1"));
        assert!(!node_ids.contains(&"o1"), "options filtered out");
        assert!(!node_ids.contains(&"e1"), "evidence filtered out");
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_handles_missing_decision() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response =
            get_decision_neighborhood(&graph, "no-such-decision", &NeighborhoodRequest::all())?;

        assert!(!response.data.root.present);
        assert!(response.data.nodes.is_empty());
        assert!(response.data.edges.is_empty());
        assert_eq!(response.result_count, 0);
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_reports_branched_supersession() -> Result<()> {
        let mut graph = FixtureGraph::sample();
        graph.decisions.insert(
            "branch_a".to_owned(),
            ("A".to_owned(), "rationale".to_owned(), Vec::new()),
        );
        graph.decisions.insert(
            "branch_b".to_owned(),
            ("B".to_owned(), "rationale".to_owned(), Vec::new()),
        );
        graph.edges.insert((
            RelationKind::Supersedes,
            "d1".to_owned(),
            "branch_a".to_owned(),
        ));
        graph.edges.insert((
            RelationKind::Supersedes,
            "d1".to_owned(),
            "branch_b".to_owned(),
        ));

        let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

        let supersedes_targets: Vec<&str> = response
            .data
            .edges
            .iter()
            .filter(|edge| edge.relation == RelationKind::Supersedes && edge.from == "d1")
            .map(|edge| edge.to.as_str())
            .collect();
        assert!(supersedes_targets.contains(&"branch_a"));
        assert!(supersedes_targets.contains(&"branch_b"));
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_includes_refuting_evidence_via_hypothesis() -> Result<()> {
        let mut graph = FixtureGraph::sample();
        graph
            .edges
            .insert((RelationKind::Refutes, "e2".to_owned(), "h1".to_owned()));

        let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

        let refutes_edges: Vec<&NeighborEdge> = response
            .data
            .edges
            .iter()
            .filter(|edge| edge.relation == RelationKind::Refutes)
            .collect();
        assert_eq!(refutes_edges.len(), 1);
        assert_eq!(refutes_edges[0].from, "e2");
        assert_eq!(refutes_edges[0].to, "h1");

        let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(node_ids.contains(&"e2"), "refuting evidence reached via h1");

        let hypothesis_node = response
            .data
            .nodes
            .iter()
            .find(|n| n.id == "h1")
            .expect("hypothesis node present");
        assert_eq!(
            hypothesis_node.hypothesis_status,
            Some(HypothesisStatus::Refuted)
        );
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_rejects_empty_id() {
        let graph = FixtureGraph::sample();
        let error = get_decision_neighborhood(&graph, "   ", &NeighborhoodRequest::all())
            .expect_err("empty id rejected");
        assert!(format!("{error}").contains("decision_id"));
    }

    #[test]
    fn get_supersession_chain_detects_cycle() {
        let mut graph = FixtureGraph::sample();
        graph
            .edges
            .insert((RelationKind::Supersedes, "d2".to_owned(), "d1".to_owned()));
        graph
            .edges
            .insert((RelationKind::Supersedes, "d1".to_owned(), "d2".to_owned()));

        let error = get_supersession_chain(&graph, "d1").expect_err("cycle should fail");
        assert!(format!("{error}").contains("cycle detected"));
    }
}
