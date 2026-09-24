//! Premise-chain traversal: walks the FOLLOWS_FROM edge graph with cycle protection.

use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{neighbor_pairs, query_error, Direction};

/// The chain of decisions that connects `from_id` to `to_id` over `FOLLOWS_FROM` — `from_id`
/// rests on the second id, which rests on the third, and so on until `to_id` — or `None` when
/// `from_id` does not (transitively) rest on `to_id`. Ids run from `from_id` to `to_id`
/// inclusive; `from_id == to_id` is the one-element chain.
///
/// Deterministic: breadth-first with neighbours in id order, so the shortest chain wins and ties
/// break by id. Every decision is visited at most once, so the walk terminates on any graph,
/// including one that already holds a cycle, and is bounded by the number of decisions reachable
/// from `from_id`.
pub fn follows_from_path(
    graph: &impl GraphView,
    from_id: &str,
    to_id: &str,
) -> Result<Option<Vec<String>>> {
    if from_id.trim().is_empty() || to_id.trim().is_empty() {
        return Err(query_error("decision ids must not be empty").into());
    }

    // Decisions in discovery order, and for each the index of the decision it was discovered from
    // (the start points at itself). Indices instead of a second map of ids keeps the walk
    // allocation-light; membership is a scan of `reached`, which only ever holds the decisions
    // one decision transitively rests on.
    let mut reached: Vec<String> = vec![from_id.to_owned()];
    let mut discovered_from: Vec<usize> = vec![0];
    let mut cursor = 0;

    loop {
        let neighbors = {
            let Some(current) = reached.get(cursor) else {
                return Ok(None);
            };
            if current == to_id {
                return Ok(Some(chain_ending_at(&reached, &discovered_from, cursor)));
            }
            neighbor_pairs(
                graph,
                NodeKind::Decision,
                current,
                RelationKind::FollowsFrom,
                NodeKind::Decision,
                Direction::Outgoing,
            )?
        };
        for (next, _event_origin) in neighbors {
            if !reached.contains(&next) {
                reached.push(next);
                discovered_from.push(cursor);
            }
        }
        cursor += 1;
    }
}

/// The ids from the start of the walk to `reached[end]`, following each decision back to the one
/// it was discovered from. Every parent index is smaller than its child's, so this terminates.
fn chain_ending_at(reached: &[String], discovered_from: &[usize], end: usize) -> Vec<String> {
    let mut indices = vec![end];
    let mut index = end;
    while index != 0 {
        index = discovered_from.get(index).copied().unwrap_or(0);
        indices.push(index);
    }
    indices
        .iter()
        .rev()
        .filter_map(|index| reached.get(*index))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
