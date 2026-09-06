//! In-house explainable decision-quality scorer: pure graph signals, no LLM.
//!
//! Combines outcome signals (T1: `outcome.rs`) with context features (T2: `context.rs`)
//! into a quality score that ALWAYS ships with its contributing reasons and node IDs.
//! Never emits a bare number.
//!
//! # Design constraints
//! - Pure layer-2 read: no writes, no LLMs, no external APIs.
//! - Works self-hosted and hosted identically.
//! - Precision-biased: only escalates to high tiers on strong, provable signals.
//! - Scores decisions, never actors.
//! - Single engine reused by the MCP surface and scheduled scan.
//!
//! # Scoring model
//! Score ∈ [0.0, 1.0] starts at 1.0 (perfect) and accrues deductions.
//! Tier thresholds are configurable via `ScorerConfig` (defaults are precision-biased).
//!
//! Outcome penalties (from graph provenance — strongest signals):
//! - Superseded rapidly (gap ≤ 5 events): -0.50
//! - Superseded quickly  (gap ≤ 20 events): -0.40
//! - Superseded (any speed): -0.30
//! - Stale premises (premised on refuted hypothesis): -0.50 (capped at one application)
//! - Contested (accepted + rejected actors): -0.25
//! - Thin structure (no options AND no evidence): -0.15
//! - Thin structure (no options only): -0.10
//! - Thin structure (no evidence only): -0.05
//!
//! Context modifiers (from condition features — weaker, precision-preserving):
//! - Agent-only authorship + unreviewed: -0.10
//! - No options AND no evidence (context side, when not already counted via outcome): -0.05

use std::time::Instant;

use serde::Serialize;

use crate::projector::GraphView;
use crate::Result;

use super::context::{
    get_decision_context, get_decision_context_candidates, DecisionContext, DecisionContextRequest,
};
use super::outcome::{
    get_decision_outcome, get_decision_quality_candidates, DecisionOutcome,
    DecisionQualityCandidatesRequest, OutcomeReason,
};
use super::shared::{query_error, MAX_QUERY_RESULTS};
use super::QueryResponse;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Quality tier for a scored decision. Precision-biased: only escalates on strong signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    /// No significant negative signals detected. Score ≥ high threshold.
    Clean,
    /// Minor concerns only (e.g. thin structure). Score ≥ mid threshold.
    MinorConcerns,
    /// Significant concern: superseded, contested, or strong context warning.
    /// Score ≥ low threshold.
    SignificantConcerns,
    /// High concern: stale premises, rapid supersession, or multiple compounding signals.
    HighConcern,
}

impl QualityTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::MinorConcerns => "minor_concerns",
            Self::SignificantConcerns => "significant_concerns",
            Self::HighConcern => "high_concern",
        }
    }
}

/// One reason contributing to a quality-score deduction, with all context needed to understand it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScorerReason {
    /// Decision was superseded; `by_id` identifies the superseding decision.
    /// `gap_events` is the ledger-event gap (smaller = faster turnaround).
    SupersededBy {
        by_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        gap_events: Option<i64>,
        /// Whether the supersession was rapid (gap ≤ 5) or quick (gap ≤ 20).
        speed: SupersessionSpeed,
        deduction: f64,
    },
    /// Decision premises on at least one refuted hypothesis.
    PremisedOnRefuted {
        hypothesis_ids: Vec<String>,
        deduction: f64,
    },
    /// Decision is actively contested — both accepted and rejected actors exist.
    Contested { deduction: f64 },
    /// Decision has thin structure: no options and/or no evidence attached.
    ThinStructure {
        no_options: bool,
        no_evidence: bool,
        deduction: f64,
    },
    /// Agent-only authorship with no human review — unverified automation.
    AgentOnlyUnreviewed { deduction: f64 },
}

/// How quickly a supersession happened relative to the decision's ledger event origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupersessionSpeed {
    /// gap ≤ 5 ledger events
    Rapid,
    /// gap ≤ 20 ledger events
    Quick,
    /// gap > 20 ledger events (or gap unknown)
    Normal,
}

