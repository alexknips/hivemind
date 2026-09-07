//! In-memory graph projector: builds and caches the decision graph from replayed ledger events.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use crate::error::ProjectorError;
use crate::Result;

use super::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};

type NodesAndEdges = (
    BTreeMap<(NodeKind, String), GraphProperties>,
    Vec<(RelationKind, String, String)>,
);

#[derive(Debug, Default)]
pub struct MemoryGraph {
    nodes: Mutex<BTreeMap<(NodeKind, String), GraphProperties>>,
    edges: Mutex<BTreeSet<MemoryEdge>>,
}

impl Clone for MemoryGraph {
    fn clone(&self) -> Self {
        MemoryGraph {
            nodes: Mutex::new(self.nodes_snapshot().unwrap_or_default()),
            edges: Mutex::new(self.edges_snapshot().unwrap_or_default()),
        }
    }
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

        if cypher.contains("UNION") && cypher.contains("from_id") && cypher.contains("to_id") {
            let edges = self.edges_snapshot()?;
            let mut pairs: std::collections::BTreeSet<(String, String)> =
                std::collections::BTreeSet::new();
            for edge in &edges {
                if edge.relation == RelationKind::Chose {
                    let opt_id = &edge.to_id;
                    let decision_id = &edge.from_id;
                    for hop2 in &edges {
                        if hop2.relation == RelationKind::PremisedOn && hop2.from_id == *opt_id {
                            let pair = (decision_id.clone(), hop2.to_id.clone()); // ubs:ignore:
                            pairs.insert(pair);
                        }
                    }
                }
                if edge.relation == RelationKind::PremisedOnDirect {
                    pairs.insert((edge.from_id.clone(), edge.to_id.clone())); // ubs:ignore:
                }
            }
            let mut rows: Vec<GraphRow> = pairs
                .into_iter()
                .map(|(from_id, to_id)| {
                    GraphRow::from([
                        ("from_id".to_owned(), GraphValue::String(from_id)),
                        ("to_id".to_owned(), GraphValue::String(to_id)),
                    ])
                })
                .collect();
            rows.sort_by(|left, right| {
                (row_string(left, "from_id"), row_string(left, "to_id")) // ubs:ignore:
                    .cmp(&(row_string(right, "from_id"), row_string(right, "to_id")))
            });
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

        if cypher.contains("MATCH (o:`Option` {id: $id}) RETURN o.label AS label LIMIT 1;") {
            let id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if let Some(properties) = nodes.get(&(NodeKind::Option, id.to_owned())) {
                return Ok(vec![GraphRow::from([(
                    "label".to_owned(),
                    graph_property_or_default(properties, "label"),
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

        // Checked before the broader "UNION ... hypothesis_id ... $id" branch below: this query
        // (outcome.rs's query_refuted_premises) also matches that broader condition's substrings
        // but needs the REFUTES filter applied, so it must win the match first.
        if cypher.contains("REFUTES") && cypher.contains("hypothesis_id") {
            let decision_id = required_param_string(params, "id")?;
            let edges = self.edges_snapshot()?;
            let refuted_hypotheses: BTreeSet<String> = edges
                .iter()
                .filter(|edge| edge.relation == RelationKind::Refutes)
                .map(|edge| edge.to_id.clone())
                .collect();
            let mut ids: BTreeSet<String> = BTreeSet::new();
            let chosen_options: Vec<String> = edges
                .iter()
                .filter(|edge| edge.relation == RelationKind::Chose && edge.from_id == decision_id)
                .map(|edge| edge.to_id.clone())
                .collect();
            ids.extend(chosen_options.iter().flat_map(|opt_id| {
                edges
                    .iter()
                    .filter(|edge| {
                        edge.relation == RelationKind::PremisedOn
                            && edge.from_id == *opt_id
                            && refuted_hypotheses.contains(&edge.to_id)
                    })
                    .map(|edge| edge.to_id.clone())
            }));
            ids.extend(
                edges
                    .iter()
                    .filter(|edge| {
                        edge.relation == RelationKind::PremisedOnDirect
                            && edge.from_id == decision_id
                            && refuted_hypotheses.contains(&edge.to_id)
                    })
                    .map(|edge| edge.to_id.clone()),
            );
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([("hypothesis_id".to_owned(), GraphValue::String(id))]))
                .collect());
        }

        if cypher.contains("UNION") && cypher.contains("hypothesis_id") && cypher.contains("$id") {
            let decision_id = required_param_string(params, "id")?;
            let edges = self.edges_snapshot()?;
            let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let chosen_options: Vec<String> = edges
                .iter()
                .filter(|e| e.relation == RelationKind::Chose && e.from_id == decision_id)
                .map(|e| e.to_id.clone()) // ubs:ignore:
                .collect();
            for opt_id in &chosen_options {
                ids.extend(
                    edges
                        .iter()
                        .filter(|e| e.relation == RelationKind::PremisedOn && e.from_id == *opt_id)
                        .map(|e| e.to_id.clone()), // ubs:ignore:
                );
            }
            ids.extend(
                edges
                    .iter()
                    .filter(|e| {
                        e.relation == RelationKind::PremisedOnDirect && e.from_id == decision_id
                    })
                    .map(|e| e.to_id.clone()), // ubs:ignore:
            );
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([("hypothesis_id".to_owned(), GraphValue::String(id))])) // ubs:ignore:
                .collect());
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
            let ids: BTreeSet<String> = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation && edge.from_id == decision_id)
                .map(|edge| edge.to_id)
                .collect();
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

        // ---------------------------------------------------------------------------
        // Handlers below close a pre-existing gap this bead's queries surfaced:
        // context.rs (get_decision_context) and outcome.rs (get_decision_outcome) never had
        // MemoryGraph support for their query shapes — every existing test for those two
        // functions used a hand-rolled fixture, never the real in-memory backend, so both were
        // silently broken for the default graph backend (and for their MCP tools) before now.
        // ---------------------------------------------------------------------------

        // query_hypothesis_count (context.rs): same premised-on-hypothesis computation as the
        // "hypothesis_id" UNION handler above, under the "hid" alias it actually uses.
        if cypher.contains("UNION") && cypher.contains(" AS hid") {
            let decision_id = required_param_string(params, "id")?;
            let edges = self.edges_snapshot()?;
            let mut ids: BTreeSet<String> = BTreeSet::new();
            let chosen_options: Vec<String> = edges
                .iter()
                .filter(|edge| edge.relation == RelationKind::Chose && edge.from_id == decision_id)
                .map(|edge| edge.to_id.clone())
                .collect();
            ids.extend(chosen_options.iter().flat_map(|opt_id| {
                edges
                    .iter()
                    .filter(|edge| {
                        edge.relation == RelationKind::PremisedOn && edge.from_id == *opt_id
                    })
                    .map(|edge| edge.to_id.clone())
            }));
            ids.extend(
                edges
                    .iter()
                    .filter(|edge| {
                        edge.relation == RelationKind::PremisedOnDirect
                            && edge.from_id == decision_id
                    })
                    .map(|edge| edge.to_id.clone()),
            );
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([("hid".to_owned(), GraphValue::String(id))]))
                .collect());
        }

        // get_decision_context / get_decision_outcome single-decision lookups: any remaining
        // "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, ..." shape returns the id plus
        // every stored property, same as the bulk `node.id AS id` handler above — the caller
        // reads only the specific keys it asked for.
        if cypher.contains("MATCH (d:`Decision` {id: $id})") && cypher.contains("RETURN d.id AS id")
        {
            let decision_id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            if let Some(properties) = nodes.get(&(NodeKind::Decision, decision_id.to_owned())) {
                let mut row =
                    GraphRow::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]);
                row.extend(properties.clone());
                return Ok(vec![row]);
            }
            return Ok(Vec::new());
        }

