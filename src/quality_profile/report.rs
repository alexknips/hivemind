//! What `score_decision` and `scan_decision_quality` return: the one core behind the stdio MCP
//! server, the HTTP MCP endpoint and the CLI. Each surface parses its own arguments, calls the
//! two functions here and serializes the response it gets back, so the three cannot drift.
//!
//! # `score_decision`
//! The decision's profile ([`QualityProfile`]): the seven dimensions, each assessed (a level, the
//! reasons and the node ids behind it) or not assessed (and why), the attention lines, and a
//! [`Provenance`] naming who authored the decision and whether a human has looked at it. There is
//! no composite number, no tier and no score anywhere in the response: no number in it says how
//! good a decision is.
//!
//! # `scan_decision_quality`
//! One page of [attention findings](super::findings): the decisions that need a look now, each
//! with the reason in words, the nodes it rests on and the dimensions it bears on, every one with
//! its level and reasons. A finding is not a grade and the page is not a ranking.
//!
//! Which dimensions a kind of finding bears on:
//!
//! | Kind | Dimensions | Why |
//! | --- | --- | --- |
//! | `bet_past_check_date` | Information, Calibration | the bet counts as information on record; confidence was declared over it |
//! | `premise_superseded`, `premise_rejected` | Information, Reasoning | the prior decision counts as information and as stated reasoning |
//! | `assumption_refuted` | Information | the assumption counts as information on record |
//! | `bet_failed` | Information, Calibration | as for a bet past its check date |
//! | `evidence_not_rechecked` | Information | the evidence is what Information counts |
//!
//! # `get_suggestions`
//! The same page of findings, without the ones someone has already acknowledged. A finding is
//! acknowledged by its `finding_id`, and that id changes when the finding's basis does, so a
//! finding whose premise moved on since it was acknowledged is a new finding and is shown again.
//! `exclude_acknowledged` (true unless a caller says otherwise) is "what is new since I last
//! looked"; false is every finding, which is what `scan_decision_quality` always returns.
//! Acknowledged findings are left out before the page is cut ([`AttentionRequest::excluded`]), so
//! a page is full whenever that many findings remain and `truncated` is exact. What counts as
//! acknowledged is read by [`acknowledged_finding_ids`]: no event records an acknowledgement yet,
//! so today both settings return the same findings.
//!
//! # Cost
//! A scan page costs what [`attention_findings_at`] costs (a fixed number of bulk reads plus at
//! most one anchored read per distinct superseding decision on the page) plus one profile read
//! per distinct decision on the page: never more than `limit` of each, however many decisions the
//! graph holds. A profile read is a handful of anchored lookups, not a scan.
//! A suggestions page adds one read of the acknowledged ids and, for each acknowledged finding it
//! steps over, what wording that finding costs (see [`attention_findings_at`]).
//!
//! # Placement
//! Layer 3, with the rest of the profile. Nothing in `queries/` or `commands/` imports it; the
//! transports import it. No model, no network, no write.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::projector::GraphView;
use crate::queries::{
    get_decision_context, AuthorshipShape, DecisionContext, QueryResponse, ReviewShape,
};
use crate::Result;

use super::{
    attention_findings_at, quality_profile_of, Assessment, AttentionConfig, AttentionFinding,
    AttentionRequest, Dimension, FindingKind, QualityProfile, DEFAULT_EVIDENCE_WINDOW_DAYS,
};

/// Findings per page on every surface when the caller does not say.
pub const SCAN_DEFAULT_LIMIT: usize = 25;

/// The provenance line for a decision no human has looked at.
pub const NOT_REVIEWED_BY_A_HUMAN: &str = "not yet reviewed by a human";

// ---------------------------------------------------------------------------
// score_decision
// ---------------------------------------------------------------------------

/// Who authored a decision and whether a human has looked at it: facts about how the decision
/// came to be recorded, shown beside its quality and never folded into it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub authorship: AuthorshipShape,
    pub review: ReviewShape,
    /// [`NOT_REVIEWED_BY_A_HUMAN`], present exactly when an agent authored the decision and no
    /// human accepted it. A decision someone has rejected is not marked: the review shape does
    /// not say whether that someone was a human, and the line makes a negative claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<&'static str>,
}