/// The quality assessment for a single decision.
///
/// `score` ∈ [0.0, 1.0]; 1.0 is perfect.
/// `tier` encodes the quality tier based on configurable thresholds.
/// `reasons` carries every contributing deduction with its magnitude.
/// `contributing_ids` lists all peer node IDs referenced by the reasons
/// (superseding decisions, refuted hypotheses, etc.).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScoredDecision {
    pub decision_id: String,
    pub score: f64,
    pub tier: QualityTier,
    pub reasons: Vec<ScorerReason>,
    /// IDs of other graph nodes that drove negative signals (superseder, hypothesis IDs, etc.).
    pub contributing_ids: Vec<String>,
}

/// Configurable thresholds and weights for the scorer.
///
/// Defaults are precision-biased: the scorer only escalates to high tiers on
/// provably strong signals, keeping false-positive rates low.
#[derive(Clone, Debug)]
pub struct ScorerConfig {
    /// Score threshold at or above which a decision is `Clean`. Default: 0.85.
    pub clean_threshold: f64,
    /// Score threshold at or above which a decision is `MinorConcerns`. Default: 0.65.
    pub minor_threshold: f64,
    /// Score threshold at or above which a decision is `SignificantConcerns`. Default: 0.45.
    pub significant_threshold: f64,
    // Below significant_threshold → `HighConcern`

    // Outcome deduction weights
    pub deduct_superseded_rapid: f64,
    pub deduct_superseded_quick: f64,
    pub deduct_superseded_normal: f64,
    pub deduct_stale_premises: f64,
    pub deduct_contested: f64,
    pub deduct_thin_both: f64,
    pub deduct_thin_no_options: f64,
    pub deduct_thin_no_evidence: f64,

    // Context deduction weights
    pub deduct_agent_only_unreviewed: f64,
}

impl Default for ScorerConfig {
    fn default() -> Self {
        Self {
            clean_threshold: 0.85,
            minor_threshold: 0.65,
            significant_threshold: 0.45,

            deduct_superseded_rapid: 0.50,
            deduct_superseded_quick: 0.40,
            deduct_superseded_normal: 0.30,
            deduct_stale_premises: 0.50,
            deduct_contested: 0.25,
            deduct_thin_both: 0.15,
            deduct_thin_no_options: 0.10,
            deduct_thin_no_evidence: 0.05,

            deduct_agent_only_unreviewed: 0.10,
        }
    }
}

impl ScorerConfig {
    fn tier_for(&self, score: f64) -> QualityTier {
        if score >= self.clean_threshold {
            QualityTier::Clean
        } else if score >= self.minor_threshold {
            QualityTier::MinorConcerns
        } else if score >= self.significant_threshold {
            QualityTier::SignificantConcerns
        } else {
            QualityTier::HighConcern
        }
    }
}

/// Request for the bulk scored-scan query.
#[derive(Clone, Debug, Default)]
pub struct ScanQualityRequest {
    /// Minimum ledger offset to include (inclusive). Filters by the decision's `event_origin`.
    pub since_event_origin: Option<i64>,
    /// Maximum number of results (capped at `MAX_QUERY_RESULTS`).
    pub limit: usize,
    /// Skip N results (offset-based cursor).
    pub cursor: Option<String>,
    /// When set, only return decisions at this tier or worse (lower quality).
    /// E.g. `Some(QualityTier::SignificantConcerns)` returns SignificantConcerns + HighConcern only.
    pub min_tier: Option<QualityTier>,
}

// ---------------------------------------------------------------------------
// Single-decision scoring
// ---------------------------------------------------------------------------

