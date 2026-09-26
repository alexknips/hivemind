//! What every decision rests on, in bulk, as plain facts: the input the attention findings read.
//!
//! Pure Layer-2 reads: no interpretation, no ranking, no LLM, no write. Where
//! [`get_record_facts`](super::get_record_facts) reads one decision with anchored lookups, this
//! reads the grounding relations of the whole graph in [`GROUNDING_FACT_READS`] bulk reads, a
//! number that does not grow with the number of decisions. What grows is the rows those reads
//! return: one id pair per grounding link, one row per hypothesis and per evidence item. Nothing
//! here says whether anything needs a look; that judgement is Layer 3's and this module knows
//! nothing of it.
//!
//! Every read is `relation_edges` or `node_rows`, the two shapes every backend answers, so
//! memory, Postgres and Kuzu agree on it. A link to a node that does not exist reads as a link;
//! a hypothesis with no node is simply absent from [`GroundingFacts::hypotheses`].

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use crate::events::HypothesisKind;
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::grounding::hypothesis_facts_from_row;
use super::shared::{node_row, node_rows, optional_datetime, relation_edges};
use super::status::{hypothesis_status_from_evidence, DecisionStandings, HypothesisStatus};

/// Bulk reads [`get_grounding_facts`] issues, whatever the size of the graph: three for who
/// superseded, accepted or rejected what, five for the grounding links (`FOLLOWS_FROM`,
/// `BASED_ON`, `PREMISED_ON_DIRECT`, `PREMISED_ON`, `CHOSE`), two for the hypothesis and evidence
/// rows, and two for what refutes or supports a hypothesis.
pub const GROUNDING_FACT_READS: usize = 12;

/// One assumption or declared bet, as its node states it and as the graph has judged it so far.
#[derive(Clone, Debug, PartialEq)]
pub struct HypothesisRecord {
    pub kind: HypothesisKind,
    /// When a bet is to be checked, as recorded.
    pub check_by: Option<DateTime<Utc>>,
    /// Refuted when any evidence refutes it, else supported when any supports it, else open:
    /// the status rule [`derive_hypothesis_status`](super::derive_hypothesis_status) applies.
    pub status: HypothesisStatus,
    /// Every evidence item that refutes it, sorted by id.
    pub refuted_by: Vec<String>,
}

/// The grounding of every decision at once. Every list is sorted and distinct.
#[derive(Debug, Default)]
pub struct GroundingFacts {
    /// `(decision, prior decision it follows from)` for every `FOLLOWS_FROM` link.
    pub premise_links: Vec<(String, String)>,
    /// `(decision, evidence)` for every `BASED_ON` link.
    pub evidence_links: Vec<(String, String)>,
    /// `(decision, hypothesis)` for every assumption or bet a decision rests on, whether it hangs
    /// off the decision itself or off the option it chose.
    pub hypothesis_links: Vec<(String, String)>,
    /// Every hypothesis node, by id.
    pub hypotheses: BTreeMap<String, HypothesisRecord>,
    /// When each evidence item was recorded, for those whose record states it.
    pub evidence_recorded_at: BTreeMap<String, DateTime<Utc>>,
    /// Who superseded, accepted or rejected which decision.
    pub standings: DecisionStandings,
}

