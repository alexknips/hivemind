// Layer-3: compactification view over a decision subgraph.
// Non-destructive — no ledger writes; all graph access via Layer-2 functions.
// Per AGENTS.md: no direct graph queries here; every graph access goes through a Layer-2 fn.

use std::time::Instant;

use serde::Serialize;

use crate::events::BlockerPriority;
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::active_blockers::{
    get_active_decision_blockers, ActiveDecisionBlockersRequest, DecisionBlockerFilters,
};
use super::decision::{get_decision, DecisionView};
use super::shared::{
    hypothesis_evidence_ids, hypothesis_statement, neighbor_ids, neighbor_pairs, node_rows,
    optional_string, query_error, Direction,
};
use super::status::{DecisionStatus, HypothesisStatus};
use super::supersession::get_supersession_chain;
use super::QueryResponse;

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

/// Present when the focal decision is part of a supersession chain (chain_length > 1).
/// `oldest_id` is the root; full ordered list is in `elided.chain_decision_ids`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SupersessionSummary {
    pub chain_length: usize,
    pub oldest_id: String,
}

/// Present only when the terminal decision status is `Contested`.
/// Both sides are always preserved — contested is never compacted.
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
    /// For `status=supported`: evidence IDs kept for provenance (IDs only, content elided).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supporting_evidence_ids: Option<Vec<String>>,
    /// For `status=refuted`: IDs of evidence that refuted this assumption.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refuting_evidence_ids: Option<Vec<String>>,
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
    /// Number of superseded (non-terminal) decisions in the chain.
    pub superseded_decision_count: usize,
    /// Unchosen options on the terminal decision (chosen option is SIGNAL; others elided).
    pub unchosen_option_count: usize,
    /// Resolved blockers for the terminal decision (already handled; historical noise).
    pub resolved_blocker_count: usize,
    /// Notification nodes elided (operational ephemera, not decision content).
    pub notification_count: usize,
    /// Ordered decision IDs for all superseded decisions (oldest first, terminal excluded).
    /// Enables a follow-up `get_supersession_chain` without re-querying the compact view.
    pub chain_decision_ids: Vec<String>,
    /// Ledger offsets for elided nodes (enables audit without re-querying the compact view).
    pub event_origins: Vec<u64>,
}

