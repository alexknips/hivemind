//! What the importance ranking reads, in bulk, as plain facts: every decision with its standing,
//! its deciders and the decisions that follow from it.
//!
//! Pure Layer-2 reads: no interpretation, no ranking, no LLM, no write. Where
//! [`get_decision_brief`](super::get_decision_brief) reads one decision with anchored lookups,
//! this reads the decisions of the whole graph in [`IMPORTANCE_FACT_READS`] bulk reads, a number
//! that does not grow with the number of decisions. What grows is the rows those reads return.
//! Nothing here says whether a decision matters; that judgement is Layer 3's
//! (`crate::importance`) and this module knows nothing of it.
//!
//! Every read is `relation_edges` or `node_rows`, the two shapes every backend answers, so
//! memory, Postgres and Kuzu agree on it. A node that was only ever named (by a request, a
//! blocker, a supersession) and never proposed has no title and is not a decision here.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::project_label::ProjectLabels;
use super::shared::{
    node_rows, optional_datetime, optional_float, optional_int, optional_string, relation_edges,
};
use super::status::{Decider, DecisionStandings, DecisionStatus};

/// Bulk reads [`get_importance_facts`] issues, whatever the size of the graph: the decision
/// nodes, the project nodes (for their display names), the `FOLLOWS_FROM` links, the
/// `PROPOSED_BY` links, and three for who superseded, accepted or rejected what.
pub const IMPORTANCE_FACT_READS: usize = 7;

/// What a model said about a decision's importance, as the graph keeps it: the three numbers and
/// where to find the rest. The explanations the model gave stay in the ledger event.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelJudgedFacts {
    /// Unbounded, log-scaled magnitude (severity times reach).
    pub stakes: f64,
    /// In `[0,1]`: 0 fully reversible, 1 fully irreversible.
    pub irreversibility: f64,
    /// In `[0,1]`: 0 not actionable, 1 fully actionable.
    pub actionability: f64,
    /// The model and prompt version that judged it. Absent for the older score events, which
    /// keyed the scores by a capture node and did not store them.
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    /// The ledger offset of the assessment event, when the graph recorded it.
    pub assessed_at_origin: Option<i64>,
}

/// One decision as its record states it.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionFacts {
    pub title: String,
    /// The project address the decision is filed under; `None` for none recorded.
    pub project: Option<String>,
    /// What a person calls that project.
    pub project_label: String,
    pub occurred_at: Option<DateTime<Utc>>,
    /// The ledger offset that created the node: the canonical "newer than" order.
    pub event_origin: Option<i64>,
    pub status: DecisionStatus,
    /// Who accepted it, sorted by actor id, each with the kind the actor-id prefix gives.
    pub deciders: Vec<Decider>,
    /// Who rejected it, sorted by actor id.
    pub rejected_by: Vec<String>,
    /// The actor who recorded it (`PROPOSED_BY`): often a scribe, never read as the decider.
    pub recorded_by: Option<String>,
    /// The human whose delegation an agent's acceptance fell within.
    pub delegated_by: Option<String>,
    /// Present only when a model judged the importance factors.
    pub model_judged: Option<ModelJudgedFacts>,
}

/// Every decision in the graph, and the `FOLLOWS_FROM` links between them.
#[derive(Debug, Default)]
pub struct ImportanceFacts {
    pub decisions: BTreeMap<String, DecisionFacts>,
    /// For each decision, the decisions that follow from it directly: sorted, distinct, and
    /// never the decision itself.
    pub direct_dependents: BTreeMap<String, Vec<String>>,
}

/// The decisions of the graph and what follows from each. See the module docs for what it costs.
pub fn get_importance_facts(graph: &impl GraphView) -> Result<ImportanceFacts> {
    let standings = DecisionStandings::load(graph)?;
    let labels = ProjectLabels::from_graph(graph)?;

    let mut recorded_by: BTreeMap<String, String> = BTreeMap::new();
    // Edges arrive sorted by (decision, actor): the first recorder by actor id is the one shown.
    for (decision_id, actor_id) in relation_edges(graph, RelationKind::ProposedBy)? {
        recorded_by.entry(decision_id).or_insert(actor_id);
    }

    let mut decisions = BTreeMap::new();
    for (decision_id, row) in node_rows(graph, NodeKind::Decision)? {
        let Some(title) = optional_string(&row, "title") else {
            continue;
        };
        let project = optional_string(&row, "project");
        decisions.insert(
            decision_id.clone(), // ubs:ignore: the map owns its ids; one clone per decision
            DecisionFacts {
                title,
                project_label: labels.label_of(project.as_deref()),
                project,
                occurred_at: optional_datetime(&row, "occurred_at")?,
                event_origin: optional_int(&row, "event_origin"),
                status: standings.status_of(&decision_id),
                deciders: standings.deciders_of(&decision_id),
                rejected_by: standings.rejecters_of(&decision_id).to_vec(),
                recorded_by: recorded_by.remove(&decision_id),
                delegated_by: optional_string(&row, "delegated_by"),
                model_judged: model_judged(&row),
            },
        );
    }

    let mut direct_dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Edges run follower -> premise and arrive sorted by (follower, premise); a decision that
    // follows from itself rests on nothing but itself.
    for (follower, premise) in relation_edges(graph, RelationKind::FollowsFrom)? {
        if follower == premise {
            continue;
        }
        let followers = direct_dependents.entry(premise).or_default();
        if followers.last() != Some(&follower) {
            followers.push(follower);
        }
    }

    Ok(ImportanceFacts {
        decisions,
        direct_dependents,
    })
}

/// The model's three importance numbers off a decision row, when it has all three.
fn model_judged(row: &GraphRow) -> Option<ModelJudgedFacts> {
    let stakes = optional_float(row, "importance_stakes")?;
    let irreversibility = optional_float(row, "importance_irreversibility")?;
    let actionability = optional_float(row, "importance_actionability")?;
    let assessment: Option<Value> = optional_string(row, "model_assessment")
        .and_then(|stored| serde_json::from_str(&stored).ok());
    let text = |key: &str| {
        assessment
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Some(ModelJudgedFacts {
        stakes,
        irreversibility,
        actionability,
        model: text("model"),
        prompt_version: text("prompt_version"),
        assessed_at_origin: optional_int(row, "model_assessment_origin"),
    })
}

#[cfg(test)]
mod tests;
