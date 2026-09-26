//! Attention findings: which decisions need a look now, derived from what the graph already
//! states. A finding is not a grade: it names a decision, says in words what changed or lapsed,
//! and lists the nodes it rests on. There is no score, no tier and no ranking; the order is by id
//! so that a page can be resumed, and a consumer that wants a priority applies its own.
//!
//! # The findings
//! Six kinds, in the three lists the quality profile rolls up into. Each needs the decision to
//! *stand* (not superseded and not rejected: a decision that has been replaced needs no look) and
//! looks one hop: a decision is flagged for its own premises. Carrying a change on to the decisions
//! that rest on the flagged one is the re-examine walk, which lands with `hivemind-ottw`.
//!
//! | Kind | List | The decision is flagged when | Basis time (`basis_at`) |
//! | --- | --- | --- | --- |
//! | `bet_past_check_date` | bets past their check date | it rests on a declared bet whose check date has passed and nothing supports or refutes it | the check date |
//! | `premise_superseded` | premise changed | a decision it follows from was superseded | when the earliest superseding decision was made |
//! | `premise_rejected` | premise changed | a decision it follows from was rejected (and nobody accepted it: a contested premise is a disagreement, shown as such) | none: the graph does not record when |
//! | `assumption_refuted` | premise changed | an assumption it rests on was refuted | when the earliest refuting evidence was recorded |
//! | `bet_failed` | premise changed | a bet it rests on was refuted | when the earliest refuting evidence was recorded |
//! | `evidence_not_rechecked` | evidence not re-checked | the newest evidence linked to it was recorded more than [`AttentionConfig::evidence_window_days`] ago | when that evidence was recorded |
//!
//! "Re-checked" means newer evidence is linked to the decision, whether it was attached at
//! capture or afterwards: the graph has no notion of one observation re-checking another, so the
//! finding is about the decision's newest evidence, not each item. A decision with no evidence
//! linked, or none whose record states a time, is never flagged here (its Information floor
//! says so). "High-leverage premises not re-examined" needs the re-examine state and is not
//! built here.
//!
//! # Identity
//! `finding_id` is a hash of the kind, the node ids and the basis time, so a consumer can dedupe
//! across scans. The same graph gives the same ids on every run; an id changes when the basis
//! does (newer evidence is linked, another decision supersedes the premise), and only then: it
//! does not depend on the clock.
//!
//! # Paging and cost
//! A page holds at most `limit` findings (never more than `MAX_QUERY_RESULTS`), in order of
//! `(decision id, kind, subject id)`; when there are more, `truncated` is true and `next_cursor`
//! resumes after the last one returned. The cursor is a position in that order, not an offset, so
//! findings that appear or vanish between pages never make a consumer skip or repeat one that
//! stood throughout.
//!
//! A request may list findings to leave out ([`AttentionRequest::excluded`], by `finding_id`).
//! They are left out before the page is cut, not after: the page is filled from the findings that
//! remain, so a consumer that has dealt with a long run of them still gets a full page, and
//! `truncated` is true only when another finding that was not left out follows.
//!
//! A page issues [`GROUNDING_FACT_READS`](crate::queries::GROUNDING_FACT_READS) bulk reads
//! however many decisions there are, plus one anchored read per distinct superseding decision on
//! the page (to state when it was made), so at most `limit` more. When findings are left out, the
//! same anchored read is made for each one stepped over on the way to a full page: the extra cost
//! grows with the findings left out that sort before the end of the page, and stops there. The
//! rows those bulk reads return grow with the grounding links, hypotheses and evidence, not with
//! decisions times a per-decision cost. Nothing walks the premise graph: a premise link is read
//! once, so a `FOLLOWS_FROM` cycle costs nothing extra and cannot loop.
//!
//! # Placement
//! Layer 3, with the rest of the profile. It reads the graph through `queries::get_grounding_facts`
//! (pure Layer-2 reads) and nothing in `queries/` or `commands/` imports it. No model, no
//! network, no write.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::QueryError;
use crate::events::HypothesisKind;
use crate::projector::GraphView;
use crate::queries::{
    get_decision_times, get_grounding_facts, DecisionStatus, GroundingFacts, HypothesisRecord,
    HypothesisStatus, MAX_QUERY_RESULTS,
};
use crate::Result;

