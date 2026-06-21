use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use crate::error::ProjectorError;
use crate::Result;

use super::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};

#[derive(Debug, Default)]
pub struct MemoryGraph {
    nodes: Mutex<BTreeMap<(NodeKind, String), GraphProperties>>,
    edges: Mutex<BTreeSet<MemoryEdge>>,
}

impl GraphView for MemoryGraph {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
        let key = (kind, id.to_owned());
        let mut nodes = self.nodes_lock()?;
        let mut existing = nodes.get(&key).cloned().unwrap_or_default();
        existing.extend(properties.clone());
        nodes.insert(key, existing);
        Ok(())
    }

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()> {
        let mut edges = self.edges_lock()?;
        let event_origin = match properties.get("event_origin") {
            Some(GraphValue::Int(value)) => Some(*value),
            _ => None,
        };
        let tenant_id = match properties.get("tenant_id") {
            Some(GraphValue::String(value)) => value.clone(), // ubs:ignore: edge property copy; false positive from impl GraphWriter for.
            _ => String::new(),
        };
        edges.insert(MemoryEdge {
            relation: kind,
            from_id: from_id.to_owned(),
            to_id: to_id.to_owned(),
            _tenant_id: tenant_id,
            _event_origin: event_origin,
        });
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        if cypher.contains("RETURN count(rel) AS count;") {
            let relation = query_relation(cypher)?;
            let id = required_param_string(params, "id")?;
            let incoming = cypher.contains("<-[rel:");
            let edges = self.edges_snapshot()?;
            let count = edges
                .iter()
                .filter(|edge| {
                    if edge.relation != relation {
                        return false;
                    }
                    if incoming {
                        edge.to_id == id
                    } else {
                        edge.from_id == id
                    }
                })
                .count();
            let count = i64::try_from(count)
                .map_err(|error| memory_error(format!("count overflow: {error}")))?;
            return Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(count),
            )])]);
        }

        if cypher.contains("RETURN node.id AS id LIMIT 1;") {
            let kind = query_node_kind(cypher)?;
            let id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if nodes.contains_key(&(kind, id.to_owned())) {
                return Ok(vec![GraphRow::from([(
                    "id".to_owned(),
                    GraphValue::String(id.to_owned()),
                )])]);
            }
            return Ok(Vec::new());
        }

        if cypher.contains("RETURN node.id AS id") {
            let kind = query_node_kind(cypher)?;
            let nodes = self.nodes_snapshot()?;
            let mut rows = nodes
                .iter()
                .filter_map(|((node_kind, id), properties)| {
                    if *node_kind != kind {
                        return None;
                    }
                    let mut row =
                        GraphRow::from([("id".to_owned(), GraphValue::String(id.clone()))]);
                    row.extend(properties.clone());
                    Some(row)
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| row_string(left, "id").cmp(row_string(right, "id")));
            return Ok(rows);
        }

        if cypher.contains("RETURN from.id AS from_id, to.id AS to_id") {
            let relation = query_relation(cypher)?;
            let mut rows = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation)
                .map(|edge| {
                    GraphRow::from([
                        ("from_id".to_owned(), GraphValue::String(edge.from_id)),
                        ("to_id".to_owned(), GraphValue::String(edge.to_id)),
                    ])
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| {
                (row_string(left, "from_id"), row_string(left, "to_id"))
                    .cmp(&(row_string(right, "from_id"), row_string(right, "to_id")))
            });
            return Ok(rows);
        }

        if cypher.contains("RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys LIMIT 1;") {
            let decision_id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if let Some(properties) = nodes.get(&(NodeKind::Decision, decision_id.to_owned())) {
                return Ok(vec![GraphRow::from([
                    ("id".to_owned(), GraphValue::String(decision_id.to_owned())),
                    (
                        "title".to_owned(),
                        graph_property_or_default(properties, "title"),
                    ),
                    (
                        "rationale".to_owned(),
                        graph_property_or_default(properties, "rationale"),
                    ),
                    (
                        "topic_keys".to_owned(),
                        graph_property_or_default(properties, "topic_keys"),
                    ),
                ])]);
            }
            return Ok(Vec::new());
        }

        if cypher
            .contains("MATCH (h:`Hypothesis` {id: $id}) RETURN h.statement AS statement LIMIT 1;")
        {
            let id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if let Some(properties) = nodes.get(&(NodeKind::Hypothesis, id.to_owned())) {
                return Ok(vec![GraphRow::from([(
                    "statement".to_owned(),
                    graph_property_or_default(properties, "statement"),
                )])]);
            }
            return Ok(Vec::new());
        }

        if cypher.contains("MATCH (d:`Decision` {id: $id}) RETURN d.id AS id LIMIT 1;") {
            let decision_id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if nodes.contains_key(&(NodeKind::Decision, decision_id.to_owned())) {
                return Ok(vec![GraphRow::from([(
                    "id".to_owned(),
                    GraphValue::String(decision_id.to_owned()),
                )])]);
            }
            return Ok(Vec::new());
        }

        if cypher.contains("RETURN count(d) AS count;") {
            let topic = required_param_string(params, "topic")?;
            let nodes = self.nodes_snapshot()?;
            let count = nodes
                .iter()
                .filter(|((kind, _), properties)| {
                    *kind == NodeKind::Decision
                        && topic_keys(properties)
                            .iter()
                            .any(|candidate| candidate == topic)
                })
                .count();
            let count = i64::try_from(count)
                .map_err(|error| memory_error(format!("count overflow: {error}")))?;
            return Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(count),
            )])]);
        }

        if cypher.contains("WHERE $topic IN d.topic_keys RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys ORDER BY d.id LIMIT 1000;") {
            let topic = required_param_string(params, "topic")?;
            let nodes = self.nodes_snapshot()?;
            let mut decisions = nodes
                .iter()
                .filter_map(|((kind, id), properties)| {
                    if *kind != NodeKind::Decision
                        || !topic_keys(properties).iter().any(|candidate| candidate == topic)
                    {
                        return None;
                    }
                    Some(GraphRow::from([
                        ("id".to_owned(), GraphValue::String(id.clone())),
                        (
                            "title".to_owned(),
                            graph_property_or_default(properties, "title"),
                        ),
                        (
                            "rationale".to_owned(),
                            graph_property_or_default(properties, "rationale"),
                        ),
                        (
                            "topic_keys".to_owned(),
                            graph_property_or_default(properties, "topic_keys"),
                        ),
                    ]))
                })
                .collect::<Vec<_>>();
            decisions.sort_by(|left, right| row_string(left, "id").cmp(row_string(right, "id")));
            decisions.truncate(1000);
            return Ok(decisions);
        }

        if cypher.contains("RETURN n.id AS") {
            let relation = query_relation(cypher)?;
            let decision_id = required_param_string(params, "id")?;
            let alias = if cypher.contains("AS option_id") {
                "option_id"
            } else if cypher.contains("AS evidence_id") {
                "evidence_id"
            } else if cypher.contains("AS hypothesis_id") {
                "hypothesis_id"
            } else {
                return Err(
                    memory_error(format!("unknown neighbor alias in query: {cypher}")).into(),
                );
            };
            let mut ids = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation && edge.from_id == decision_id)
                .map(|edge| edge.to_id)
                .collect::<Vec<_>>();
            ids.sort();
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([(alias.to_owned(), GraphValue::String(id))]))
                .collect());
        }

        if cypher.contains("RETURN b.id AS id, r.event_origin AS event_origin ORDER BY b.id;") {
            let relation = query_relation(cypher)?;
            let id = required_param_string(params, "id")?;
            let incoming = cypher.contains("<-[r:`");
            let mut ids = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| {
                    edge.relation == relation
                        && if incoming {
                            edge.to_id == id
                        } else {
                            edge.from_id == id
                        }
                })
                .map(|edge| if incoming { edge.from_id } else { edge.to_id })
                .collect::<Vec<_>>();
            ids.sort();
            return Ok(ids
                .into_iter()
                .map(|id| {
                    GraphRow::from([
                        ("id".to_owned(), GraphValue::String(id)),
                        ("event_origin".to_owned(), GraphValue::Null),
                    ])
                })
                .collect());
        }

        if cypher.contains("MATCH (d:`Decision` {id: $id})-[:`SUPERSEDES`]->(other:`Decision`)") {
            let id = required_param_string(params, "id")?;
            let mut older = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == RelationKind::Supersedes && edge.from_id == id)
                .map(|edge| edge.to_id)
                .collect::<Vec<_>>();
            older.sort();
            return Ok(older
                .into_iter()
                .map(|value| GraphRow::from([("id".to_owned(), GraphValue::String(value))]))
                .collect());
        }

        if cypher.contains("MATCH (other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision` {id: $id})") {
            let id = required_param_string(params, "id")?;
            let mut newer = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == RelationKind::Supersedes && edge.to_id == id)
                .map(|edge| edge.from_id)
                .collect::<Vec<_>>();
            newer.sort();
            return Ok(newer
                .into_iter()
                .map(|value| GraphRow::from([("id".to_owned(), GraphValue::String(value))]))
                .collect());
        }

        Err(memory_error(format!("unsupported query: {cypher}")).into())
    }

    fn wipe(&self) -> Result<()> {
        self.nodes_lock()?.clear();
        self.edges_lock()?.clear();
        Ok(())
    }
}

