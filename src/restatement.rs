//! Layer 3 for restatements: judging that a captured decision states a recorded one again, and
//! proposing links for decisions that were already recorded twice (hivemind-83cj).
//!
//! Two jobs, both optional: the write path and the reads are correct without either.
//!
//! - **Judging.** After a classifier extracts a decision, it is shown the closest recorded
//!   decisions (the same read `recall` and `why` use) and says which of them, if any, the new
//!   capture restates. Its answer is `CaptureItem::restates_id`; the write layer only checks the
//!   id names a recorded decision. A model that names an id it was not shown, or none, records the
//!   capture as a decision of its own: leaving a duplicate is the safe failure, linking two
//!   different decisions is not.
//! - **Proposing.** For decisions recorded before any of this, [`propose_same_as_links`] lists
//!   links a person can approve once. Its basis is printed with every proposal: the title words
//!   two decisions share and their overlap, nothing a model said. Nothing is written until
//!   [`apply_links`] is told to, and nothing is ever deleted or rewritten (H22).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::commands::Commands;
use crate::events::{CaptureItem, EventId};
use crate::ledger::EventLedger;
use crate::projector::GraphView;
use crate::queries::{
    decision_headings, resolve_decision_for_reading, same_as_groups, supersession_pairs,
    text_terms, DecisionHeading, ResolveOutcome,
};
use crate::Result;

/// How many recorded decisions a capture is compared with.
pub const NEARBY_LIMIT: usize = 5;

/// Fewest words two titles must share, after common words are dropped, to be proposed.
const MIN_SHARED_TERMS: usize = 3;
/// Smallest share of all the words of two titles (shared / either) to be proposed: the bar the
/// product check scans with, so what it reports is what the proposals cover.
const MIN_TITLE_OVERLAP: f64 = 0.5;
const MAX_JUDGE_TOKENS: u32 = 200;

// ---------------------------------------------------------------------------
// Judging
// ---------------------------------------------------------------------------

/// A recorded decision shown to the judge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearbyDecision {
    pub decision_id: String,
    pub title: String,
}

/// The recorded decisions closest to a decision capture, by the words of its title: what `recall`
/// would answer to the capture's title. A decision recorded more than once and linked `SAME_AS`
/// is shown once.
pub fn nearby_decisions(
    graph: &impl GraphView,
    capture: &CaptureItem,
) -> Result<Vec<NearbyDecision>> {
    let title = capture.title.trim();
    if title.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = match resolve_decision_for_reading(graph, title, None)?.data {
        ResolveOutcome::Resolved { candidate } => vec![candidate],
        ResolveOutcome::Ambiguous { candidates } => candidates,
        ResolveOutcome::NotFound => Vec::new(),
    };
    Ok(candidates
        .into_iter()
        .take(NEARBY_LIMIT)
        .map(|candidate| NearbyDecision {
            decision_id: candidate.decision_id,
            title: candidate.title,
        })
        .collect())
}

/// The question put to the judge model for one capture and the decisions near it.
pub fn restatement_prompt(capture: &CaptureItem, nearby: &[NearbyDecision]) -> String {
    let mut prompt = String::from(
        "You are the HiveMind restatement checker.\n\n\
A new decision was extracted from a conversation. Some decisions already recorded look close to \
it. Say which recorded decision, if any, the new decision RESTATES: the same choice (the same \
question, answered the same way) made again or relayed once more.\n\n\
Answer null when it only concerns the same topic, answers a different question, chooses \
differently, narrows or widens an earlier choice, or replaces it. Answer null when you are not \
sure: two records of one decision are a small cost, two different decisions called one are not. \
Answer with the id exactly as listed, or null.\n\nNEW DECISION\n",
    );
    prompt.push_str(&format!("title: {}\n", capture.title.trim()));
    if let Some(chosen) = capture.chosen_option.as_deref() {
        prompt.push_str(&format!("chosen: {chosen}\n"));
    }
    prompt.push_str(&format!(
        "why: {}\n\nRECORDED DECISIONS\n",
        capture.rationale.trim()
    ));
    for decision in nearby {
        prompt.push_str(&format!(
            "- id: {}\n  title: {}\n",
            decision.decision_id, decision.title
        ));
    }
    prompt
}

/// JSON Schema of the judge's answer.
pub fn restatement_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "restates_id": { "oneOf": [{ "type": "string" }, { "type": "null" }] }
        },
        "required": ["restates_id"],
        "additionalProperties": false
    })
}

/// The judge's answer.
#[derive(Debug, Deserialize)]
pub struct RestatementAnswer {
    pub restates_id: Option<String>,
}

impl RestatementAnswer {
    /// The decision named, only when it is one of those shown: an id the judge was not given is
    /// an invention, and it never reaches the ledger.
    pub fn named_among(self, nearby: &[NearbyDecision]) -> Option<String> {
        let named = self.restates_id?;
        let named = named.trim();
        nearby
            .iter()
            .find(|decision| decision.decision_id == named)
            .map(|decision| decision.decision_id.clone())
    }
}