/// How long evidence may go without a newer item before it is flagged, when nobody says
/// otherwise: one quarter.
pub const DEFAULT_EVIDENCE_WINDOW_DAYS: u32 = 90;

/// Findings per page when the request does not say.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// What a finding is about. The variant order is the order findings of one decision come in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A declared bet's check date has passed and nothing supports or refutes it.
    BetPastCheckDate,
    /// A decision it follows from was superseded.
    PremiseSuperseded,
    /// A decision it follows from was rejected.
    PremiseRejected,
    /// An assumption it rests on was refuted.
    AssumptionRefuted,
    /// A bet it rests on was refuted.
    BetFailed,
    /// The newest evidence it rests on is older than the window.
    EvidenceNotRechecked,
}

impl FindingKind {
    pub const ALL: [Self; 6] = [
        Self::BetPastCheckDate,
        Self::PremiseSuperseded,
        Self::PremiseRejected,
        Self::AssumptionRefuted,
        Self::BetFailed,
        Self::EvidenceNotRechecked,
    ];

    /// The wire name, also what the finding id is hashed from.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BetPastCheckDate => "bet_past_check_date",
            Self::PremiseSuperseded => "premise_superseded",
            Self::PremiseRejected => "premise_rejected",
            Self::AssumptionRefuted => "assumption_refuted",
            Self::BetFailed => "bet_failed",
            Self::EvidenceNotRechecked => "evidence_not_rechecked",
        }
    }

    /// The kind a wire name spells, or `None` for a name that is no kind.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

/// The definitions the findings depend on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttentionConfig {
    /// A decision is flagged for `evidence_not_rechecked` when the newest evidence linked to it
    /// was recorded more than this many days ago. Default [`DEFAULT_EVIDENCE_WINDOW_DAYS`].
    pub evidence_window_days: u32,
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            evidence_window_days: DEFAULT_EVIDENCE_WINDOW_DAYS,
        }
    }
}

/// Which findings to page through.
#[derive(Clone, Debug, Default)]
pub struct AttentionRequest {
    /// Only these kinds; every kind when empty.
    pub kinds: Vec<FindingKind>,
    /// Findings per page; [`DEFAULT_PAGE_SIZE`] when 0, and never more than `MAX_QUERY_RESULTS`.
    pub limit: usize,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// Findings to leave out, by `finding_id`: a page is filled from the findings that are not
    /// listed here, so it holds `limit` of them whenever that many remain, and `truncated` says
    /// whether another one that is not listed follows. Nothing is left out when empty.
    pub excluded: BTreeSet<String>,
}

/// One decision that needs a look, and why.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AttentionFinding {
    /// Stable across scans: a hash of the kind, the node ids and the basis time.
    pub finding_id: String,
    pub kind: FindingKind,
    /// The decision that needs a look.
    pub decision_id: String,
    /// The moment the finding rests on (see the table in the module docs); absent when the graph
    /// does not record one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis_at: Option<DateTime<Utc>>,
    /// Every node the finding rests on, the decision included: sorted and distinct.
    pub node_ids: Vec<String>,
    /// What changed or lapsed, in words.
    pub reason: String,
}

/// One page of findings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AttentionPage {
    /// The clock the page was derived at: what "past its check date" was measured against.
    pub as_of: DateTime<Utc>,
    /// The evidence window the page was derived with.
    pub evidence_window_days: u32,
    pub findings: Vec<AttentionFinding>,
    /// More findings follow. A page that is cut short never looks complete.
    pub truncated: bool,
    /// Where the next page resumes; present exactly when `truncated`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One page of the findings as of now. Reads the clock once; see [`attention_findings_at`].
pub fn attention_findings(
    graph: &impl GraphView,
    request: &AttentionRequest,
    config: &AttentionConfig,
) -> Result<AttentionPage> {
    attention_findings_at(graph, request, config, Utc::now())
}

