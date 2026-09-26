//! Failure-mode attribution: which conditions predict decisions that do not hold up?
//!
//! Joins outcome signals (T1: `outcome.rs`) with context features (T2: `context.rs`)
//! and computes AGGREGATE failure-rate patterns across condition dimensions. Delivers
//! the research question from bead hivemind-he9a.4:
//!
//! - Does a given MODEL/source underperform on certain decision types?
//! - Does missing/thin CONTEXT predict failure?
//! - Does human review improve outcomes?
//! - Do joint human+AI decisions hold up better than either alone?
//! - Do agent decisions made within a human's delegation hold up differently from ones an
//!   agent made alone? (`by_delegation`, hivemind-zdsh.6)
//! - Do decisions that record little on some aspect of how they were made (options weighed,
//!   what they rest on) fail more often? (`by_condition`, fed by the caller: see
//!   [`DecisionCondition`])
//!
//! # Design constraints
//! - Pure layer-2 read: no writes, no LLMs, no external APIs.
//! - Reports AGGREGATE patterns over authorship/review/source/context-richness.
//!   Never per-person rankings. Individuals appear only in their own self-view.
//! - Effect sizes are failure-rate deltas vs the corpus baseline.
//! - Confidence is honest: groups with n < 10 are flagged as LOW confidence.
//! - "Failure" = `held_up == false` (superseded OR stale premises OR contested). How a decision
//!   was made is never part of that definition: it is a condition to group failures by, never
//!   a failure.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use serde::Serialize;

use crate::projector::GraphView;
use crate::Result;

use super::context::{
    get_decision_context_candidates, AuthorshipShape, DecisionContext, DecisionContextRequest,
    ReviewShape,
};
use super::outcome::{get_decision_quality_candidates, DecisionQualityCandidatesRequest};
use super::shared::MAX_QUERY_RESULTS;
use super::QueryResponse;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Minimum sample size below which a group's finding is marked LOW confidence.
const LOW_CONFIDENCE_THRESHOLD: usize = 10;
/// Minimum sample size below which a group's finding is marked MEDIUM confidence.
const MEDIUM_CONFIDENCE_THRESHOLD: usize = 30;

/// Corpus-wide baseline statistics.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CorpusStats {
    /// Total decisions analysed.
    pub total_decisions: usize,
    /// Decisions where `held_up == false` (superseded, stale, or contested).
    pub failed_decisions: usize,
    /// `failed_decisions / total_decisions` or 0.0 when total = 0.
    pub baseline_failure_rate: f64,
    /// Per-signal counts across the corpus.
    pub signal_breakdown: SignalBreakdown,
}

/// How many decisions exhibit each failure signal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SignalBreakdown {
    pub superseded_count: usize,
    pub stale_premises_count: usize,
    pub contested_count: usize,
}

/// One named condition a decision was in that this analysis does not read from the graph: the
/// caller derives it (for example how far the record supports each aspect of decision quality)
/// and hands it in, so failure rates can be grouped by it. The graph read stays pure and works
/// without any caller supplying conditions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionCondition {
    /// The axis the condition belongs to; becomes the group's `dimension`.
    pub dimension: String,
    /// Which side of that axis the decision falls on; becomes the group's `group_label`.
    pub label: String,
}

/// Confidence level for a group finding, based on sample size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    /// n ≥ 30 — findings are reasonably reliable.
    High,
    /// 10 ≤ n < 30 — interpret with caution.
    Medium,
    /// n < 10 — insufficient data; treat as directional only.
    Low,
}

