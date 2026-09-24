//! Oriented edges: the read side of `projector::arrow`. Every arrow a reader is shown runs from
//! the newer node to the older node; the graph itself keeps each relation in its stored,
//! semantic direction. See `docs/GRAPH_CONTRACT.md`.

use std::collections::{BTreeMap, BTreeSet};

use crate::projector::arrow::{orient, Arrow, NodeTime};
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{node_row, node_rows, optional_int, relation_edges};

/// Recorded times of nodes (the `event_origin` that created each), fetched on demand and
/// remembered for the rest of one read.
#[derive(Default)]
pub(crate) struct NodeTimes {
    times: BTreeMap<NodeKind, BTreeMap<String, NodeTime>>,
    /// Kinds whose every node was loaded in one query, so a miss is a real absence.
    loaded: BTreeSet<NodeKind>,
}

impl NodeTimes {
    /// Loads every node of `kind` in one query. For a read that walks a whole relation.
    fn load_kind(&mut self, graph: &impl GraphView, kind: NodeKind) -> Result<()> {
        if !self.loaded.insert(kind) {
            return Ok(());
        }
        let times = self.times.entry(kind).or_default();
        for (id, row) in node_rows(graph, kind)? {
            times.insert(id, optional_int(&row, "event_origin"));
        }
        Ok(())
    }

    /// One node's recorded time; `None` when the node is missing or carries none.
    fn time_of(&mut self, graph: &impl GraphView, kind: NodeKind, id: &str) -> Result<NodeTime> {
        if let Some(time) = self.times.get(&kind).and_then(|times| times.get(id)) {
            return Ok(*time);
        }
        if self.loaded.contains(&kind) {
            return Ok(None);
        }
        let time = node_row(graph, kind, id)?.and_then(|row| optional_int(&row, "event_origin"));
        self.times
            .entry(kind)
            .or_default()
            .insert(id.to_owned(), time);
        Ok(time)
    }

    /// Orients one stored edge `stored_from -[relation]-> stored_to` newer → older.
    pub(crate) fn arrow(
        &mut self,
        graph: &impl GraphView,
        relation: RelationKind,
        stored_from: &str,
        stored_to: &str,
    ) -> Result<Arrow> {
        // A relation that can never reverse (its target is an actor) needs no lookups.
        if relation.arrow_labels().reversed.is_none() {
            return Ok(orient(relation, stored_from, None, stored_to, None));
        }
        let (from_kind, to_kind) = relation.endpoints();
        let from_time = self.time_of(graph, from_kind, stored_from)?;
        let to_time = self.time_of(graph, to_kind, stored_to)?;
        Ok(orient(relation, stored_from, from_time, stored_to, to_time))
    }
}

/// Every edge of the graph as an arrow, newer → older. Relations come in
/// [`RelationKind::ALL`] order and, within one relation, in stored (source id, target id)
/// order, so the result is deterministic.
pub fn oriented_edges(graph: &impl GraphView) -> Result<Vec<Arrow>> {
    let mut times = NodeTimes::default();
    let mut arrows = Vec::new();
    for relation in RelationKind::ALL {
        let edges = relation_edges(graph, relation)?;
        if edges.is_empty() {
            continue;
        }
        if relation.arrow_labels().reversed.is_some() {
            let (from_kind, to_kind) = relation.endpoints();
            times.load_kind(graph, from_kind)?;
            times.load_kind(graph, to_kind)?;
        }
        for (from, to) in edges {
            arrows.push(times.arrow(graph, relation, &from, &to)?);
        }
    }
    Ok(arrows)
}

#[cfg(test)]
mod tests;