/// One page of the findings as of `now`. Read-only.
pub fn attention_findings_at(
    graph: &impl GraphView,
    request: &AttentionRequest,
    config: &AttentionConfig,
    now: DateTime<Utc>,
) -> Result<AttentionPage> {
    let after = request.cursor.as_deref().map(parse_cursor).transpose()?;
    let limit = if request.limit == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        request.limit.min(MAX_QUERY_RESULTS)
    };
    let kinds: BTreeSet<FindingKind> = if request.kinds.is_empty() {
        FindingKind::ALL.into_iter().collect()
    } else {
        request.kinds.iter().copied().collect()
    };

    let facts = get_grounding_facts(graph)?;
    let mut candidates = candidates(&facts, &kinds, config, now);
    candidates.sort_unstable_by_key(Candidate::key);

    let start = after.as_ref().map_or(0, |after| {
        let after = (
            after.decision_id.as_str(),
            after.kind,
            after.subject_id.as_str(),
        );
        candidates.partition_point(|candidate| candidate.key() <= after)
    });
    let rest = candidates.get(start..).unwrap_or_default();
    let (findings, next_cursor) = take_page(graph, rest, limit, &request.excluded, config)?;

    Ok(AttentionPage {
        as_of: now,
        evidence_window_days: config.evidence_window_days,
        findings,
        truncated: next_cursor.is_some(),
        next_cursor,
    })
}

/// The first `limit` findings of `rest` that are not `excluded`, in order, and the cursor that
/// resumes after the last of them when another finding follows.
///
/// A candidate is worded, and so given its id, only when it is reached; the superseding decisions
/// it needs dated are read then, one anchored read per distinct id however many candidates share
/// it. With nothing excluded that is the page and no more: the candidate after it is known to be
/// a finding without being worded, so a page costs at most `limit` anchored reads. With findings
/// excluded, what a page costs grows with the excluded findings it has to step over.
fn take_page(
    graph: &impl GraphView,
    rest: &[Candidate<'_>],
    limit: usize,
    excluded: &BTreeSet<String>,
    config: &AttentionConfig,
) -> Result<(Vec<AttentionFinding>, Option<String>)> {
    let mut times: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    let mut dated: BTreeSet<&str> = BTreeSet::new();
    let mut findings = Vec::with_capacity(limit.min(rest.len()));
    let mut last: Option<&Candidate<'_>> = None;

    for candidate in rest {
        if findings.len() == limit && excluded.is_empty() {
            return Ok((findings, last.map(cursor_of).transpose()?));
        }
        let undated: Vec<&str> = candidate
            .other_superseders()
            .filter(|by_id| dated.insert(*by_id))
            .collect();
        times.extend(get_decision_times(graph, undated)?);
        let finding = candidate.to_finding(&times, config);
        if excluded.contains(&finding.finding_id) {
            continue;
        }
        if findings.len() == limit {
            return Ok((findings, last.map(cursor_of).transpose()?));
        }
        findings.push(finding);
        last = Some(candidate);
    }
    Ok((findings, None))
}

// ---------------------------------------------------------------------------
// Candidates
// ---------------------------------------------------------------------------

/// A finding before it is worded: everything the bulk facts say, borrowed from them, so only the
/// findings on the page are ever copied.
struct Candidate<'a> {
    decision_id: &'a str,
    kind: FindingKind,
    /// What the finding is about besides the decision: the bet, hypothesis or prior decision, or
    /// the newest evidence.
    subject_id: &'a str,
    /// The node whose time is the basis, when it is not the subject: the refuting evidence.
    basis_node: Option<&'a str>,
    basis_at: Option<DateTime<Utc>>,
    /// The decisions that superseded the subject; the basis is the earliest of them, which takes
    /// an anchored read to date. Empty for every other kind.
    superseders: &'a [String],
}

impl<'a> Candidate<'a> {
    fn new(decision_id: &'a str, kind: FindingKind, subject_id: &'a str) -> Self {
        Self {
            decision_id,
            kind,
            subject_id,
            basis_node: None,
            basis_at: None,
            superseders: &[],
        }
    }

    /// The decisions that superseded the subject, other than this decision itself.
    fn other_superseders(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.superseders
            .iter()
            .map(String::as_str)
            .filter(|by_id| *by_id != self.decision_id)
    }

    /// The order findings come in, and what a cursor is a position in. Unique per finding.
    fn key(&self) -> (&'a str, FindingKind, &'a str) {
        (self.decision_id, self.kind, self.subject_id)
    }
}

