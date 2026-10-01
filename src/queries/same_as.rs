//! One decision, recorded more than once: an explicit `SAME_AS` link folds the copies into one
//! answer (hivemind-83cj).
//!
//! The same ruling relayed by three sessions is three records. Once a link says they are the
//! same decision (the classifier named it when it recorded the later copy, or a person approved
//! the link for copies recorded before that), `recall` and the description resolver behind `why`
//! show it once: the earliest recorded copy that matches the words asked, with the others listed
//! as `also_recorded_as`. Nothing is deleted, and nothing is folded on closeness: two decisions
//! with no link between them stay two, so a description matching both is still `Ambiguous`
//! (hivemind-tenv.1). A link is followed in either direction and transitively.
//!
//! A `SAME_AS` retraction is a ledger fact the graph does not project (`relation.removed` is
//! ledger-only for every relation), so a retracted link is still folded here.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{node_row, node_rows, optional_int, optional_string, relation_edges};

/// Another record of the same decision: its id and what it was titled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedCopy {
    pub decision_id: String,
    pub title: String,
}

/// Every `SAME_AS` link in the graph, as groups of decisions that are one decision.
pub(crate) struct SameAsLinks {
    group_of: HashMap<String, usize>,
    groups: Vec<Vec<String>>,
}

impl SameAsLinks {
    /// One read of the `SAME_AS` edges, which are only as many as there are restatements.
    pub(crate) fn load(graph: &impl GraphView) -> Result<Self> {
        let mut index_of: HashMap<String, usize> = HashMap::new();
        let mut ids: Vec<String> = Vec::new();
        let mut parent: Vec<usize> = Vec::new();
        let mut intern = |id: String| -> usize {
            *index_of.entry(id.clone()).or_insert_with(|| {
                ids.push(id);
                parent.push(parent.len());
                parent.len() - 1
            })
        };
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (from, to) in relation_edges(graph, RelationKind::SameAs)? {
            pairs.push((intern(from), intern(to)));
        }
        for (left, right) in pairs {
            let (left, right) = (find_root(&mut parent, left), find_root(&mut parent, right));
            if left != right {
                parent[right] = left;
            }
        }

        let mut by_root: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (index, id) in ids.iter().enumerate() {
            by_root
                .entry(find_root(&mut parent, index))
                .or_default()
                .push(id.clone());
        }
        let mut group_of = HashMap::new();
        let mut groups: Vec<Vec<String>> = Vec::new();
        for mut members in by_root.into_values() {
            members.sort();
            for id in &members {
                group_of.insert(id.clone(), groups.len());
            }
            groups.push(members);
        }
        Ok(Self { group_of, groups })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    fn group_of(&self, decision_id: &str) -> Option<usize> {
        self.group_of.get(decision_id).copied()
    }

    /// Every group of decisions that are one decision, members in id order.
    pub(crate) fn into_groups(self) -> Vec<Vec<String>> {
        self.groups
    }
}

fn find_root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

/// One entry of a result list after folding: the item shown and the other records of its decision.
pub(crate) struct Folded<T> {
    pub(crate) item: T,
    pub(crate) also_recorded_as: Vec<RecordedCopy>,
}

/// Folds the items that are records of one decision into one: the one recorded first (lowest
/// `event_origin`) is shown, with `absorb` taking the others' match onto it, and every other
/// record of that decision, matching or not, is listed beside it. The folded entry keeps the
/// place of the first of its records in the list. Items with no link pass through unchanged.
pub(crate) fn fold_linked<T>(
    graph: &impl GraphView,
    items: Vec<T>,
    id_of: impl Fn(&T) -> &str,
    absorb: impl Fn(&mut T, T),
) -> Result<Vec<Folded<T>>> {
    let links = SameAsLinks::load(graph)?;
    if links.is_empty() {
        return Ok(items
            .into_iter()
            .map(|item| Folded {
                item,
                also_recorded_as: Vec::new(),
            })
            .collect());
    }

    enum Slot<T> {
        Single(T),
        Linked { group: usize, items: Vec<T> },
    }
    let mut slots: Vec<Slot<T>> = Vec::with_capacity(items.len());
    let mut slot_of_group: HashMap<usize, usize> = HashMap::new();
    for item in items {
        match links.group_of(id_of(&item)) {
            None => slots.push(Slot::Single(item)),
            Some(group) => match slot_of_group.get(&group) {
                Some(&slot) => {
                    if let Slot::Linked { items, .. } = &mut slots[slot] {
                        items.push(item);
                    }
                }
                None => {
                    slot_of_group.insert(group, slots.len());
                    slots.push(Slot::Linked {
                        group,
                        items: vec![item],
                    });
                }
            },
        }
    }

    let mut folded = Vec::with_capacity(slots.len());
    for slot in slots {
        let (group, items) = match slot {
            Slot::Single(item) => {
                folded.push(Folded {
                    item,
                    also_recorded_as: Vec::new(),
                });
                continue;
            }
            Slot::Linked { group, items } => (group, items),
        };
        let records = records_in_order(graph, &links.groups[group])?;
        let Some(shown_id) = records
            .iter()
            .map(|record| record.decision_id.as_str())
            .find(|id| items.iter().any(|item| id_of(item) == *id))
            .map(str::to_owned)
        else {
            continue;
        };
        let mut shown: Option<T> = None;
        let mut others: Vec<T> = Vec::new();
        for item in items {
            if shown.is_none() && id_of(&item) == shown_id {
                shown = Some(item);
            } else {
                others.push(item);
            }
        }
        let Some(mut item) = shown else { continue };
        for other in others {
            absorb(&mut item, other);
        }
        let also_recorded_as = records
            .into_iter()
            .filter(|record| record.decision_id != shown_id)
            .collect();
        folded.push(Folded {
            item,
            also_recorded_as,
        });
    }
    Ok(folded)
}

/// The records of one decision with their titles, earliest recorded first (ties by id).
fn records_in_order(graph: &impl GraphView, ids: &[String]) -> Result<Vec<RecordedCopy>> {
    let mut records: BTreeMap<(i64, String), RecordedCopy> = BTreeMap::new();
    for id in ids {
        let row = node_row(graph, NodeKind::Decision, id)?;
        let event_origin = row
            .as_ref()
            .and_then(|row| optional_int(row, "event_origin"))
            .unwrap_or(i64::MAX);
        let title = row
            .as_ref()
            .and_then(|row| optional_string(row, "title"))
            .unwrap_or_default();
        records.insert(
            (event_origin, id.clone()),
            RecordedCopy {
                decision_id: id.clone(),
                title,
            },
        );
    }
    Ok(records.into_values().collect())
}

/// A recorded decision's id, title, project and place in the ledger: the little the restatement
/// scan (`restatement::propose_same_as_links`) compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionHeading {
    pub decision_id: String,
    pub title: String,
    pub project: Option<String>,
    pub event_origin: i64,
}

