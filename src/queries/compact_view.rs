use std::collections::BTreeSet;
use std::time::Instant;

use serde::Serialize;

use crate::events::BlockerPriority;
use crate::projector::{GraphView, RelationKind};
use crate::Result;

use super::active_blockers::{ActiveDecisionBlockersRequest, DecisionBlockerFilters};
use super::decision::{get_hypothesis_statement, DecisionView};
use super::neighborhood::{get_decision_neighborhood, NeighborhoodRequest};
use super::shared::{query_error, MAX_QUERY_RESULTS};
use super::status::{DecisionStatus, HypothesisStatus};
use super::supersession::get_supersession_chain;
use super::{get_active_decision_blockers, get_decision, QueryResponse};

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompactView {
    pub decision: DecisionView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersession_chain: Option<SupersessionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contest: Option<ContestView>,
    pub hypotheses: Vec<HypothesisSummaryView>,
    pub evidence_ids: Vec<String>,
    pub active_blockers: Vec<BlockerSummary>,
    pub elided: ElidedSummary,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SupersessionSummary {
    pub chain_length: usize,
    pub oldest_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContestView {
    pub accepted_by: Vec<String>,
    pub rejected_by: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HypothesisSummaryView {
    pub id: String,
    pub status: HypothesisStatus,
    pub statement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refuting_evidence_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supporting_evidence_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockerSummary {
    pub id: String,
    pub reason: String,
    pub priority: BlockerPriority,
    pub blocked_actor_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ElidedSummary {
    pub superseded_decision_count: usize,
    pub unchosen_option_count: usize,
    pub resolved_blocker_count: usize,
    pub notification_count: usize,
    pub event_origins: Vec<u64>,
}

// ---------------------------------------------------------------------------
// Layer-3 function — wraps Layer-2 only; no direct graph queries
// ---------------------------------------------------------------------------

pub fn get_compact_view(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<CompactView>>> {
    let started = Instant::now();
    let decision_id = decision_id.trim();
    if decision_id.is_empty() {
        return Err(query_error("decision_id must not be empty").into());
    }

    // 1. Resolve focal decision
    let focal = get_decision(graph, decision_id)?;
    let Some(focal_decision) = focal.data else {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: None,
        });
    };

    // 2. Supersession chain → find terminal (newest, last in chain)
    let chain = get_supersession_chain(graph, decision_id)?.data;
    let terminal_id = chain
        .decision_ids
        .last()
        .cloned()
        .unwrap_or_else(|| decision_id.to_owned());

    // 3. Terminal decision (reuse focal when chain has length 1)
    let terminal_decision = if terminal_id == decision_id {
        focal_decision
    } else {
        get_decision(graph, &terminal_id)?
            .data
            .unwrap_or(focal_decision)
    };

    // 4. Neighborhood of the terminal decision
    let neighborhood =
        get_decision_neighborhood(graph, &terminal_id, &NeighborhoodRequest::all())?.data;

    // 5. Active blockers scoped to this decision
    let blockers = get_active_decision_blockers(
        graph,
        &ActiveDecisionBlockersRequest {
            filters: DecisionBlockerFilters {
                decision_ids: vec![terminal_id.clone()],
                ..DecisionBlockerFilters::default()
            },
            limit: MAX_QUERY_RESULTS,
            cursor: None,
        },
    )?
    .data;

    // -----------------------------------------------------------------------
    // Extract contest actors (only meaningful when status == Contested)
    // -----------------------------------------------------------------------
    let contest = if terminal_decision.status == DecisionStatus::Contested {
        let accepted_by: Vec<String> = neighborhood
            .edges
            .iter()
            .filter(|e| e.relation == RelationKind::AcceptedBy && e.from == terminal_id)
            .map(|e| e.to.to_owned())
            .collect();
        let rejected_by: Vec<String> = neighborhood
            .edges
            .iter()
            .filter(|e| e.relation == RelationKind::RejectedBy && e.from == terminal_id)
            .map(|e| e.to.to_owned())
            .collect();
        Some(ContestView {
            accepted_by,
            rejected_by,
        })
    } else {
        None
    };

    // -----------------------------------------------------------------------
    // Supersession summary (omit when the decision is not part of a chain)
    // -----------------------------------------------------------------------
    let superseded_decision_count = chain.decision_ids.len().saturating_sub(1);
    let supersession_chain = if chain.decision_ids.len() > 1 {
        Some(SupersessionSummary {
            chain_length: chain.decision_ids.len(),
            oldest_id: chain.decision_ids.first().cloned().unwrap_or_default(),
        })
    } else {
        None
    };

    // -----------------------------------------------------------------------
    // Options: track chosen set to count unchosen
    // -----------------------------------------------------------------------
    let chose_set: BTreeSet<String> = neighborhood
        .edges
        .iter()
        .filter(|e| e.relation == RelationKind::Chose && e.from == terminal_id)
        .map(|e| e.to.to_owned())
        .collect();
    let unchosen_option_count = neighborhood
        .edges
        .iter()
        .filter(|e| {
            e.relation == RelationKind::HasOption
                && e.from == terminal_id
                && !chose_set.contains(&e.to)
        })
        .count();

    // -----------------------------------------------------------------------
    // Hypotheses: build summaries with evidence IDs from neighborhood hop-2
    // -----------------------------------------------------------------------
    let mut hypotheses = Vec::with_capacity(terminal_decision.hypotheses.len());
    for hyp in &terminal_decision.hypotheses {
        let refuting_ids: Vec<String> = neighborhood
            .edges
            .iter()
            .filter(|e| e.relation == RelationKind::Refutes && e.to == hyp.id)
            .map(|e| e.from.to_owned())
            .collect();
        let supporting_ids: Vec<String> = neighborhood
            .edges
            .iter()
            .filter(|e| e.relation == RelationKind::Supports && e.to == hyp.id)
            .map(|e| e.from.to_owned())
            .collect();

        let statement = get_hypothesis_statement(graph, &hyp.id)?.unwrap_or_default();

        let (refuting_evidence_ids, supporting_evidence_ids) = match hyp.status {
            HypothesisStatus::Refuted => (Some(refuting_ids), None),
            HypothesisStatus::Supported => (
                None,
                if supporting_ids.is_empty() {
                    None
                } else {
                    Some(supporting_ids)
                },
            ),
            HypothesisStatus::Open => (None, None),
        };

        hypotheses.push(HypothesisSummaryView {
            id: hyp.id.to_owned(),
            status: hyp.status,
            statement,
            refuting_evidence_ids,
            supporting_evidence_ids,
        });
    }

    // -----------------------------------------------------------------------
    // Active blockers summary
    // -----------------------------------------------------------------------
    let active_blockers: Vec<BlockerSummary> = blockers
        .items
        .into_iter()
        .map(|b| BlockerSummary {
            id: b.id,
            reason: b.reason,
            priority: b.priority,
            blocked_actor_id: b.blocked_actor_id,
        })
        .collect();

    // -----------------------------------------------------------------------
    // ElidedSummary — collect event_origins for elided graph elements
    // -----------------------------------------------------------------------
    let mut event_origins: Vec<u64> = Vec::new();

    // Superseded decision edges (terminal -[SUPERSEDES]-> older)
    for edge in &neighborhood.edges {
        if edge.relation == RelationKind::Supersedes && edge.from == terminal_id {
            if let Some(origin) = edge.event_origin {
                if let Ok(u) = u64::try_from(origin) {
                    event_origins.push(u);
                }
            }
        }
    }

    // Unchosen option edges
    for edge in &neighborhood.edges {
        if edge.relation == RelationKind::HasOption
            && edge.from == terminal_id
            && !chose_set.contains(&edge.to)
        {
            if let Some(origin) = edge.event_origin {
                if let Ok(u) = u64::try_from(origin) {
                    event_origins.push(u);
                }
            }
        }
    }

    event_origins.sort_unstable();
    event_origins.dedup();

    let elided = ElidedSummary {
        superseded_decision_count,
        unchosen_option_count,
        resolved_blocker_count: 0,
        notification_count: 0,
        event_origins,
    };

    Ok(QueryResponse {
        result_count: 1,
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: Some(CompactView {
            evidence_ids: terminal_decision.evidence_ids.clone(),
            decision: terminal_decision,
            supersession_chain,
            contest,
            hypotheses,
            active_blockers,
            elided,
        }),
    })
}