fn candidates<'a>(
    facts: &'a GroundingFacts,
    kinds: &BTreeSet<FindingKind>,
    config: &AttentionConfig,
    now: DateTime<Utc>,
) -> Vec<Candidate<'a>> {
    let standing = |decision_id: &str| {
        !matches!(
            facts.standings.status_of(decision_id),
            DecisionStatus::Superseded | DecisionStatus::Rejected
        )
    };
    let mut found = Vec::new();

    for (decision_id, premise_id) in &facts.premise_links {
        let candidate = match facts.standings.status_of(premise_id) {
            DecisionStatus::Superseded => {
                let superseders = facts.standings.superseders_of(premise_id);
                // A decision that itself superseded its premise is the change, not something
                // that needs a look because of it.
                if superseders.iter().all(|by_id| by_id == decision_id) {
                    continue;
                }
                let mut candidate =
                    Candidate::new(decision_id, FindingKind::PremiseSuperseded, premise_id);
                candidate.superseders = superseders;
                candidate
            }
            DecisionStatus::Rejected => {
                Candidate::new(decision_id, FindingKind::PremiseRejected, premise_id)
            }
            _ => continue,
        };
        if kinds.contains(&candidate.kind) && standing(decision_id) {
            found.push(candidate);
        }
    }

    for (decision_id, hypothesis_id) in &facts.hypothesis_links {
        let Some(hypothesis) = facts.hypotheses.get(hypothesis_id) else {
            continue;
        };
        let Some(candidate) =
            hypothesis_candidate(facts, decision_id, hypothesis_id, hypothesis, now)
        else {
            continue;
        };
        if kinds.contains(&candidate.kind) && standing(decision_id) {
            found.push(candidate);
        }
    }

    if kinds.contains(&FindingKind::EvidenceNotRechecked) {
        found.extend(stale_evidence(facts, config, now, standing));
    }
    found
}

/// The finding a decision's link to one hypothesis gives rise to, if any: a bet past its check
/// date with nothing said either way, or a refuted assumption or bet.
fn hypothesis_candidate<'a>(
    facts: &'a GroundingFacts,
    decision_id: &'a str,
    hypothesis_id: &'a str,
    hypothesis: &'a HypothesisRecord,
    now: DateTime<Utc>,
) -> Option<Candidate<'a>> {
    match (hypothesis.status, hypothesis.kind) {
        (HypothesisStatus::Open, HypothesisKind::Bet) => {
            let check_by = hypothesis.check_by.filter(|check_by| *check_by < now)?;
            let mut candidate =
                Candidate::new(decision_id, FindingKind::BetPastCheckDate, hypothesis_id);
            candidate.basis_at = Some(check_by);
            Some(candidate)
        }
        (HypothesisStatus::Refuted, kind) => {
            let kind = match kind {
                HypothesisKind::Bet => FindingKind::BetFailed,
                HypothesisKind::Assumption => FindingKind::AssumptionRefuted,
            };
            let mut candidate = Candidate::new(decision_id, kind, hypothesis_id);
            // The first refutation is when it stopped holding; evidence with no recorded time
            // sorts after any that has one.
            let earliest = hypothesis
                .refuted_by
                .iter()
                .map(|evidence_id| {
                    let at = facts.evidence_recorded_at.get(evidence_id).copied();
                    (
                        at.unwrap_or(DateTime::<Utc>::MAX_UTC),
                        evidence_id.as_str(),
                        at,
                    )
                })
                .min();
            if let Some((_, evidence_id, at)) = earliest {
                candidate.basis_node = Some(evidence_id);
                candidate.basis_at = at;
            }
            Some(candidate)
        }
        _ => None,
    }
}