impl ConfidenceLevel {
    pub(crate) fn from_n(n: usize) -> Self {
        if n >= MEDIUM_CONFIDENCE_THRESHOLD {
            Self::High
        } else if n >= LOW_CONFIDENCE_THRESHOLD {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

/// Failure statistics for one group within a dimension.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AttributionGroup {
    /// Dimension this group belongs to (e.g., "authorship", "review", "source").
    pub dimension: String,
    /// Label identifying this group within the dimension (e.g., "agent_only", "peer_reviewed").
    pub group_label: String,
    /// Number of decisions in this group.
    pub total: usize,
    /// Decisions in this group where `held_up == false`.
    pub failed: usize,
    /// `failed / total` or 0.0 when total = 0.
    pub failure_rate: f64,
    /// `failure_rate − baseline_failure_rate` — positive means worse than average.
    pub effect_vs_baseline: f64,
    /// Honest confidence flag based on sample size.
    pub confidence: ConfidenceLevel,
}

/// A notable pattern extracted from the attribution breakdown.
///
/// Sorted by `|effect_size|` descending: the first finding has the strongest signal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AttributionFinding {
    pub dimension: String,
    pub group_label: String,
    /// Human-readable summary of the finding.
    pub finding: String,
    /// `failure_rate − baseline`. Positive = higher failure rate than corpus average.
    pub effect_size: f64,
    pub confidence: ConfidenceLevel,
    pub sample_size: usize,
}

/// The complete failure-mode attribution report.
///
/// All breakdowns are AGGREGATE patterns — never per-person rankings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FailureModeReport {
    pub corpus_stats: CorpusStats,
    /// Failure rates broken down by who authored the decision.
    pub by_authorship: Vec<AttributionGroup>,
    /// Failure rates broken down by how substantively the decision was reviewed.
    pub by_review: Vec<AttributionGroup>,
    /// Failure rates for decisions an agent made for itself, split by whether a human had
    /// delegated the scope (`delegated`) or not (`agent_alone`) — the two cases that
    /// `by_authorship` and `by_review` cannot tell apart (hivemind-zdsh.6). Decisions an
    /// agent did not decide for itself belong to neither group and do not appear here.
    pub by_delegation: Vec<AttributionGroup>,
    /// Failure rates broken down by which source system captured the decision.
    pub by_source: Vec<AttributionGroup>,
    /// Failure rates broken down by context richness (evidence/options/rationale buckets).
    pub by_context_richness: Vec<AttributionGroup>,
    /// Failure rates broken down by the conditions the caller supplied, one group per
    /// (dimension, label) it reported. Empty when the caller supplied none.
    pub by_condition: Vec<AttributionGroup>,
    /// Top findings sorted by absolute effect size. Groups with n < `min_sample_size` excluded.
    pub findings: Vec<AttributionFinding>,
}