/// The grounding of every decision in the graph. See the module docs for what it costs.
pub fn get_grounding_facts(graph: &impl GraphView) -> Result<GroundingFacts> {
    let standings = DecisionStandings::load(graph)?;

    let premise_links = distinct_links(graph, RelationKind::FollowsFrom)?;
    let evidence_links = distinct_links(graph, RelationKind::BasedOn)?;
    let hypothesis_links = hypothesis_links(graph)?;

    // Edges run evidence -> hypothesis and arrive sorted by (evidence, hypothesis), so each
    // hypothesis's refuting evidence is sorted by id.
    // The same refutation asserted twice is one.
    let mut refuted_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (evidence_id, hypothesis_id) in relation_edges(graph, RelationKind::Refutes)? {
        let refuters = refuted_by.entry(hypothesis_id).or_default();
        if refuters.last() != Some(&evidence_id) {
            refuters.push(evidence_id);
        }
    }
    let supported: BTreeSet<String> = relation_edges(graph, RelationKind::Supports)?
        .into_iter()
        .map(|(_, hypothesis_id)| hypothesis_id)
        .collect();

    let mut hypotheses = BTreeMap::new();
    for (hypothesis_id, row) in node_rows(graph, NodeKind::Hypothesis)? {
        let facts = hypothesis_facts_from_row(&row)?;
        let refuted_by = refuted_by.remove(&hypothesis_id).unwrap_or_default();
        let status = hypothesis_status_from_evidence(
            !refuted_by.is_empty(),
            supported.contains(&hypothesis_id),
        );
        hypotheses.insert(
            hypothesis_id,
            HypothesisRecord {
                kind: facts.kind,
                check_by: facts.check_by,
                status,
                refuted_by,
            },
        );
    }

    let mut evidence_recorded_at = BTreeMap::new();
    for (evidence_id, row) in node_rows(graph, NodeKind::Evidence)? {
        if let Some(recorded_at) = optional_datetime(&row, "recorded_at")? {
            evidence_recorded_at.insert(evidence_id, recorded_at);
        }
    }

    Ok(GroundingFacts {
        premise_links,
        evidence_links,
        hypothesis_links,
        hypotheses,
        evidence_recorded_at,
        standings,
    })
}

/// When each of these decisions was made, as its own record states it; a decision that is not in
/// the graph, or whose record states no time, is absent. Anchored lookups: one per id, never a
/// scan.
pub fn get_decision_times<'a>(
    graph: &impl GraphView,
    decision_ids: impl IntoIterator<Item = &'a str>,
) -> Result<BTreeMap<String, DateTime<Utc>>> {
    let distinct: BTreeSet<&str> = decision_ids.into_iter().collect();
    let times = distinct
        .into_iter()
        .map(|decision_id| decision_time(graph, decision_id))
        .collect::<Result<Vec<_>>>()?;
    Ok(times.into_iter().flatten().collect())
}

fn decision_time(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Option<(String, DateTime<Utc>)>> {
    let Some(row) = node_row(graph, NodeKind::Decision, decision_id)? else {
        return Ok(None);
    };
    Ok(optional_datetime(&row, "occurred_at")?
        .map(|occurred_at| (decision_id.to_owned(), occurred_at)))
}

/// Every `(from, to)` pair of one relation, sorted and distinct. The same pair asserted by more
/// than one event is one link.
fn distinct_links(graph: &impl GraphView, relation: RelationKind) -> Result<Vec<(String, String)>> {
    let mut links = relation_edges(graph, relation)?;
    links.dedup();
    Ok(links)
}

/// `(decision, hypothesis)` for every assumption or bet a decision rests on. One hangs off the
/// decision (`PREMISED_ON_DIRECT`) when grounded later and off the option the decision chose
/// (`CHOSE` then `PREMISED_ON`) when named at capture; either way it is the decision's.
fn hypothesis_links(graph: &impl GraphView) -> Result<Vec<(String, String)>> {
    let mut links: BTreeSet<(String, String)> =
        relation_edges(graph, RelationKind::PremisedOnDirect)?
            .into_iter()
            .collect();

    let mut premised_by_option: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (option_id, hypothesis_id) in relation_edges(graph, RelationKind::PremisedOn)? {
        premised_by_option
            .entry(option_id)
            .or_default()
            .push(hypothesis_id);
    }
    for (decision_id, option_id) in relation_edges(graph, RelationKind::Chose)? {
        let Some(hypothesis_ids) = premised_by_option.get(&option_id) else {
            continue;
        };
        for hypothesis_id in hypothesis_ids {
            // ubs:ignore: one owned pair per link; the option map is read-only here
            links.insert((decision_id.clone(), hypothesis_id.clone()));
        }
    }
    Ok(links.into_iter().collect())
}

#[cfg(test)]
mod tests;
