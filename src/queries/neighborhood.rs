//! One-hop neighborhood queries: returns a decision with its immediate graph context.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;

use crate::projector::arrow::Arrow;
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::arrows::NodeTimes;
use super::brief::{get_decision_brief, resolve_option_label, DecisionBrief};
use super::decision::{get_decision_title, get_evidence_content, get_hypothesis_statement};
use super::project_label::ProjectLabels;
use super::shared::{
    decision_node_exists, decision_project, neighbor_pairs, query_error, Direction,
    MAX_QUERY_RESULTS,
};
use super::status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
use super::QueryResponse;

/// Node labels longer than this many characters are clipped and end in `…`, so an evidence item
/// holding a pasted log cannot bloat a `why` answer.
const NODE_LABEL_MAX_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborhoodRoot {
    pub id: String,
    pub kind: NodeKind,
    pub present: bool,
    /// The root decision's answer to "why": title, rationale, chosen and rejected option
    /// labels, status, who decided, and whether it still holds -- the same `DecisionBrief`
    /// `verify` returns, flattened in beside the id. Absent when the decision is not present.
    /// This is also where the root names its project: the brief's `project` and `project_label`
    /// (see `DecisionView::project`) land on the root beside the id, so the root carries no
    /// project fields of its own to collide with them.
    #[serde(flatten)]
    pub brief: Option<DecisionBrief>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborNode {
    pub id: String,
    pub kind: NodeKind,
    /// Address of the project a decision node is filed under; absent on every other kind and on
    /// a decision that has none (see `DecisionView::project`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// What a person calls that project (see `DecisionView::project_label`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_status: Option<DecisionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hypothesis_status: Option<HypothesisStatus>,
    /// What the node says, so the id need not be read: a decision's title, an option's label, a
    /// hypothesis' statement, an evidence item's content. Absent for actors (the id is the name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One edge of the neighborhood, drawn as an arrow from the newer node to the older node
/// (docs/GRAPH_CONTRACT.md). `from`/`to` are the arrow's ends, not the stored direction: use
/// [`NeighborEdge::stored_source`] / [`NeighborEdge::stored_target`] to ask which side makes
/// the claim.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborEdge {
    /// The newer node (or, for equal ages, the stored source).
    pub from: String,
    /// The older node.
    pub to: String,
    /// What the edge means. Unchanged whichever way the arrow runs.
    pub relation: RelationKind,
    /// The relation read along the arrow: an active phrase such as `based on` or `informs`.
    pub label: &'static str,
    /// True when the arrow runs against the relation's stored direction because the stored
    /// target was recorded after the stored source.
    pub reversed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_origin: Option<i64>,
}

impl NeighborEdge {
    fn from_arrow(arrow: Arrow, event_origin: Option<i64>) -> Self {
        Self {
            from: arrow.from_id,
            to: arrow.to_id,
            relation: arrow.relation,
            label: arrow.label,
            reversed: arrow.reversed,
            event_origin,
        }
    }

    /// The side making the claim (the decision that is `BASED_ON` the evidence), whichever way
    /// the arrow runs.
    pub fn stored_source(&self) -> &str {
        if self.reversed {
            &self.to
        } else {
            &self.from
        }
    }

    /// The side the claim is about.
    pub fn stored_target(&self) -> &str {
        if self.reversed {
            &self.from
        } else {
            &self.to
        }
    }
}

/// An edge as the graph stores it, before it is oriented for display.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StoredEdge {
    relation: RelationKind,
    from: String,
    to: String,
    event_origin: Option<i64>,
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

const DECISION_HOP1_RELATIONS: [(RelationKind, NodeKind, Direction); 11] = [
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
        RelationKind::PremisedOn,
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
    // What this decision rests on (outgoing) and what rests on it (incoming): a premise in the
    // broad sense (hivemind-zdsh.15).
    (
        RelationKind::FollowsFrom,
        NodeKind::Decision,
        Direction::Outgoing,
    ),
    (
        RelationKind::FollowsFrom,
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

/// The one-hop neighborhood of a decision, led by the decision's own answer: `root` carries its
/// title, rationale, chosen/rejected option labels, status and deciders, and every non-actor
/// node carries its label. Pure graph reads; the structure itself comes from
/// [`neighborhood_structure`].
pub fn get_decision_neighborhood(
    graph: &impl GraphView,
    decision_id: &str,
    request: &NeighborhoodRequest,
) -> Result<QueryResponse<NeighborhoodView>> {
    let started = Instant::now();
    let mut response = neighborhood_structure(graph, decision_id, request)?;
    if !response.data.root.present {
        return Ok(response);
    }

    let brief = get_decision_brief(graph, &response.data.root.id)?.data;
    for node in &mut response.data.nodes {
        node.label = match &brief {
            Some(brief) if node.kind == NodeKind::Decision && node.id == brief.decision_id => {
                Some(brief.title.clone()) // ubs:ignore: one clone per neighbourhood, only for the root decision
            }
            _ => node_label(graph, node.kind, &node.id)?,
        };
    }
    response.data.root.brief = brief;
    response.latency_ms = started.elapsed().as_millis();
    Ok(response)
}

fn node_label(graph: &impl GraphView, kind: NodeKind, id: &str) -> Result<Option<String>> {
    let label = match kind {
        NodeKind::Decision => get_decision_title(graph, id)?,
        NodeKind::Option => Some(resolve_option_label(graph, id)?.label),
        NodeKind::Hypothesis => get_hypothesis_statement(graph, id)?,
        NodeKind::Evidence => get_evidence_content(graph, id)?,
        _ => None,
    };
    Ok(label.map(|text| clip_label(&text)))
}

fn clip_label(text: &str) -> String {
    if text.chars().count() <= NODE_LABEL_MAX_CHARS {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(NODE_LABEL_MAX_CHARS - 1).collect();
    clipped.push('…');
    clipped
}

/// The bare one-hop structure: root presence, typed nodes with derived statuses, and edges. No
/// labels and no root brief -- for callers that only walk the graph (`compact_view`).
pub(super) fn neighborhood_structure(
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
    let labels = ProjectLabels::from_graph(graph)?;
    let root = NeighborhoodRoot {
        id: decision_id.to_owned(),
        kind: NodeKind::Decision,
        present: root_present,
        brief: None,
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

    let mut edges: Vec<StoredEdge> = Vec::new();
    let mut hypothesis_ids: BTreeSet<String> = BTreeSet::new();

    for (relation, other_kind, direction) in DECISION_HOP1_RELATIONS {
        if !request.allows(relation) {
            continue;
        }
        if relation == RelationKind::PremisedOn {
            // Direct hypotheses (optionless / legacy path).
            let direct = neighbor_pairs(
                graph,
                NodeKind::Decision,
                decision_id,
                RelationKind::PremisedOnDirect,
                NodeKind::Hypothesis,
                Direction::Outgoing,
            )?;
            for (hypothesis_id, event_origin) in direct {
                edges.push(StoredEdge {
                    from: decision_id.to_owned(), // ubs:ignore:
                    to: hypothesis_id.clone(),    // ubs:ignore:
                    relation: RelationKind::PremisedOn,
                    event_origin,
                });
                hypothesis_ids.insert(hypothesis_id);
            }
            // Option-routed hypotheses (Decision→CHOSE→Option→PREMISED_ON→Hypothesis).
            let chosen_option_ids: Vec<String> = edges
                .iter()
                .filter(|e| e.relation == RelationKind::Chose && e.from == decision_id)
                .map(|e| e.to.clone()) // ubs:ignore:
                .collect();
            for opt_id in chosen_option_ids {
                let opt_pairs = neighbor_pairs(
                    graph,
                    NodeKind::Option,
                    &opt_id,
                    RelationKind::PremisedOn,
                    NodeKind::Hypothesis,
                    Direction::Outgoing,
                )?;
                for (hypothesis_id, event_origin) in opt_pairs {
                    edges.push(StoredEdge {
                        from: decision_id.to_owned(), // ubs:ignore:
                        to: hypothesis_id.clone(),    // ubs:ignore:
                        relation: RelationKind::PremisedOn,
                        event_origin,
                    });
                    hypothesis_ids.insert(hypothesis_id);
                }
            }
        } else {
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
                edges.push(StoredEdge {
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
                edges.push(StoredEdge {
                    from,
                    to,
                    relation,
                    event_origin,
                });
            }
        }
    }

    edges.sort();
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
        let (project, project_label) = if matches!(kind, NodeKind::Decision) {
            named_decision_project(graph, &labels, &id)?
        } else {
            (None, None)
        };
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
            project,
            project_label,
            decision_status,
            hypothesis_status,
            label: None,
        });
    }
    nodes.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));

    let mut times = NodeTimes::default();
    let edges = edges
        .into_iter()
        .map(|edge| {
            // The view shows every premise, direct or through the chosen option, as a
            // decision -> hypothesis `PremisedOn` edge, so that is the pair to orient.
            let orient_as = match edge.relation {
                RelationKind::PremisedOn => RelationKind::PremisedOnDirect,
                relation => relation,
            };
            let mut arrow = times.arrow(graph, orient_as, &edge.from, &edge.to)?;
            arrow.relation = edge.relation;
            Ok(NeighborEdge::from_arrow(arrow, edge.event_origin))
        })
        .collect::<Result<Vec<_>>>()?;

    let result_count = nodes.len() + edges.len();

    Ok(QueryResponse {
        result_count,
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: NeighborhoodView { root, nodes, edges },
    })
}

/// The project address and label of a decision node that is present. A decision that was only
/// referenced, never proposed, has no address; its label says so rather than staying blank.
fn named_decision_project(
    graph: &impl GraphView,
    labels: &ProjectLabels,
    decision_id: &str,
) -> Result<(Option<String>, Option<String>)> {
    let project = decision_project(graph, decision_id)?;
    let label = labels.label_of(project.as_deref());
    Ok((project, Some(label)))
}

#[cfg(test)]
mod tests;