/// Sets `restates_id` on each decision capture the judge says restates a recorded decision.
/// Best effort by design: a failed read or model call leaves that capture as it was extracted,
/// because this layer may be absent or down and recording must not wait on it.
pub async fn mark_restatements(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    graph: &impl GraphView,
    captures: &mut [CaptureItem],
) {
    for capture in captures
        .iter_mut()
        .filter(|capture| capture.kind == "decision" && capture.restates_id.is_none())
    {
        let nearby = match nearby_decisions(graph, capture) {
            Ok(nearby) if !nearby.is_empty() => nearby,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(target: "hivemind::classifier", "restatement lookup failed: {error}");
                continue;
            }
        };
        let answer: std::result::Result<RestatementAnswer, _> = crate::anthropic::call_json_schema(
            client,
            api_key,
            model,
            MAX_JUDGE_TOKENS,
            restatement_prompt(capture, &nearby),
            restatement_schema(),
        )
        .await;
        match answer {
            Ok(answer) => capture.restates_id = answer.named_among(&nearby),
            Err(error) => {
                tracing::warn!(target: "hivemind::classifier", "restatement judge failed: {error}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Proposing links for decisions already recorded more than once
// ---------------------------------------------------------------------------

/// One proposed link: `decision_id` (recorded later) is the same decision as `restates_id`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProposedLink {
    pub decision_id: String,
    pub title: String,
    pub restates_id: String,
    pub restates_title: String,
    pub project: Option<String>,
    /// The title words the two share, after common words are dropped: the whole basis.
    pub shared_terms: Vec<String>,
    /// Shared words over all the words of both titles, 0 to 1.
    pub overlap: f64,
}

/// What the scan found.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestatementProposals {
    pub proposed: Vec<ProposedLink>,
    /// Decisions scanned, so an empty list reads as "checked this many", not "checked nothing".
    pub decisions_scanned: usize,
}

/// Proposes `SAME_AS` links for decisions that look like one decision recorded more than once.
///
/// Each decision is compared with the ones recorded before it in the same project: the best of
/// those sharing at least three title words and at least half of all the words of both titles
/// is proposed, ties to the earliest. A pair already linked (directly or through others) is not
/// proposed, and neither is a pair where one decision superseded the other: a replacement is a
/// different decision however alike the words. `project` limits the scan to one project.
///
/// A proposal is a candidate for a person to confirm, never a finding: title words cannot tell
/// the same choice from a similar one.
pub fn propose_same_as_links(
    graph: &impl GraphView,
    project: Option<&str>,
) -> Result<RestatementProposals> {
    let headings: Vec<DecisionHeading> = decision_headings(graph)?
        .into_iter()
        .filter(|heading| project.is_none_or(|project| heading.project.as_deref() == Some(project)))
        .collect();
    let terms: Vec<BTreeSet<String>> = headings
        .iter()
        .map(|heading| text_terms(&heading.title).into_iter().collect())
        .collect();

    let mut group_of: HashMap<String, usize> = HashMap::new();
    for (group, members) in same_as_groups(graph)?.into_iter().enumerate() {
        for member in members {
            group_of.insert(member, group);
        }
    }
    let superseded = supersession_pairs(graph)?;

    // word -> the decisions (by position) whose title has it, so a decision is only compared with
    // the ones it shares a word with.
    let mut with_term: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut proposed = Vec::new();
    for (index, heading) in headings.iter().enumerate() {
        let mut shared_with: HashMap<usize, usize> = HashMap::new();
        for term in &terms[index] {
            for &earlier in with_term.get(term.as_str()).into_iter().flatten() {
                *shared_with.entry(earlier).or_default() += 1;
            }
        }
        let best = shared_with
            .into_iter()
            .filter(|&(earlier, shared)| {
                let other = &headings[earlier];
                shared >= MIN_SHARED_TERMS
                    && other.project == heading.project
                    && group_of
                        .get(&heading.decision_id)
                        .is_none_or(|group| group_of.get(&other.decision_id) != Some(group))
                    && !superseded
                        .contains(&(heading.decision_id.clone(), other.decision_id.clone()))
                    && !superseded
                        .contains(&(other.decision_id.clone(), heading.decision_id.clone()))
            })
            .map(|(earlier, shared)| {
                let union = terms[index].len() + terms[earlier].len() - shared;
                (earlier, shared, shared as f64 / union as f64)
            })
            .filter(|&(_, _, overlap)| overlap >= MIN_TITLE_OVERLAP)
            .max_by(|left, right| {
                left.2
                    .total_cmp(&right.2)
                    // The earlier of two equally close decisions wins.
                    .then(right.0.cmp(&left.0))
            });
        if let Some((earlier, _, overlap)) = best {
            let other = &headings[earlier];
            proposed.push(ProposedLink {
                decision_id: heading.decision_id.clone(),
                title: heading.title.clone(),
                restates_id: other.decision_id.clone(),
                restates_title: other.title.clone(),
                project: heading.project.clone(),
                shared_terms: terms[index]
                    .intersection(&terms[earlier])
                    .cloned()
                    .collect(),
                overlap,
            });
        }
        for term in &terms[index] {
            with_term.entry(term.as_str()).or_default().push(index);
        }
    }
    Ok(RestatementProposals {
        proposed,
        decisions_scanned: headings.len(),
    })
}

/// One link applied, or found already there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppliedLink {
    pub decision_id: String,
    pub restates_id: String,
    /// The `relation.added` event, or `None` when the two were already linked.
    pub event_id: Option<EventId>,
}

/// Records each `(decision, restated)` pair as a `SAME_AS` link attributed to `actor_id`. All
/// pairs are checked before the first is written, so a typo in the last one leaves the ledger as
/// it was; a pair already linked writes nothing.
pub fn apply_links<L: EventLedger>(
    commands: &Commands<'_, L>,
    actor_id: &str,
    links: &[(String, String)],
) -> Result<Vec<AppliedLink>> {
    for (decision_id, restated_id) in links {
        commands.require_linkable(decision_id, restated_id)?;
    }
    links
        .iter()
        .map(|(decision_id, restated_id)| {
            Ok(AppliedLink {
                decision_id: decision_id.clone(),
                restates_id: restated_id.clone(),
                event_id: commands.link_same_as(actor_id, decision_id, restated_id)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