/// Every decision that has a title (a node only referenced by a request or blocker has none),
/// earliest recorded first, ties by id.
pub fn decision_headings(graph: &impl GraphView) -> Result<Vec<DecisionHeading>> {
    let mut headings: Vec<DecisionHeading> = node_rows(graph, NodeKind::Decision)?
        .into_iter()
        .filter_map(|(decision_id, row)| {
            Some(DecisionHeading {
                title: optional_string(&row, "title").filter(|title| !title.trim().is_empty())?,
                project: optional_string(&row, "project"),
                event_origin: optional_int(&row, "event_origin").unwrap_or(i64::MAX),
                decision_id,
            })
        })
        .collect();
    headings.sort_by(|left, right| {
        (left.event_origin, &left.decision_id).cmp(&(right.event_origin, &right.decision_id))
    });
    Ok(headings)
}

/// The groups of decisions already linked `SAME_AS`, each in id order.
pub fn same_as_groups(graph: &impl GraphView) -> Result<Vec<Vec<String>>> {
    Ok(SameAsLinks::load(graph)?.into_groups())
}

/// Every `(newer, older)` pair joined by a `SUPERSEDES` edge. A decision that replaced another
/// is a different decision, however alike their words, and is never a restatement of it.
pub fn supersession_pairs(graph: &impl GraphView) -> Result<BTreeSet<(String, String)>> {
    Ok(relation_edges(graph, RelationKind::Supersedes)?
        .into_iter()
        .collect())
}

#[cfg(test)]
mod tests;