/// Derive a quality score for a single decision, or `None` if the decision does not exist.
pub fn get_decision_quality_score(
    graph: &impl GraphView,
    decision_id: &str,
    config: &ScorerConfig,
) -> Result<QueryResponse<Option<ScoredDecision>>> {
    let started = Instant::now();

    let outcome_resp = get_decision_outcome(graph, decision_id)?;
    let context_resp = get_decision_context(graph, decision_id)?;

    let data = match (outcome_resp.data, context_resp.data) {
        (Some(outcome), Some(context)) => Some(score_from_signals(&outcome, &context, config)),
        (Some(outcome), None) => {
            // decision exists but context derivation returned None — shouldn't happen,
            // but score from outcome alone rather than silently dropping.
            let stub_context = DecisionContext {
                decision_id: outcome.decision_id.clone(),
                authorship: super::context::AuthorshipShape::Unknown,
                proposer_id: None,
                source: "unknown".to_owned(),
                source_ref: None,
                review: super::context::ReviewShape::Unreviewed,
                accepted_count: 0,
                rejected_count: 0,
                evidence_count: 0,
                hypothesis_count: 0,
                options_count: 0,
                rationale_chars: 0,
            };
            Some(score_from_signals(&outcome, &stub_context, config))
        }
        _ => None,
    };

    Ok(QueryResponse {
        result_count: usize::from(data.is_some()),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

// ---------------------------------------------------------------------------
// Bulk scan
// ---------------------------------------------------------------------------

/// Scan all decisions (or a filtered subset) and return quality scores, paginated.
/// Suitable for the MCP surface and the scheduled quality-scan loop.
pub fn scan_decision_quality(
    graph: &impl GraphView,
    request: &ScanQualityRequest,
    config: &ScorerConfig,
) -> Result<QueryResponse<Vec<ScoredDecision>>> {
    let started = Instant::now();
    let limit = if request.limit == 0 {
        MAX_QUERY_RESULTS
    } else {
        request.limit.min(MAX_QUERY_RESULTS)
    };
    let skip: usize = match request.cursor.as_deref() {
        None => 0,
        Some(c) => c
            .parse::<usize>()
            .map_err(|e| query_error(format!("cursor must be a non-negative offset: {e}")))?,
    };

    // Pull all outcome + context records in parallel page-scans, then join by decision_id.
    // We request a large enough page to cover the skip+limit window, then filter.
    let outcome_req = DecisionQualityCandidatesRequest {
        since_event_origin: request.since_event_origin,
        limit: 0, // 0 → MAX_QUERY_RESULTS inside that function
        cursor: None,
        only_with_signals: false,
    };
    let context_req = DecisionContextRequest {
        since_event_origin: request.since_event_origin,
        limit: 0,
        cursor: None,
    };

    let outcomes = get_decision_quality_candidates(graph, &outcome_req)?.data;
    let contexts = get_decision_context_candidates(graph, &context_req)?.data;

    // Build a context map for O(n) join.
    let context_map: std::collections::HashMap<String, DecisionContext> = contexts
        .into_iter()
        .map(|c| (c.decision_id.clone(), c))
        .collect();

    // Score all decisions in outcome order (stable ordering).
    let mut scored: Vec<ScoredDecision> = outcomes
        .into_iter()
        .map(|outcome| {
            let context = context_map
                .get(&outcome.decision_id)
                .cloned()
                .unwrap_or_else(|| DecisionContext {
                    decision_id: outcome.decision_id.clone(),
                    authorship: super::context::AuthorshipShape::Unknown,
                    proposer_id: None,
                    source: "unknown".to_owned(),
                    source_ref: None,
                    review: super::context::ReviewShape::Unreviewed,
                    accepted_count: 0,
                    rejected_count: 0,
                    evidence_count: 0,
                    hypothesis_count: 0,
                    options_count: 0,
                    rationale_chars: 0,
                });
            score_from_signals(&outcome, &context, config)
        })
        .collect();

    // Apply tier filter (precision gate).
    if let Some(min_tier) = request.min_tier {
        scored.retain(|s| s.tier >= min_tier);
    }

    // Apply pagination over the (optionally filtered) list.
    let total_after_filter = scored.len();
    let paged: Vec<_> = scored.into_iter().skip(skip).take(limit + 1).collect();
    let truncated = paged.len() > limit;
    let window: Vec<ScoredDecision> = paged.into_iter().take(limit).collect();
    let result_count = window.len();

    let _ = total_after_filter; // available for future metadata
    Ok(QueryResponse {
        result_count,
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: window,
    })
}

// ---------------------------------------------------------------------------
// Core scoring logic (pure function — no graph I/O)
// ---------------------------------------------------------------------------

/// Combine outcome and context signals into a scored decision.
/// This is a pure function: all graph I/O is done by the callers above.
pub(crate) fn score_from_signals(
    outcome: &DecisionOutcome,
    context: &DecisionContext,
    config: &ScorerConfig,
) -> ScoredDecision {
    let mut score: f64 = 1.0;
    let mut reasons: Vec<ScorerReason> = Vec::new();
    let mut contributing_ids: Vec<String> = Vec::new();

    // --- Outcome signals ---

    for reason in &outcome.reasons {
        match reason {
            OutcomeReason::SupersededBy { by_id, gap_events } => {
                let speed = match gap_events {
                    Some(g) if *g <= 5 => SupersessionSpeed::Rapid,
                    Some(g) if *g <= 20 => SupersessionSpeed::Quick,
                    _ => SupersessionSpeed::Normal,
                };
                let deduction = match speed {
                    SupersessionSpeed::Rapid => config.deduct_superseded_rapid,
                    SupersessionSpeed::Quick => config.deduct_superseded_quick,
                    SupersessionSpeed::Normal => config.deduct_superseded_normal,
                };
                score -= deduction;
                contributing_ids.push(by_id.clone()); // ubs:ignore: borrowed from &OutcomeReason; must own String for Vec<String>
                reasons.push(ScorerReason::SupersededBy {
                    by_id: by_id.clone(), // ubs:ignore: borrowed from &OutcomeReason; must own String for ScorerReason
                    gap_events: *gap_events,
                    speed,
                    deduction,
                });
            }

            OutcomeReason::PremisedOnRefuted { hypothesis_id } => {
                // Collect all refuted hypothesis IDs — apply the deduction once (capped).
                // We handle this after the loop by collecting all hypotheses first.
                contributing_ids.push(hypothesis_id.clone()); // ubs:ignore: borrowed from &OutcomeReason; must own String for Vec<String>
            }

            OutcomeReason::Contested => {
                let deduction = config.deduct_contested;
                score -= deduction;
                reasons.push(ScorerReason::Contested { deduction });
            }

            OutcomeReason::ThinStructure {
                no_options,
                no_evidence,
            } => {
                let deduction = match (*no_options, *no_evidence) {
                    (true, true) => config.deduct_thin_both,
                    (true, false) => config.deduct_thin_no_options,
                    (false, true) => config.deduct_thin_no_evidence,
                    (false, false) => 0.0, // shouldn't happen, guard
                };
                if deduction > 0.0 {
                    score -= deduction;
                    reasons.push(ScorerReason::ThinStructure {
                        no_options: *no_options,
                        no_evidence: *no_evidence,
                        deduction,
                    });
                }
            }
        }
    }

    // Apply stale-premises deduction once (not per hypothesis) — precision matters.
    let hypothesis_ids: Vec<String> = contributing_ids
        .iter()
        .filter(|id| outcome.refuted_hypothesis_ids.contains(id))
        .cloned()
        .collect();
    if !hypothesis_ids.is_empty() {
        let deduction = config.deduct_stale_premises;
        score -= deduction;
        reasons.push(ScorerReason::PremisedOnRefuted {
            hypothesis_ids: hypothesis_ids.clone(),
            deduction,
        });
        // hypothesis IDs already in contributing_ids from the loop above
    }

    // --- Context modifiers ---
    let is_agent_only = matches!(
        context.authorship,
        super::context::AuthorshipShape::AgentOnly
    );
    let is_unreviewed = matches!(context.review, super::context::ReviewShape::Unreviewed);

    if is_agent_only && is_unreviewed {
        let deduction = config.deduct_agent_only_unreviewed;
        score -= deduction;
        reasons.push(ScorerReason::AgentOnlyUnreviewed { deduction });
    }

    // Clamp score to [0.0, 1.0] — deductions can compound.
    let score = score.clamp(0.0, 1.0);
    let tier = config.tier_for(score);

    ScoredDecision {
        decision_id: outcome.decision_id.clone(),
        score,
        tier,
        reasons,
        contributing_ids,
    }
}

// ---------------------------------------------------------------------------
// Cursor helpers (public for MCP serialization)
// ---------------------------------------------------------------------------

pub fn scorer_next_cursor(skip: usize, window_len: usize) -> Option<String> {
    if window_len > 0 {
        Some((skip + window_len).to_string())
    } else {
        None
    }
}