        // get_decision_context_candidates / get_decision_quality_candidates bulk lookups:
        // same "return every stored property" treatment as the single-decision case above,
        // scanning all Decision nodes with an optional `$since` event_origin floor.
        if cypher.contains("MATCH (d:`Decision`)") && cypher.contains("RETURN d.id AS id") {
            let since = match params.get("since") {
                Some(GraphValue::Int(value)) => Some(*value),
                _ => None,
            };
            let nodes = self.nodes_snapshot()?;
            let mut rows: Vec<GraphRow> = nodes
                .iter()
                .filter_map(|((kind, id), properties)| {
                    if *kind != NodeKind::Decision {
                        return None;
                    }
                    if let Some(since) = since {
                        let origin = match properties.get("event_origin") {
                            Some(GraphValue::Int(value)) => *value,
                            _ => 0,
                        };
                        if origin < since {
                            return None;
                        }
                    }
                    let mut row =
                        GraphRow::from([("id".to_owned(), GraphValue::String(id.clone()))]);
                    row.extend(properties.clone());
                    Some(row)
                })
                .collect();
            rows.sort_by(|left, right| {
                let origin_of = |row: &GraphRow| match row.get("event_origin") {
                    Some(GraphValue::Int(value)) => *value,
                    _ => 0,
                };
                (origin_of(left), row_string(left, "id"))
                    .cmp(&(origin_of(right), row_string(right, "id")))
            });
            return Ok(rows);
        }

