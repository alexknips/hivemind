//! Decision status derivation: computes proposed/accepted/contested/superseded from graph edges.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::projector::{actor_kind, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{relation_count, relation_edges, Direction};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

/// The one status rule. A decision anyone has superseded is `superseded` whatever else was said
/// about it; otherwise accepted and rejected positions together make it `contested`, never one
/// side winning silently. Shared by [`derive_decision_status`] (one decision) and
/// [`DecisionStandings`] (every decision at once) so the two cannot drift.
fn status_from_positions(superseded: bool, accepted: bool, rejected: bool) -> DecisionStatus {
    if superseded {
        return DecisionStatus::Superseded;
    }
    match (accepted, rejected) {
        (true, true) => DecisionStatus::Contested,
        (true, false) => DecisionStatus::Accepted,
        (false, true) => DecisionStatus::Rejected,
        (false, false) => DecisionStatus::Proposed,
    }
}

pub fn derive_decision_status(graph: &impl GraphView, decision_id: &str) -> Result<DecisionStatus> {
    let count = |relation, direction| {
        relation_count(graph, relation, direction, NodeKind::Decision, decision_id)
    };
    Ok(status_from_positions(
        count(RelationKind::Supersedes, Direction::Incoming)? > 0,
        count(RelationKind::AcceptedBy, Direction::Outgoing)? > 0,
        count(RelationKind::RejectedBy, Direction::Outgoing)? > 0,
    ))
}

/// An actor who accepted a decision, with whether they are a human or an agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Decider {
    pub id: String,
    /// `human`, `agent` or `unknown`, read from the actor-id prefix (`human:` / `agent:`): the
    /// same rule that sets `kind` on the Actor node.
    pub kind: &'static str,
}

/// Every decision's status and deciders, read in three bulk edge scans (`SUPERSEDES`,
/// `ACCEPTED_BY`, `REJECTED_BY`) instead of three lookups per decision. For readers that show
/// the whole graph. A decision no edge names is `proposed` with no decider.
#[derive(Debug, Default)]
pub struct DecisionStandings {
    /// Each superseded decision with the decisions that superseded it, sorted by id.
    superseded_by: BTreeMap<String, Vec<String>>,
    accepted_by: BTreeMap<String, Vec<String>>,
    rejected: BTreeSet<String>,
}

impl DecisionStandings {
    pub fn load(graph: &impl GraphView) -> Result<Self> {
        let mut standings = Self::default();
        // `SUPERSEDES` runs newer decision -> older decision, so the target is the superseded one.
        // Edges arrive sorted by (newer, older), so each decision's superseders are sorted by id.
        // The same supersession asserted twice is one.
        for (newer, superseded) in relation_edges(graph, RelationKind::Supersedes)? {
            let superseders = standings.superseded_by.entry(superseded).or_default();
            if superseders.last() != Some(&newer) {
                superseders.push(newer);
            }
        }
        // Edges arrive sorted by (decision, actor), so each decision's actors are sorted by id.
        for (decision_id, actor_id) in relation_edges(graph, RelationKind::AcceptedBy)? {
            standings
                .accepted_by
                .entry(decision_id)
                .or_default()
                .push(actor_id);
        }
        for (decision_id, _) in relation_edges(graph, RelationKind::RejectedBy)? {
            standings.rejected.insert(decision_id);
        }
        Ok(standings)
    }

    /// The status [`derive_decision_status`] gives this decision.
    pub fn status_of(&self, decision_id: &str) -> DecisionStatus {
        status_from_positions(
            self.superseded_by.contains_key(decision_id),
            self.accepted_by.contains_key(decision_id),
            self.rejected.contains(decision_id),
        )
    }

    /// The decisions that superseded this one, sorted by id; empty when nobody has. More than one
    /// means concurrent supersessions, which both stand.
    pub fn superseders_of(&self, decision_id: &str) -> &[String] {
        self.superseded_by
            .get(decision_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Who accepted this decision, sorted by actor id; empty when nobody has.
    pub fn deciders_of(&self, decision_id: &str) -> Vec<Decider> {
        self.accepted_by
            .get(decision_id)
            .into_iter()
            .flatten()
            .map(|actor_id| Decider {
                id: actor_id.clone(), // ubs:ignore: the answer owns its ids; one clone per decider
                kind: actor_kind(actor_id),
            })
            .collect()
    }
}

/// The one hypothesis status rule: evidence that refutes it beats evidence that supports it, and
/// with neither it is open. Shared by [`derive_hypothesis_status`] (one hypothesis) and the bulk
/// grounding read so the two cannot drift.
pub(super) fn hypothesis_status_from_evidence(refuted: bool, supported: bool) -> HypothesisStatus {
    if refuted {
        HypothesisStatus::Refuted
    } else if supported {
        HypothesisStatus::Supported
    } else {
        HypothesisStatus::Open
    }
}

pub fn derive_hypothesis_status(
    graph: &impl GraphView,
    hypothesis_id: &str,
) -> Result<HypothesisStatus> {
    let refuted = relation_count(
        graph,
        RelationKind::Refutes,
        Direction::Incoming,
        NodeKind::Hypothesis,
        hypothesis_id,
    )? > 0;
    // Only a hypothesis nothing refutes needs the second read.
    let supported = !refuted
        && relation_count(
            graph,
            RelationKind::Supports,
            Direction::Incoming,
            NodeKind::Hypothesis,
            hypothesis_id,
        )? > 0;
    Ok(hypothesis_status_from_evidence(refuted, supported))
}