/// One candidate per standing decision whose newest evidence is older than the window.
fn stale_evidence<'a>(
    facts: &'a GroundingFacts,
    config: &AttentionConfig,
    now: DateTime<Utc>,
    standing: impl Fn(&str) -> bool,
) -> Vec<Candidate<'a>> {
    // A window that reaches before the start of time flags nothing.
    let Some(cutoff) = Duration::try_days(i64::from(config.evidence_window_days))
        .and_then(|window| now.checked_sub_signed(window))
    else {
        return Vec::new();
    };

    let mut newest: BTreeMap<&str, (DateTime<Utc>, &str)> = BTreeMap::new();
    for (decision_id, evidence_id) in &facts.evidence_links {
        let Some(recorded_at) = facts.evidence_recorded_at.get(evidence_id).copied() else {
            continue;
        };
        let latest = (recorded_at, evidence_id.as_str());
        let slot = newest.entry(decision_id.as_str()).or_insert(latest);
        if latest > *slot {
            *slot = latest;
        }
    }

    newest
        .into_iter()
        .filter(|(decision_id, (recorded_at, _))| *recorded_at < cutoff && standing(decision_id))
        .map(|(decision_id, (recorded_at, evidence_id))| {
            let mut candidate =
                Candidate::new(decision_id, FindingKind::EvidenceNotRechecked, evidence_id);
            candidate.basis_at = Some(recorded_at);
            candidate
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Wording and identity
// ---------------------------------------------------------------------------

impl Candidate<'_> {
    fn to_finding(
        &self,
        times: &BTreeMap<String, DateTime<Utc>>,
        config: &AttentionConfig,
    ) -> AttentionFinding {
        let mut basis_node = self.basis_node;
        let mut basis_at = self.basis_at;
        if self.kind == FindingKind::PremiseSuperseded {
            // The premise stopped standing when the earliest superseding decision was made;
            // one with no recorded time sorts after any that has one.
            let earliest = self
                .other_superseders()
                .map(|by_id| {
                    let at = times.get(by_id).copied();
                    (at.unwrap_or(DateTime::<Utc>::MAX_UTC), by_id, at)
                })
                .min();
            if let Some((_, by_id, at)) = earliest {
                basis_node = Some(by_id);
                basis_at = at;
            }
        }

        let node_ids: Vec<String> = [Some(self.decision_id), Some(self.subject_id), basis_node]
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(str::to_owned)
            .collect();

        let on = basis_at.map_or_else(String::new, |at| format!(" on {}", date(at)));
        let subject = self.subject_id;
        let by = basis_node.unwrap_or(subject);
        let reason = match self.kind {
            FindingKind::BetPastCheckDate => format!(
                "the bet {subject} was to be checked by {}; nothing has been recorded for or against it",
                basis_at.map_or_else(|| "its check date".to_owned(), date),
            ),
            FindingKind::PremiseSuperseded => format!(
                "the decision it follows from, {subject}, was superseded by {by}{on}"
            ),
            FindingKind::PremiseRejected => {
                format!("the decision it follows from, {subject}, was rejected")
            }
            FindingKind::AssumptionRefuted => {
                format!("the assumption it rests on, {subject}, was refuted by {by}{on}")
            }
            FindingKind::BetFailed => {
                format!("the bet it rests on, {subject}, failed: refuted by {by}{on}")
            }
            FindingKind::EvidenceNotRechecked => format!(
                "the newest evidence it rests on, {subject}, was recorded{on}, more than {} days ago",
                config.evidence_window_days,
            ),
        };

        AttentionFinding {
            finding_id: finding_id(self.kind, &node_ids, basis_at),
            kind: self.kind,
            decision_id: self.decision_id.to_owned(),
            basis_at,
            node_ids,
            reason,
        }
    }
}

fn date(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// A hash of the kind, the node ids (already sorted) and the basis time, so it is the same on
/// every run and changes only when one of them does. The time is written in one fixed form,
/// whatever precision it was recorded with.
fn finding_id(kind: FindingKind, node_ids: &[String], basis_at: Option<DateTime<Utc>>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    for node_id in node_ids {
        hasher.update([0u8]);
        hasher.update(node_id.as_bytes());
    }
    hasher.update([0u8, 0u8]);
    match basis_at {
        Some(at) => hasher.update(at.to_rfc3339_opts(SecondsFormat::Nanos, true).as_bytes()),
        None => hasher.update(b"-"),
    }
    let digest = hasher.finalize();

    let mut id = String::from("finding-");
    for byte in digest.iter().take(16) {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// A position in the order findings come in: the key of the last finding a page returned.
#[derive(Deserialize, Serialize)]
struct Cursor {
    decision_id: String,
    kind: FindingKind,
    subject_id: String,
}

fn cursor_of(last: &Candidate<'_>) -> Result<String> {
    serde_json::to_string(&Cursor {
        decision_id: last.decision_id.to_owned(),
        kind: last.kind,
        subject_id: last.subject_id.to_owned(),
    })
    .map_err(|error| QueryError::Execution(format!("cursor could not be written: {error}")).into())
}

fn parse_cursor(cursor: &str) -> Result<Cursor> {
    serde_json::from_str(cursor).map_err(|_| {
        QueryError::Execution("cursor is not one an attention scan returned".to_owned()).into()
    })
}

#[cfg(test)]
pub(crate) mod tests;
