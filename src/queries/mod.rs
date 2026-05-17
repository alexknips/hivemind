use crate::error::QueryError;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

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
}