/// Request parameters for the failure attribution analysis.
#[derive(Clone, Debug, Default)]
pub struct FailureAttributionRequest {
    /// Minimum ledger offset to include (inclusive). Filters by the decision's `event_origin`.
    pub since_event_origin: Option<i64>,
    /// Minimum group size required for a group to appear in `findings`.
    /// Groups smaller than this are still included in breakdowns but omitted from findings.
    /// Default: 3.
    pub min_sample_size: usize,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Compute the failure-mode attribution report over the decision corpus.
pub fn get_failure_attribution(
    graph: &impl GraphView,
    request: &FailureAttributionRequest,
) -> Result<QueryResponse<FailureModeReport>> {
    get_failure_attribution_with(graph, request, |_| Ok(Vec::new()))
}

/// [`get_failure_attribution`] that also groups failures by conditions the caller derives.
/// `conditions_of` is asked once for each analysed decision (at most `MAX_QUERY_RESULTS`), and
/// its answer fills `by_condition`. A failed lookup fails the whole report rather than leaving
/// that decision out of its groups unnoticed.
pub fn get_failure_attribution_with(
    graph: &impl GraphView,
    request: &FailureAttributionRequest,
    conditions_of: impl Fn(&str) -> Result<Vec<DecisionCondition>>,
) -> Result<QueryResponse<FailureModeReport>> {
    let started = Instant::now();
    let min_sample = if request.min_sample_size == 0 {
        3
    } else {
        request.min_sample_size
    };

    // 1. Load all outcomes.
    let outcome_req = DecisionQualityCandidatesRequest {
        since_event_origin: request.since_event_origin,
        limit: 0, // 0 → MAX_QUERY_RESULTS
        cursor: None,
        only_with_signals: false,
    };
    let outcomes = get_decision_quality_candidates(graph, &outcome_req)?.data;

    // 2. Load all contexts.
    let context_req = DecisionContextRequest {
        since_event_origin: request.since_event_origin,
        limit: 0,
        cursor: None,
    };
    let contexts = get_decision_context_candidates(graph, &context_req)?.data;

    // 3. Join by decision_id → Vec<(outcome, context)>.
    let context_map: HashMap<String, DecisionContext> = contexts
        .into_iter()
        .map(|c| (c.decision_id.clone(), c)) // ubs:ignore: key must be owned; value moves c
        .collect();

    // Pairs we can analyse (only decisions that appear in both outcome and context sets).
    let pairs: Vec<_> = outcomes
        .into_iter()
        .filter_map(|o| context_map.get(&o.decision_id).cloned().map(|c| (o, c)))
        .collect();

    // Enforce MAX_QUERY_RESULTS on the joined corpus to prevent runaway allocations.
    let pairs = if pairs.len() > MAX_QUERY_RESULTS {
        pairs.into_iter().take(MAX_QUERY_RESULTS).collect()
    } else {
        pairs
    };

    // 4. Corpus stats.
    let total_decisions = pairs.len();
    let failed_decisions = pairs.iter().filter(|(o, _)| !o.held_up).count();
    let baseline = if total_decisions == 0 {
        0.0
    } else {
        failed_decisions as f64 / total_decisions as f64
    };

    let signal_breakdown = SignalBreakdown {
        superseded_count: pairs.iter().filter(|(o, _)| o.superseded).count(),
        stale_premises_count: pairs.iter().filter(|(o, _)| o.stale_premises).count(),
        contested_count: pairs.iter().filter(|(o, _)| o.contested).count(),
    };

    let corpus_stats = CorpusStats {
        total_decisions,
        failed_decisions,
        baseline_failure_rate: baseline,
        signal_breakdown,
    };

    // 5. Dimensional breakdowns.
    let by_authorship = breakdown_by(&pairs, baseline, "authorship", |_, c| {
        Some(authorship_label(c.authorship))
    });
    let by_review = breakdown_by(&pairs, baseline, "review", |_, c| {
        Some(review_label(c.review))
    });
    let by_delegation = breakdown_by(&pairs, baseline, "delegation", |_, c| delegation_label(c));
    let by_source = breakdown_by(&pairs, baseline, "source", |_, c| Some(c.source.clone())); // ubs:ignore: owned String required by F: Fn(…) -> Option<String>
    let by_context_richness = context_richness_breakdown(&pairs, baseline);
    let conditions = pairs
        .iter()
        .map(|(outcome, _)| conditions_of(&outcome.decision_id))
        .collect::<Result<Vec<_>>>()?;
    let by_condition = condition_breakdown(&pairs, &conditions, baseline);

    // 6. Findings: top patterns by |effect_size|, filtered by min_sample_size.
    let all_groups: Vec<&AttributionGroup> = by_authorship
        .iter()
        .chain(by_review.iter())
        .chain(by_delegation.iter())
        .chain(by_source.iter())
        .chain(by_context_richness.iter())
        .chain(by_condition.iter())
        .collect();

    let mut findings: Vec<AttributionFinding> = all_groups
        .into_iter()
        .filter(|g| g.total >= min_sample)
        .map(|g| AttributionFinding {
            dimension: g.dimension.clone(), // ubs:ignore: g is &&AttributionGroup; owned field required
            group_label: g.group_label.clone(), // ubs:ignore: same — owned field required
            finding: describe_finding(g, baseline),
            effect_size: g.effect_vs_baseline,
            confidence: g.confidence,
            sample_size: g.total,
        })
        .collect();

    // Sort by absolute effect size descending.
    findings.sort_by(|a, b| {
        b.effect_size
            .abs()
            .partial_cmp(&a.effect_size.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let result_count = total_decisions;
    let report = FailureModeReport {
        corpus_stats,
        by_authorship,
        by_review,
        by_delegation,
        by_source,
        by_context_richness,
        by_condition,
        findings,
    };

    Ok(QueryResponse {
        result_count,
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: report,
    })
}

// ---------------------------------------------------------------------------
// Dimensional breakdown helpers
// ---------------------------------------------------------------------------

fn breakdown_by<F>(
    pairs: &[(
        super::outcome::DecisionOutcome,
        super::context::DecisionContext,
    )],
    baseline: f64,
    dimension: &str,
    label_fn: F,
) -> Vec<AttributionGroup>
where
    F: Fn(&super::outcome::DecisionOutcome, &super::context::DecisionContext) -> Option<String>,
{
    // Accumulate (total, failed) per label. A `None` label leaves the decision out of this
    // dimension (it doesn't belong to any of its groups).
    let mut buckets: HashMap<String, (usize, usize)> = HashMap::new();
    for (outcome, context) in pairs {
        let Some(label) = label_fn(outcome, context) else {
            continue;
        };
        let entry = buckets.entry(label).or_insert((0, 0));
        entry.0 += 1;
        if !outcome.held_up {
            entry.1 += 1;
        }
    }

    // Convert to groups, sorted by label for stable output.
    let mut labels: Vec<_> = buckets.keys().cloned().collect();
    labels.sort();

    labels
        .into_iter()
        .map(|label| {
            let (total, failed) = buckets.get(&label).copied().unwrap_or((0, 0));
            attribution_group(dimension, label, total, failed, baseline)
        })
        .collect()
}

/// The group for `total` decisions of which `failed` did not hold up, against `baseline`.
fn attribution_group(
    dimension: &str,
    group_label: String,
    total: usize,
    failed: usize,
    baseline: f64,
) -> AttributionGroup {
    let failure_rate = if total == 0 {
        0.0
    } else {
        failed as f64 / total as f64
    };
    AttributionGroup {
        dimension: dimension.to_owned(),
        group_label,
        total,
        failed,
        failure_rate,
        effect_vs_baseline: failure_rate - baseline,
        confidence: ConfidenceLevel::from_n(total),
    }
}

/// Breakdown by the conditions the caller supplied: `conditions[i]` belongs to `pairs[i]`.
/// One group per (dimension, label), sorted by both for stable output.
fn condition_breakdown(
    pairs: &[(
        super::outcome::DecisionOutcome,
        super::context::DecisionContext,
    )],
    conditions: &[Vec<DecisionCondition>],
    baseline: f64,
) -> Vec<AttributionGroup> {
    let mut buckets: BTreeMap<(&str, &str), (usize, usize)> = BTreeMap::new();
    for ((outcome, _), decision_conditions) in pairs.iter().zip(conditions) {
        for condition in decision_conditions {
            let entry = buckets
                .entry((condition.dimension.as_str(), condition.label.as_str()))
                .or_insert((0, 0));
            entry.0 += 1;
            if !outcome.held_up {
                entry.1 += 1;
            }
        }
    }

    buckets
        .into_iter()
        .map(|((dimension, label), (total, failed))| {
            attribution_group(dimension, label.to_owned(), total, failed, baseline)
        })
        .collect()
}

/// Context richness breakdown: buckets decisions by whether they have thin evidence/options/rationale.
fn context_richness_breakdown(
    pairs: &[(
        super::outcome::DecisionOutcome,
        super::context::DecisionContext,
    )],
    baseline: f64,
) -> Vec<AttributionGroup> {
    let dimension = "context_richness";

    // Three orthogonal richness flags as group labels. Static str keys avoid per-iteration allocs.
    let mut buckets: HashMap<&'static str, (usize, usize)> = HashMap::new();

    for (outcome, context) in pairs {
        // Evidence present / absent.
        let evidence_label: &'static str = if context.evidence_count == 0 {
            "no_evidence"
        } else {
            "has_evidence"
        };
        let e = buckets.entry(evidence_label).or_insert((0, 0));
        e.0 += 1;
        if !outcome.held_up {
            e.1 += 1;
        }

        // Options present / absent.
        let options_label: &'static str = if context.options_count == 0 {
            "no_options"
        } else {
            "has_options"
        };
        let o = buckets.entry(options_label).or_insert((0, 0));
        o.0 += 1;
        if !outcome.held_up {
            o.1 += 1;
        }

        // Rationale thin / rich: threshold at 100 chars as a proxy.
        let rationale_label: &'static str = if context.rationale_chars < 100 {
            "thin_rationale"
        } else {
            "rich_rationale"
        };
        let r = buckets.entry(rationale_label).or_insert((0, 0));
        r.0 += 1;
        if !outcome.held_up {
            r.1 += 1;
        }
    }

    let mut labels: Vec<&'static str> = buckets.keys().copied().collect();
    labels.sort_unstable();

    labels
        .into_iter()
        .map(|label| {
            let (total, failed) = buckets.get(label).copied().unwrap_or((0, 0));
            attribution_group(dimension, label.to_owned(), total, failed, baseline)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------------

fn authorship_label(shape: AuthorshipShape) -> String {
    match shape {
        AuthorshipShape::HumanAuthored => "human_authored".to_owned(),
        AuthorshipShape::AgentProposedHumanAccepted => "agent_proposed_human_accepted".to_owned(),
        AuthorshipShape::AgentOnly => "agent_only".to_owned(),
        AuthorshipShape::Unknown => "unknown".to_owned(),
    }
}

/// Which side of the delegation line an agent's decision falls on, or `None` when the
/// decision is not an agent deciding for itself (hivemind-zdsh.6). The marker alone puts a
/// decision in `delegated` (it can only be recorded on an agent's self-acceptance, and stays
/// after a human later reviews it); an agent-authored, self-accepted decision without one is
/// `agent_alone`. An unreviewed agent proposal is neither: nobody has decided it yet.
fn delegation_label(context: &DecisionContext) -> Option<String> {
    if context.delegated_by.is_some() {
        Some("delegated".to_owned())
    } else if context.authorship == AuthorshipShape::AgentOnly
        && context.review == ReviewShape::SelfAccepted
    {
        Some("agent_alone".to_owned())
    } else {
        None
    }
}

fn review_label(shape: ReviewShape) -> String {
    match shape {
        ReviewShape::Unreviewed => "unreviewed".to_owned(),
        ReviewShape::SelfAccepted => "self_accepted".to_owned(),
        ReviewShape::PeerReviewed => "peer_reviewed".to_owned(),
        ReviewShape::Disputed => "disputed".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Finding description generator
// ---------------------------------------------------------------------------

fn describe_finding(group: &AttributionGroup, baseline: f64) -> String {
    let direction = if group.effect_vs_baseline > 0.0 {
        "higher"
    } else {
        "lower"
    };
    let pct_group = (group.failure_rate * 100.0).round() as i64; // ubs:ignore: bounded 0-100 percentage
    let pct_baseline = (baseline * 100.0).round() as i64; // ubs:ignore: bounded 0-100 percentage
    let abs_effect = (group.effect_vs_baseline.abs() * 100.0).round() as i64; // ubs:ignore: bounded percentage difference
    let confidence_note = match group.confidence {
        ConfidenceLevel::High => "".to_owned(),
        ConfidenceLevel::Medium => " (medium confidence — interpret with caution)".to_owned(),
        ConfidenceLevel::Low => " (low confidence — n too small for reliable inference)".to_owned(),
    };
    format!(
        "{}/{}: {pct_group}% failure rate vs {pct_baseline}% baseline \
         — {abs_effect}pp {direction} than average (n={n}){confidence_note}",
        group.dimension,
        group.group_label,
        n = group.total,
    )
}