        // context.rs's query_proposer / query_actor_ids_by_edge: decision -[relation]-> Actor,
        // returning the actor id and its "kind" (human/agent/unknown, stamped by upsert_actor).
        if cypher.contains("RETURN a.id AS actor_id, a.kind AS kind") {
            let relation = query_relation(cypher)?;
            let decision_id = required_param_string(params, "id")?;
            let nodes = self.nodes_snapshot()?;
            let mut rows: Vec<GraphRow> = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation && edge.from_id == decision_id)
                .map(|edge| {
                    let kind = nodes
                        .get(&(NodeKind::Actor, edge.to_id.clone()))
                        .and_then(|properties| properties.get("kind").cloned())
                        .unwrap_or(GraphValue::Null);
                    GraphRow::from([
                        ("actor_id".to_owned(), GraphValue::String(edge.to_id)),
                        ("kind".to_owned(), kind),
                    ])
                })
                .collect();
            rows.sort_by(|left, right| {
                row_string(left, "actor_id").cmp(row_string(right, "actor_id"))
            });
            if cypher.contains("LIMIT 1") {
                rows.truncate(1);
            }
            return Ok(rows);
        }

        // context.rs's query_actor_edge_count / query_edge_count and outcome.rs's
        // query_has_options / query_has_evidence: all four are "count decision's outgoing
        // edges of one relation kind", differing only in relation and (unused here) target kind.
        if cypher.contains("RETURN count(*) AS cnt") {
            let relation = query_relation(cypher)?;
            let decision_id = required_param_string(params, "id")?;
            let count = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| edge.relation == relation && edge.from_id == decision_id)
                .count();
            let count = i64::try_from(count)
                .map_err(|error| memory_error(format!("count overflow: {error}")))?;
            return Ok(vec![GraphRow::from([(
                "cnt".to_owned(),
                GraphValue::Int(count),
            )])]);
        }

        // outcome.rs's query_contested: Cypher COUNT{} subquery counting ACCEPTED_BY/REJECTED_BY
        // edges in one call.
        if cypher.contains("AS accepted_count") {
            let decision_id = required_param_string(params, "id")?;
            let edges = self.edges_snapshot()?;
            let count_of = |relation: RelationKind| {
                edges
                    .iter()
                    .filter(|edge| edge.relation == relation && edge.from_id == decision_id)
                    .count()
            };
            let accepted = i64::try_from(count_of(RelationKind::AcceptedBy))
                .map_err(|error| memory_error(format!("count overflow: {error}")))?;
            let rejected = i64::try_from(count_of(RelationKind::RejectedBy))
                .map_err(|error| memory_error(format!("count overflow: {error}")))?;
            return Ok(vec![GraphRow::from([
                ("accepted_count".to_owned(), GraphValue::Int(accepted)),
                ("rejected_count".to_owned(), GraphValue::Int(rejected)),
            ])]);
        }

        // outcome.rs's query_superseder: the newest SUPERSEDES edge pointing at this decision.
        if cypher.contains("RETURN newer.id AS superseder_id, r.event_origin AS edge_origin") {
            let decision_id = required_param_string(params, "id")?;
            let mut matches: Vec<(String, Option<i64>)> = self
                .edges_snapshot()?
                .into_iter()
                .filter(|edge| {
                    edge.relation == RelationKind::Supersedes && edge.to_id == decision_id
                })
                .map(|edge| (edge.from_id, edge._event_origin))
                .collect();
            matches.sort_by_key(|(_, edge_origin)| std::cmp::Reverse(*edge_origin));
            return Ok(matches
                .into_iter()
                .take(1)
                .map(|(superseder_id, edge_origin)| {
                    GraphRow::from([
                        (
                            "superseder_id".to_owned(),
                            GraphValue::String(superseder_id),
                        ),
                        (
                            "edge_origin".to_owned(),
                            edge_origin.map_or(GraphValue::Null, GraphValue::Int),
                        ),
                    ])
                })
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
    pub fn nodes_and_edges(&self) -> Result<NodesAndEdges> {
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
