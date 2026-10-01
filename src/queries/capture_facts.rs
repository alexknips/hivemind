//! How each decision was recorded, as stated facts: its topic keys and the ledger event that
//! recorded it (hivemind-xarm).
//!
//! Decisions that a classifier captured out of one conversation are recorded by one
//! `ingest.batch_classified` event, so they share an `event_origin`. This module only reads that
//! fact and the decision-to-decision links someone recorded; it does not decide that any two
//! decisions are related. Any such inference is Layer 3.

use std::collections::BTreeSet;

use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::same_as::same_as_groups;
use super::shared::{
    node_rows, optional_int, optional_string, optional_string_list, relation_edges,
};

/// What one titled decision states about how it was recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureFact {
    pub decision_id: String,
    pub title: String,
    /// The topic keys the capture carried, as stored (not de-duplicated).
    pub topic_keys: Vec<String>,
    /// The ledger offset of the event that recorded the decision; `None` on a node that carries
    /// none. Equal offsets mean one event recorded both.
    pub event_origin: Option<i64>,
}

/// Every decision that has a title (a node only referenced by a request or blocker has none), in
/// id order. One bulk read, however many decisions the graph holds.
pub fn decision_capture_facts(graph: &impl GraphView) -> Result<Vec<CaptureFact>> {
    Ok(node_rows(graph, NodeKind::Decision)?
        .into_iter()
        .filter_map(|(decision_id, row)| {
            Some(CaptureFact {
                title: optional_string(&row, "title").filter(|title| !title.trim().is_empty())?,
                topic_keys: optional_string_list(&row, "topic_keys"),
                event_origin: optional_int(&row, "event_origin"),
                decision_id,
            })
        })
        .collect())
}

/// The decisions someone has already joined to `decision_id` with a recorded decision-to-decision
/// relation: `SUPERSEDES` and `FOLLOWS_FROM` in either direction, and every record of the same
/// decision (`SAME_AS`, followed transitively). `decision_id` itself is not in the answer.
///
/// A relation two decisions share through a third node (the same evidence, the same question) is
/// not a decision-to-decision relation and is not listed.
pub fn recorded_decision_links(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<BTreeSet<String>> {
    let mut linked = BTreeSet::new();
    for relation in [RelationKind::Supersedes, RelationKind::FollowsFrom] {
        for (from, to) in relation_edges(graph, relation)? {
            if from == decision_id {
                linked.insert(to);
            } else if to == decision_id {
                linked.insert(from);
            }
        }
    }
    for group in same_as_groups(graph)? {
        if group.iter().any(|member| member == decision_id) {
            linked.extend(group);
        }
    }
    linked.remove(decision_id);
    Ok(linked)
}
