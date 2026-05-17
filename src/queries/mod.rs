use std::collections::BTreeSet;
use std::time::Instant;

use crate::error::QueryError;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

const MAX_QUERY_RESULTS: usize = 1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Contested,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HypothesisStatus {
    Open,
    Supported,
    Refuted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueryResponse<T> {
    pub result_count: usize,
    pub truncated: bool,
    pub latency_ms: u128,
    pub data: T,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HypothesisContext {
    pub id: String,
    pub status: HypothesisStatus,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct SupersessionChain {
    pub decision_ids: Vec<String>,
    pub input_index: usize,
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
        "MATCH (d:`Decision`) WHERE $topic IN d.topic_keys RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys ORDER BY d.id LIMIT $limit;",
        &GraphParams::from([
            ("topic".to_owned(), GraphValue::String(normalized_topic)),
            (
                "limit".to_owned(),
                GraphValue::Int(i64::try_from(MAX_QUERY_RESULTS).unwrap_or(1000)),
            ),
        ]),
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