impl Provenance {
    /// The provenance the decision's authorship and review shapes state.
    pub fn of(context: &DecisionContext) -> Self {
        Self::from_shapes(context.authorship, context.review)
    }

    fn from_shapes(authorship: AuthorshipShape, review: ReviewShape) -> Self {
        let unreviewed =
            authorship == AuthorshipShape::AgentOnly && review != ReviewShape::Disputed;
        Self {
            authorship,
            review,
            line: unreviewed.then_some(NOT_REVIEWED_BY_A_HUMAN),
        }
    }
}

/// The answer to `score_decision`: the profile, and the provenance beside it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScoreReport {
    #[serde(flatten)]
    pub profile: QualityProfile,
    pub provenance: Provenance,
}

/// The profile and provenance of one decision; `data` is `None` when the decision does not exist.
pub fn score_decision(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<ScoreReport>>> {
    // ubs:ignore: Instant measures response latency only; it does not generate secrets.
    let started = Instant::now();
    let data = match quality_profile_of(graph, decision_id)? {
        None => None,
        Some(profile) => {
            let provenance = match get_decision_context(graph, decision_id)?.data {
                Some(context) => Provenance::of(&context),
                // A decision with no recorded proposer: the shape says so.
                None => Provenance::from_shapes(AuthorshipShape::Unknown, ReviewShape::Unreviewed),
            };
            Some(ScoreReport {
                profile,
                provenance,
            })
        }
    };
    Ok(QueryResponse {
        result_count: usize::from(data.is_some()),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

// ---------------------------------------------------------------------------
// scan_decision_quality
// ---------------------------------------------------------------------------

/// Which findings to page through, as a surface states it.
#[derive(Clone, Debug, Default)]
pub struct ScanRequest {
    /// Only these kinds; every kind when empty.
    pub kinds: Vec<FindingKind>,
    /// Findings per page; the attention default when 0, and never more than `MAX_QUERY_RESULTS`.
    pub limit: usize,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// Days of silence after which evidence counts as not re-checked; the default when `None`.
    pub evidence_window_days: Option<u32>,
}

/// The kinds a caller named, or the error every surface shows for a name that is no kind.
pub fn parse_kinds<S: AsRef<str>>(names: &[S]) -> std::result::Result<Vec<FindingKind>, String> {
    names
        .iter()
        .map(|name| {
            let name = name.as_ref();
            FindingKind::parse(name).ok_or_else(|| {
                let valid: Vec<&str> = FindingKind::ALL.iter().map(|kind| kind.as_str()).collect();
                format!(
                    "unknown finding kind `{name}`; expected one of {}",
                    valid.join(", ")
                )
            })
        })
        .collect()
}

/// One dimension a finding bears on, with its level and reasons (or why it is not assessed).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DimensionLine {
    pub dimension: Dimension,
    #[serde(flatten)]
    pub assessment: Assessment,
}

/// An attention finding and the dimensions of its decision it bears on.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanFinding {
    #[serde(flatten)]
    pub finding: AttentionFinding,
    pub dimensions: Vec<DimensionLine>,
}

/// One page of findings, as `scan_decision_quality` returns it. Whether more follow is the
/// response's `truncated`; `next_cursor` is present exactly then.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanReport {
    /// The clock the page was derived at: what "past its check date" was measured against.
    pub as_of: DateTime<Utc>,
    /// The evidence window the page was derived with.
    pub evidence_window_days: u32,
    pub findings: Vec<ScanFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The dimensions a finding of `kind` bears on (the table in the module docs).
pub const fn dimensions_for(kind: FindingKind) -> &'static [Dimension] {
    match kind {
        FindingKind::BetPastCheckDate | FindingKind::BetFailed => {
            &[Dimension::Information, Dimension::Calibration]
        }
        FindingKind::PremiseSuperseded | FindingKind::PremiseRejected => {
            &[Dimension::Information, Dimension::Reasoning]
        }
        FindingKind::AssumptionRefuted | FindingKind::EvidenceNotRechecked => {
            &[Dimension::Information]
        }
    }
}

/// One page of the findings as of now. Reads the clock once; see [`scan_decision_quality_at`].
pub fn scan_decision_quality(
    graph: &impl GraphView,
    request: &ScanRequest,
) -> Result<QueryResponse<ScanReport>> {
    scan_decision_quality_at(graph, request, Utc::now())
}

/// One page of the findings as of `now`, each with the dimensions it bears on. Read-only.
pub fn scan_decision_quality_at(
    graph: &impl GraphView,
    request: &ScanRequest,
    now: DateTime<Utc>,
) -> Result<QueryResponse<ScanReport>> {
    scan_page_at(graph, request, BTreeSet::new(), now)
}

/// Which suggestions to page through: the findings of a [`ScanRequest`], with or without the ones
/// that have been acknowledged.
#[derive(Clone, Debug)]
pub struct SuggestionsRequest {
    pub scan: ScanRequest,
    /// Leave out findings someone has acknowledged (matched by `finding_id`); every finding when
    /// false.
    pub exclude_acknowledged: bool,
}

impl Default for SuggestionsRequest {
    fn default() -> Self {
        Self {
            scan: ScanRequest::default(),
            exclude_acknowledged: true,
        }
    }
}

/// One page of suggestions as of now. Reads the clock once; see [`get_suggestions_at`].
pub fn get_suggestions(
    graph: &impl GraphView,
    request: &SuggestionsRequest,
) -> Result<QueryResponse<ScanReport>> {
    get_suggestions_at(graph, request, Utc::now())
}

/// One page of the findings as of `now`, without the acknowledged ones unless the request asks for
/// them. Read-only.
pub fn get_suggestions_at(
    graph: &impl GraphView,
    request: &SuggestionsRequest,
    now: DateTime<Utc>,
) -> Result<QueryResponse<ScanReport>> {
    let excluded = if request.exclude_acknowledged {
        acknowledged_finding_ids(graph)?
    } else {
        BTreeSet::new()
    };
    scan_page_at(graph, &request.scan, excluded, now)
}

/// The `finding_id`s someone has acknowledged. No event records an acknowledgement yet, so nothing
/// is acknowledged and this is empty; the event and the read of it belong together.
pub fn acknowledged_finding_ids(_graph: &impl GraphView) -> Result<BTreeSet<String>> {
    Ok(BTreeSet::new())
}

/// One page of findings without those in `excluded`, each with the dimensions it bears on.
fn scan_page_at(
    graph: &impl GraphView,
    request: &ScanRequest,
    excluded: BTreeSet<String>,
    now: DateTime<Utc>,
) -> Result<QueryResponse<ScanReport>> {
    // ubs:ignore: Instant measures response latency only; it does not generate secrets.
    let started = Instant::now();
    let config = AttentionConfig {
        evidence_window_days: request
            .evidence_window_days
            .unwrap_or(DEFAULT_EVIDENCE_WINDOW_DAYS),
    };
    let page = attention_findings_at(
        graph,
        &AttentionRequest {
            kinds: request.kinds.clone(),
            limit: request.limit,
            cursor: request.cursor.clone(),
            excluded,
        },
        &config,
        now,
    )?;

    // One profile per distinct decision on the page, however many findings name it.
    let distinct: BTreeSet<&str> = page
        .findings
        .iter()
        .map(|finding| finding.decision_id.as_str())
        .collect();
    let profiles = distinct
        .into_iter()
        .map(|id| Ok((id.to_owned(), quality_profile_of(graph, id)?)))
        .collect::<Result<BTreeMap<String, Option<QualityProfile>>>>()?;
    let mut findings = Vec::with_capacity(page.findings.len());
    for finding in page.findings {
        let dimensions = profiles
            .get(&finding.decision_id)
            .and_then(Option::as_ref)
            .map(|profile| dimension_lines(profile, dimensions_for(finding.kind)))
            .unwrap_or_default();
        findings.push(ScanFinding {
            finding,
            dimensions,
        });
    }

    Ok(QueryResponse {
        result_count: findings.len(),
        truncated: page.truncated,
        latency_ms: started.elapsed().as_millis(),
        data: ScanReport {
            as_of: page.as_of,
            evidence_window_days: page.evidence_window_days,
            findings,
            next_cursor: page.next_cursor,
        },
    })
}

fn dimension_lines(profile: &QualityProfile, dimensions: &[Dimension]) -> Vec<DimensionLine> {
    dimensions
        .iter()
        .map(|dimension| DimensionLine {
            dimension: *dimension,
            assessment: profile.assessment(*dimension).clone(),
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests;