/// Layer-3 compactification entry point.
///
/// Resolves the focal decision to its terminal (most-current) node, then builds a
/// `CompactView` by filtering signal from noise per the spec in
/// `docs/COMPACTIFICATION_SPEC.md`.  The underlying ledger is never written.
pub fn get_compact_view(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<CompactView>> {
    // ubs:ignore: Instant measures query latency only; it does not generate secrets.
    let started = Instant::now();
    let decision_id = decision_id.trim();
    if decision_id.is_empty() {
        return Err(query_error("decision_id must not be empty").into());
    }

    // 1. Verify focal decision exists.
    let focal = get_decision(graph, decision_id)?;
    if focal.data.is_none() {
        return Err(query_error(format!("decision not found: {decision_id}")).into());
    }

    // 2. Find the terminal (most-current) node in the supersession chain.
    let chain_response = get_supersession_chain(graph, decision_id)?;
    let chain = &chain_response.data;
    let terminal_id = chain
        .decision_ids
        .last()
        .ok_or_else(|| query_error("supersession chain returned empty list"))?
        .clone();

    // 3. Fetch the terminal decision (may equal the focal decision).
    let terminal_response = get_decision(graph, &terminal_id)?;
    let terminal = terminal_response
        .data
        .ok_or_else(|| query_error(format!("terminal decision not found: {terminal_id}")))?;

    // 4. Supersession summary + elided chain IDs (oldest … penultimate; terminal excluded).
    let chain_length = chain.decision_ids.len();
    let (supersession_chain, chain_decision_ids) = if chain_length > 1 {
        let oldest_id = chain.decision_ids[0].clone();
        let chain_ids = chain.decision_ids[..chain_length - 1].to_vec();
        (
            Some(SupersessionSummary {
                chain_length,
                oldest_id,
            }),
            chain_ids,
        )
    } else {
        (None, Vec::new())
    };

    // 5. Contest view — contested decisions are never compacted; both sides always preserved.
    let contest = if terminal.status == DecisionStatus::Contested {
        let accepted_by = neighbor_ids(
            graph,
            &terminal_id,
            RelationKind::AcceptedBy,
            NodeKind::Actor,
            "actor_id",
        )?;
        let rejected_by = neighbor_ids(
            graph,
            &terminal_id,
            RelationKind::RejectedBy,
            NodeKind::Actor,
            "actor_id",
        )?;
        Some(ContestView {
            accepted_by,
            rejected_by,
        })
    } else {
        None
    };

    // 6. Hypothesis summary views for all hypotheses assumed by the terminal decision.
    let mut hypotheses = Vec::with_capacity(terminal.hypotheses.len());
    for hyp in &terminal.hypotheses {
        let statement = hypothesis_statement(graph, &hyp.id)?;
        let (supporting_evidence_ids, refuting_evidence_ids) = match hyp.status {
            HypothesisStatus::Supported => {
                // Mayor Q2: KEEP evidence IDs for supported hypotheses (preserve provenance cheaply).
                let ids = hypothesis_evidence_ids(graph, &hyp.id, RelationKind::Supports)?;
                (Some(ids), None)
            }
            HypothesisStatus::Refuted => {
                // SIGNAL: refuted assumptions propagate staleness — always surface refuting evidence.
                let ids = hypothesis_evidence_ids(graph, &hyp.id, RelationKind::Refutes)?;
                (None, Some(ids))
            }
            HypothesisStatus::Open => (None, None),
        };
        hypotheses.push(HypothesisSummaryView {
            id: hyp.id.clone(),
            status: hyp.status,
            statement,
            supporting_evidence_ids,
            refuting_evidence_ids,
        });
    }

    // 7. Active (unresolved) blockers for the terminal decision.
    let blockers_response = get_active_decision_blockers(
        graph,
        &ActiveDecisionBlockersRequest {
            filters: DecisionBlockerFilters {
                decision_ids: vec![terminal_id.clone()],
                ..Default::default()
            },
            limit: 1000,
            cursor: None,
        },
    )?;
    let active_blockers: Vec<BlockerSummary> = blockers_response
        .data
        .items
        .into_iter()
        .map(|b| BlockerSummary {
            id: b.id,
            reason: b.reason,
            priority: b.priority,
            blocked_actor_id: b.blocked_actor_id,
        })
        .collect();

    // 8. Unchosen options count + event_origins for elided option edges.
    let chosen_count = usize::from(terminal.chosen_option_id.is_some());
    let unchosen_option_count = terminal.option_ids.len().saturating_sub(chosen_count);

    let mut event_origins: Vec<u64> = Vec::new();
    if unchosen_option_count > 0 {
        let has_option_pairs = neighbor_pairs(
            graph,
            NodeKind::Decision,
            &terminal_id,
            RelationKind::HasOption,
            NodeKind::Option,
            Direction::Outgoing,
        )?;
        for (option_id, opt_event_origin) in &has_option_pairs {
            let is_chosen = terminal.chosen_option_id.as_deref() == Some(option_id.as_str());
            if !is_chosen {
                if let Some(origin) = opt_event_origin {
                    if *origin >= 0 {
                        #[allow(clippy::cast_sign_loss)]
                        event_origins.push(*origin as u64);
                    }
                }
            }
        }
    }

    // 9. Resolved blocker count for this decision (historical noise; compact but counted).
    let resolved_blocker_count = {
        let all_blockers = node_rows(graph, NodeKind::Blocker)?;
        all_blockers
            .values()
            .filter(|row| {
                optional_string(row, "decision_id").as_deref() == Some(terminal_id.as_str())
                    && optional_string(row, "resolved_at").is_some()
            })
            .count()
    };

    let evidence_ids = terminal.evidence_ids.clone();

    let elided = ElidedSummary {
        superseded_decision_count: chain_length.saturating_sub(1),
        unchosen_option_count,
        resolved_blocker_count,
        notification_count: 0,
        chain_decision_ids,
        event_origins,
    };

    let view = CompactView {
        decision: terminal,
        supersession_chain,
        contest,
        hypotheses,
        evidence_ids,
        active_blockers,
        elided,
    };

    Ok(QueryResponse {
        result_count: 1,
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: view,
    })
}
