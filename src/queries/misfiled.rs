//! Misfiled-decision detection: decisions whose topic keys name a different
//! ledger/rig than the project they are filed under (hivemind-zdsh.14, findings 7+8
//! of the 2026-09-20..22 mayor audit — "beadline polecat decisions in the
//! company ledger; topics MCP / MILESTONE-M6 / PROJECTS / GC invented per
//! capture"; the move half, hivemind-zywz).
//!
//! # What this is, and is not
//!
//! Deterministic and precision-biased: a decision is flagged only when one of its own
//! (already-normalized) `topic_keys` exactly matches a caller-supplied
//! "foreign" key — no fuzzy matching, no LLM, no inference about intent.
//! HiveMind does not decide on its own what is foreign; the caller (a human
//! reviewer, or a script that knows the city's rig names) supplies the list.
//!
//! Each candidate says which project it is filed under now. The report can be
//! scoped to one project (`project`: only decisions filed exactly there, so a
//! second run after the moves is clean) and can name where the flagged
//! decisions belong (`move_to`: a registered project). It is still only a
//! report. It never moves a decision: the move is `hivemind move --decision
//! <id> --to <project>` (`decision.moved`, recorded and reversible), run by
//! whoever confirmed the list, so nothing is filed or re-filed by a rule.
use std::time::Instant;

use serde::Serialize;

use crate::commands::normalize_topic_key;
use crate::ledger::EventLedger;
use crate::projector::GraphView;
use crate::Result;

use super::projects::{get_project, ProjectOutcome};
use super::search::{search_decisions, SearchDecisionRequest};
use super::shared::{query_error, MAX_QUERY_RESULTS};
use super::QueryResponse;

/// A decision that carries a topic key the caller has named as foreign to
/// this ledger.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MisfiledDecisionCandidate {
    pub decision_id: String,
    pub title: String,
    /// Which of the decision's own topic keys triggered the match.
    pub matched_topic_keys: Vec<String>,
    /// Actors on record for the decision (proposer plus any reviewers), for
    /// routing the review.
    pub actor_ids: Vec<String>,
    /// The project the decision is filed under now (a registered handle or a
    /// personal address), so the reviewer sees what a move would leave.
    pub project: Option<String>,
    /// Where the request said the flagged decisions belong (`move_to`), echoed on
    /// every row so a row is self-contained. Absent when the request named none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_move_to: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MisfiledScanRequest {
    /// Topic keys that indicate a decision belongs to a different ledger.
    /// Normalized the same way capture normalizes topic keys, so casing and
    /// punctuation in the caller's input never cause a missed match.
    pub foreign_topic_keys: Vec<String>,
    /// Only decisions filed exactly under this project: a registered handle or a personal
    /// address. No parent or dependency walk. `None` scans every project. The caller checks
    /// a handle is registered (`require_registered_project`).
    pub project: Option<String>,
    /// The registered project the flagged decisions belong in. Decisions already filed there
    /// are not flagged: a move to where they are is not a move. The caller checks the handle
    /// is registered (`require_registered_project`).
    pub move_to: Option<String>,
    /// Maximum number of candidates to return (capped at `MAX_QUERY_RESULTS`).
    pub limit: usize,
    /// Skip N candidates (offset-based cursor, same shape as the other bulk scans).
    pub cursor: Option<String>,
}

/// Refuse a project handle no one registered, so a typo reads as the refusal it is and never as
/// "nothing misfiled here". A personal address always resolves. The scan itself is
/// graph-only and cannot see the registry, so the verbs that have a ledger call this first.
pub fn require_registered_project(ledger: &impl EventLedger, handle: &str) -> Result<()> {
    match get_project(ledger, handle)?.data {
        ProjectOutcome::Found { .. } => Ok(()),
        ProjectOutcome::NotFound => Err(query_error(format!(
            "no project called {handle}: register it first with `hivemind project register {handle}`"
        ))
        .into()),
    }
}

/// Scan decisions for ones carrying a caller-named "foreign" topic key.
/// Read-only: flags candidates for human review, moves nothing.
pub fn scan_misfiled_decisions(
    graph: &impl GraphView,
    request: &MisfiledScanRequest,
) -> Result<QueryResponse<Vec<MisfiledDecisionCandidate>>> {
    let started = Instant::now();

    let foreign: Vec<String> = request
        .foreign_topic_keys
        .iter()
        .map(|key| normalize_topic_key(key))
        .filter(|key| !key.is_empty())
        .collect();
    if foreign.is_empty() {
        return Err(
            query_error("foreign_topic_keys must contain at least one non-empty key").into(),
        );
    }

    // One bounded fetch: beyond MAX_QUERY_RESULTS decisions in the tenant, this scan is itself
    // truncated (never silently — `truncated` below says so) rather than
    // paging to exhaustion on every call.
    let search_request = SearchDecisionRequest {
        limit: MAX_QUERY_RESULTS,
        ..SearchDecisionRequest::default()
    };
    let searched = search_decisions(graph, &search_request)?;

    let mut candidates: Vec<MisfiledDecisionCandidate> = searched
        .data
        .items
        .into_iter()
        .filter_map(|item| {
            if let Some(project) = request.project.as_deref() {
                if item.decision.project.as_deref() != Some(project) {
                    return None;
                }
            }
            if let Some(destination) = request.move_to.as_deref() {
                if item.decision.project.as_deref() == Some(destination) {
                    return None;
                }
            }
            let matched: Vec<String> = item
                .decision
                .topic_keys
                .iter()
                .filter(|topic| foreign.iter().any(|f| f == *topic))
                .cloned()
                .collect();
            if matched.is_empty() {
                return None;
            }
            Some(MisfiledDecisionCandidate {
                decision_id: item.decision.id,
                title: item.decision.title,
                matched_topic_keys: matched,
                actor_ids: item.graph_context.actor_ids,
                project: item.decision.project,
                proposed_move_to: request.move_to.clone(),
            })
        })
        .collect();
    candidates.sort_by(|left, right| left.decision_id.cmp(&right.decision_id));

    let skip: usize = match request.cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor.parse::<usize>().map_err(|error| {
            query_error(format!("cursor must be a non-negative offset: {error}"))
        })?,
    };
    let limit = if request.limit == 0 {
        MAX_QUERY_RESULTS
    } else {
        request.limit.min(MAX_QUERY_RESULTS)
    };

    let total_matches = candidates.len();
    let window: Vec<MisfiledDecisionCandidate> =
        candidates.into_iter().skip(skip).take(limit).collect();
    // Truncated if the underlying decision search itself was truncated (more
    // decisions exist than this scan looked at) or if this scan's own
    // pagination window didn't reach every match it found.
    let truncated = searched.truncated || skip + window.len() < total_matches;

    Ok(QueryResponse {
        result_count: window.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: window,
    })
}

pub fn misfiled_next_cursor(skip: usize, window_len: usize) -> Option<String> {
    if window_len > 0 {
        Some((skip + window_len).to_string())
    } else {
        None
    }
}