impl MemoryGraph {
    /// Return all nodes and edges as plain tuples, for evaluation use.
    pub fn nodes_and_edges(
        &self,
    ) -> Result<(
        BTreeMap<(NodeKind, String), GraphProperties>,
        Vec<(RelationKind, String, String)>,
    )> {
        let nodes = self.nodes_snapshot()?;
        let edges = self
            .edges_snapshot()?
            .into_iter()
            .map(|e| (e.relation, e.from_id, e.to_id))
            .collect();
        Ok((nodes, edges))
    }

    fn nodes_lock(&self) -> Result<MutexGuard<'_, BTreeMap<(NodeKind, String), GraphProperties>>> {
        self.nodes
            .lock()
            .map_err(|error| memory_error(format!("node lock poisoned: {error}")).into())
    }

    fn edges_lock(&self) -> Result<MutexGuard<'_, BTreeSet<MemoryEdge>>> {
        self.edges
            .lock()
            .map_err(|error| memory_error(format!("edge lock poisoned: {error}")).into())
    }

    fn nodes_snapshot(&self) -> Result<BTreeMap<(NodeKind, String), GraphProperties>> {
        Ok(self.nodes_lock()?.clone())
    }

    fn edges_snapshot(&self) -> Result<BTreeSet<MemoryEdge>> {
        Ok(self.edges_lock()?.clone())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MemoryEdge {
    relation: RelationKind,
    from_id: String,
    to_id: String,
    _tenant_id: String,
    _event_origin: Option<i64>,
}

fn query_relation(cypher: &str) -> Result<RelationKind> {
    for relation in RelationKind::ALL {
        if contains_quoted_identifier(cypher, relation.table_name()) {
            return Ok(relation);
        }
    }
    Err(memory_error(format!("unknown relation in query: {cypher}")).into())
}

fn query_node_kind(cypher: &str) -> Result<NodeKind> {
    for kind in NodeKind::ALL {
        if contains_quoted_identifier(cypher, kind.table_name()) {
            return Ok(kind);
        }
    }
    Err(memory_error(format!("unknown node kind in query: {cypher}")).into())
}

fn required_param_string<'a>(params: &'a GraphParams, key: &str) -> Result<&'a str> {
    match params.get(key) {
        Some(GraphValue::String(value)) => Ok(value),
        _ => Err(memory_error(format!("missing string param: {key}")).into()),
    }
}

fn graph_property_or_default(properties: &GraphProperties, key: &str) -> GraphValue {
    properties.get(key).cloned().unwrap_or(GraphValue::Null)
}

fn topic_keys(properties: &GraphProperties) -> Vec<String> {
    match properties.get("topic_keys") {
        Some(GraphValue::StringList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn row_string<'a>(row: &'a GraphRow, key: &str) -> &'a str {
    match row.get(key) {
        Some(GraphValue::String(value)) => value.as_str(),
        _ => "",
    }
}

fn contains_quoted_identifier(cypher: &str, identifier: &str) -> bool {
    cypher
        .split('`')
        .skip(1)
        .step_by(2)
        .any(|quoted| quoted == identifier)
}

fn memory_error(error: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(error.to_string())
}
