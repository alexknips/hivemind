use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;

use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{
    decision_node_exists, neighbor_pairs, query_error, Direction, MAX_QUERY_RESULTS,
};
use super::status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
use super::QueryResponse;

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
