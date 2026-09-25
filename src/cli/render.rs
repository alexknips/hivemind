use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::commands::{DecisionMoveOutcome, DecisionPlacement, RestsOn, RestsOnKind};
use crate::error::{CliError, CommandError};
use crate::events::{EventId, EventType};
use crate::ingest::{DocumentImportReport, DocumentPreparationReport};
use crate::projector::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind,
    RelationKind as GraphRelationKind,
};
use crate::queries::{
    derive_decision_status, derive_hypothesis_status, oriented_edges,
    BlockerNotificationCandidates, CompactView, DecidedBy, DecisionBlockerResults, DecisionBrief,
    DecisionSearchResults, DecisionStatus, DecisionView, DecisionsAddedSinceResults,
    DecisionsChangedSinceResults, GroundingAdded, GroundingItem, GroundingItemState, GroundingKind,
    GroundingState, HistoryChangeKind, HypothesisStatus, MatchReason, MisfiledDecisionCandidate,
    NeighborhoodView, OutcomeReason, ProjectDecisionsOutcome, ProjectDecisionsPage,
    ProjectListResults, ProjectMove, ProjectOutcome, QualityTier, QueryResponse, ReadOnlyExport,
    ReadOnlyExportFormat as QueryReadOnlyExportFormat, ReadOnlyExportQueryKind,
    RecentActivityResults, RecentDecisionsResults, ResolveOutcome, ScoredDecision,
    SituationalResults, SupersessionChain,
};
use crate::{HivemindError, Result};

use super::args::CliExit;

pub(crate) fn render_compact_view_summary(view: &Option<CompactView>) -> String {
    let Some(v) = view else {
        return "decision not found".to_owned();
    };
    let mut out = format!(
        "CompactView: {} [{:?}]\n  project: {}\n  rationale: {}\n",
        v.decision.id,
        v.decision.status,
        summary_cell(&v.decision.project_label),
        v.decision.rationale,
    );
    if let (Some(question), Some(quote)) = (&v.decision.question, &v.decision.quote) {
        out.push_str(&format!("  answers: {question}\n  quote: \"{quote}\"\n"));
    }
    if let Some(chain) = &v.supersession_chain {
        out.push_str(&format!(
            "  superseded {} earlier decision(s); oldest: {}\n",
            chain.chain_length - 1,
            chain.oldest_id
        ));
    }
    if let Some(contest) = &v.contest {
        out.push_str(&format!(
            "  CONTESTED: accepted_by={:?} rejected_by={:?}\n",
            contest.accepted_by, contest.rejected_by
        ));
    }
    out.push_str(&format!("  {}\n", compact_rests_on(v)));
    for premise in &v.premises {
        if matches!(
            premise.status,
            DecisionStatus::Superseded | DecisionStatus::Rejected
        ) {
            out.push_str(&format!(
                "  STALE: follows from {}, which is {}\n",
                premise.decision_id,
                decision_status_label(premise.status)
            ));
        }
    }
    if v.dependents_count > 0 {
        out.push_str(&format!("  {}\n", dependents_line(v.dependents_count)));
    }
    out.push_str(&format!("  hypotheses: {}\n", v.hypotheses.len()));
    out.push_str(&format!("  evidence_ids: {}\n", v.evidence_ids.len()));
    out.push_str(&format!("  active_blockers: {}\n", v.active_blockers.len()));
    out.push_str(&format!(
        "  elided: {} superseded, {} unchosen options\n",
        v.elided.superseded_decision_count, v.elided.unchosen_option_count
    ));
    out
}

pub(crate) fn render_recent_decisions_summary(results: &RecentDecisionsResults) -> String {
    if results.items.is_empty() {
        return "No recent decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let timestamp = item
            .creation
            .ts
            .map(|ts| ts.to_rfc3339())
            .unwrap_or_else(|| "unknown-ts".to_owned());
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}\tactor={}\tsource={}\tproject={}\tcitation={}",
            timestamp,
            decision_status_label(item.status),
            item.decision_id,
            summary_cell(&item.title),
            item.actor_ids.join(","),
            item.creation.source.as_str(),
            summary_cell(&item.project_label),
            item.creation.citation_id
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_recent_activity_summary(results: &RecentActivityResults) -> String {
    if results.items.is_empty() {
        return "No recent activity found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\tactor={}\tsource={}\tdecisions={}\tcitation={}{}",
            item.event_origin,
            change_kind_label(item.change_kind),
            event_type_label(item.event_type),
            item.actor_id,
            item.source.as_str(),
            item.decision_ids.join(","),
            item.citation_id,
            project_move_suffix(item.project_move.as_ref(), item.ts)
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_changed_since_summary(results: &DecisionsChangedSinceResults) -> String {
    if results.items.is_empty() {
        return "No changed decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\tactor={}\tsource={}\tdecisions={}\tcitation={}{}",
            item.event_origin,
            change_kind_label(item.change_kind),
            event_type_label(item.event_type),
            item.actor_id,
            item.source.as_str(),
            item.decision_ids.join(","),
            item.citation_id,
            project_move_suffix(item.project_move.as_ref(), item.ts)
        );
    }
    output.trim_end().to_owned()
}

/// The tail of a `project_moved` history line: where the decision went and when. Empty for every
/// other kind of change, so the line for those is unchanged. The actor is already in the line.
fn project_move_suffix(project_move: Option<&ProjectMove>, ts: Option<DateTime<Utc>>) -> String {
    let Some(project_move) = project_move else {
        return String::new();
    };
    let moved_at = ts.map_or_else(|| "unknown".to_owned(), |ts| ts.to_rfc3339());
    format!(
        "\tmoved={}->{}\tat={moved_at}",
        project_move.from, project_move.to
    )
}

pub(crate) fn render_added_since_summary(results: &DecisionsAddedSinceResults) -> String {
    if results.added_decisions.is_empty() && results.changed_existing_decisions.is_empty() {
        return "No added or changed decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.added_decisions {
        let _ = writeln!(
            output,
            "added\t{}\t{}\ttopics={}\tcitation={}\tchanges={}",
            decision_status_label(item.status),
            item.decision_id,
            item.topic_keys.join(","),
            item.creation.citation_id,
            item.changes_in_window.len()
        );
    }
    for item in &results.changed_existing_decisions {
        let _ = writeln!(
            output,
            "changed\t{}\t{}\ttopics={}\tchanges={}",
            decision_status_label(item.status),
            item.decision_id,
            item.topic_keys.join(","),
            item.changes_in_window.len()
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_read_only_export_summary(export: &ReadOnlyExport) -> String {
    if let Some(markdown) = &export.markdown {
        return markdown.trim_end().to_owned();
    }

    format!(
        "read_only_export\tquery={}\tformat={}\tresult_count={}\ttruncated={}\tcitations={}",
        read_only_query_label(export.query),
        read_only_format_label(export.format),
        export.result_count,
        export.truncated,
        export.citation_map.len()
    )
}

pub(crate) fn format_query_response<T: Serialize>(
    summary: bool,
    response: &QueryResponse<T>,
    render_summary: impl FnOnce(&T) -> String,
    next_cursor: Option<&str>,
) -> Result<String> {
    if !summary {
        return format_json_value(true, response);
    }

    let mut output = render_summary(&response.data);
    append_truncation_notice(&mut output, response.truncated, next_cursor);
    Ok(output.trim_end().to_owned())
}

pub(crate) fn append_truncation_notice(
    output: &mut String,
    truncated: bool,
    next_cursor: Option<&str>,
) {
    if !truncated {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    match next_cursor {
        Some(cursor) => {
            let _ = write!(
                output,
                "truncated=true next_cursor={}",
                summary_cell(cursor)
            );
        }
        None => output.push_str("truncated=true"),
    }
}

pub(crate) fn render_decision_summary(decision: &Option<DecisionView>) -> String {
    let Some(decision) = decision else {
        return "No decision found".to_owned();
    };

    let mut output = String::new();
    write_decision_summary_row(&mut output, "decision", decision);
    output.trim_end().to_owned()
}

pub(crate) fn render_decision_list_summary(decisions: &[DecisionView]) -> String {
    if decisions.is_empty() {
        return "No decisions found".to_owned();
    }

    let mut output = String::new();
    for decision in decisions {
        write_decision_summary_row(&mut output, "decision", decision);
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_search_summary(results: &DecisionSearchResults) -> String {
    if results.items.is_empty() {
        return "No matching decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "match\trank={}\t{}\t{}\t{}\ttopics={}\tproject={}\tmatched={}",
            item.rank,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            item.decision.topic_keys.join(","),
            summary_cell(&item.decision.project_label),
            item.matched_fields.join(",")
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_situational_summary(results: &SituationalResults) -> String {
    if results.matches.is_empty() {
        let mut output = format!(
            "No decisions bear on this situation (terms: {})",
            results.query_terms.join(",")
        );
        // An empty scoped answer still says which projects it looked in.
        if let Some(scope) = &results.scope {
            let _ = write!(output, "\nscope\t{}", summary_cell(&scope.note));
        }
        return output;
    }

    let mut output = String::new();
    let _ = writeln!(output, "terms\t{}", results.query_terms.join(","));
    if let Some(scope) = &results.scope {
        let _ = writeln!(output, "scope\t{}", summary_cell(&scope.note));
    }
    for item in &results.matches {
        let held_up = if item.outcome.held_up {
            "holds".to_owned()
        } else {
            // The reason rides inside the cell: STALE(premise superseded), STALE(assumption
            // refuted), STALE(superseded), ...
            let labels = still_holds_labels(&item.outcome.reasons);
            if labels.is_empty() {
                "STALE".to_owned()
            } else {
                format!("STALE({})", labels.join(", "))
            }
        };
        let changed = match item.changed_since {
            Some(true) => "changed",
            Some(false) => "unchanged",
            None => "-",
        };
        let _ = write!(
            output,
            "match\tscore={:.2}\t{}\t{}\t{}\t{}\tsince={}\tproject={}\t",
            item.score,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            held_up,
            changed,
            summary_cell(&item.decision.project_label),
        );
        // Only a project-scoped answer says how each decision reached it.
        if let Some(scope) = &item.scope {
            let _ = write!(output, "scope={}\t", summary_cell(&scope.label));
        }
        // Exact topic_keys membership and fuzzy evidence-content overlap are visually
        // distinguished so an agent doesn't over-trust the fuzzy half (AGENTS.md §6).
        for (index, reason) in item.matched_via.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            match reason {
                MatchReason::TopicKey { topic } => {
                    let _ = write!(output, "constrains(topic:{topic})");
                }
                MatchReason::EvidenceOverlap { terms, .. } => {
                    let _ = write!(output, "may-relate(evidence:{})", terms.join("+"));
                }
            }
        }
        output.push('\n');
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_recall_summary(response: &crate::summarize::RecallResponse) -> String {
    let mut output = String::new();
    if response.ranked.items.is_empty() {
        let mut empty = "No decisions found matching the query.".to_owned();
        if !response.ignored_words.is_empty() {
            let _ = write!(
                empty,
                "\nignored question words: {}",
                response.ignored_words.join(" ")
            );
        }
        // An empty scoped answer still says which projects it looked in.
        if let Some(scope) = &response.scope {
            let _ = write!(empty, "\nscope\t{}", summary_cell(&scope.note));
        }
        return empty;
    }
    if !response.ignored_words.is_empty() {
        let _ = writeln!(output, "ignored\t{}", response.ignored_words.join(" "));
    }
    if let Some(scope) = &response.scope {
        let _ = writeln!(output, "scope\t{}", summary_cell(&scope.note));
    }
    let _ = writeln!(output, "digest\t{}", summary_cell(&response.digest.summary));
    let _ = writeln!(
        output,
        "cited\t{}",
        response.digest.cited_decision_ids.join(",")
    );
    for item in &response.ranked.items {
        let _ = write!(
            output,
            "match\trank={}\t{}\t{}\t{}\ttopics={}\tproject={}",
            item.rank,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            item.decision.topic_keys.join(","),
            summary_cell(&item.decision.project_label),
        );
        // Only a project-scoped answer says how each decision reached it.
        if let Some(scope) = &item.scope {
            let _ = write!(output, "\tscope={}", summary_cell(&scope.label));
        }
        output.push('\n');
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_scored_decision_summary(scored: &Option<ScoredDecision>) -> String {
    let Some(s) = scored else {
        return "No decision found".to_owned();
    };
    let mut output = String::new();
    let _ = writeln!(
        output,
        "scored\t{}\tscore={:.3}\ttier={}",
        s.decision_id,
        s.score,
        quality_tier_label(s.tier)
    );
    for reason in &s.reasons {
        let _ = writeln!(
            output,
            "reason\t{}",
            summary_cell(&format!("{reason:?}")), // ubs:ignore: Debug-format alloc in loop; no &str alternative
        );
    }
    if !s.contributing_ids.is_empty() {
        let _ = writeln!(output, "contributing\t{}", s.contributing_ids.join(","));
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_scan_quality_summary(decisions: &[ScoredDecision]) -> String {
    if decisions.is_empty() {
        return "No decisions found".to_owned();
    }
    let mut output = String::new();
    for s in decisions {
        let _ = writeln!(
            output,
            "scored\t{}\tscore={:.3}\ttier={}\treasons={}",
            s.decision_id,
            s.score,
            quality_tier_label(s.tier),
            s.reasons.len()
        );
    }
    output.trim_end().to_owned()
}

fn quality_tier_label(tier: QualityTier) -> &'static str {
    tier.as_str()
}

pub(crate) fn render_misfiled_scan_summary(candidates: &[MisfiledDecisionCandidate]) -> String {
    if candidates.is_empty() {
        return "No misfiled candidates found".to_owned();
    }
    let mut output = String::new();
    for candidate in candidates {
        let _ = writeln!(
            output,
            "misfiled\t{}\t{}\ttopics={}\tactors={}",
            candidate.decision_id,
            candidate.title,
            candidate.matched_topic_keys.join(","),
            candidate.actor_ids.join(",")
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_supersession_summary(chain: &SupersessionChain) -> String {
    if chain.decision_ids.is_empty() {
        return "No supersession chain found".to_owned();
    }

    let mut output = String::new();
    for (index, decision_id) in chain.decision_ids.iter().enumerate() {
        let marker = if index == chain.input_index {
            "input"
        } else {
            "chain"
        };
        let _ = writeln!(output, "{marker}\t{index}\t{decision_id}");
    }
    output.trim_end().to_owned()
}

/// Leads with the decision, then why, who decided, and whether it still holds — the shared
/// output contract for fluent verbs (docs/AGENT_FLUENT_QUERYING.md §4). IDs are a trailing
/// "ref:" line, present for follow-up, never required reading to understand the answer.
pub(crate) fn render_decision_brief_summary(brief: &Option<DecisionBrief>) -> String {
    let Some(brief) = brief else {
        return "No decision found".to_owned();
    };

    let mut output = String::new();
    write_decision_brief(&mut output, brief);
    output.trim_end().to_owned()
}

fn write_decision_brief(output: &mut String, brief: &DecisionBrief) {
    let _ = writeln!(
        output,
        "decision: {} [{}]",
        summary_cell(&brief.title),
        decision_status_label(brief.status)
    );
    let _ = writeln!(output, "  project: {}", summary_cell(&brief.project_label));
    let _ = writeln!(output, "  rationale: {}", summary_cell(&brief.rationale));
    if let (Some(question), Some(quote)) = (&brief.question, &brief.quote) {
        let _ = writeln!(output, "  answers: {}", summary_cell(question));
        let _ = writeln!(output, "  quote: \"{}\"", summary_cell(quote));
    }
    if let Some(chosen) = &brief.chosen_option {
        let _ = writeln!(output, "  chose: {}", summary_cell(&chosen.label));
    }
    if !brief.rejected_options.is_empty() {
        let labels: Vec<&str> = brief
            .rejected_options
            .iter()
            .map(|option| option.label.as_str())
            .collect();
        let _ = writeln!(
            output,
            "  rejected: {} (shares the rationale above — no distinct per-option reason is recorded)",
            labels.join(", ")
        );
    }
    write_decided_by(output, &brief.decided_by);
    if let Some(occurred_at) = brief.occurred_at {
        let _ = writeln!(output, "  when: {}", occurred_at.to_rfc3339());
    }
    write_rests_on(output, brief);
    if brief.still_holds.held_up {
        let _ = writeln!(output, "  still holds: yes");
    } else {
        let labels = still_holds_labels(&brief.still_holds.reasons);
        if labels.is_empty() {
            let _ = writeln!(output, "  still holds: NO");
        } else {
            let _ = writeln!(output, "  still holds: NO: {}", labels.join(", "));
        }
    }
    for reason in &brief.still_holds.reasons {
        let _ = writeln!(
            output,
            "    - {}",
            format_outcome_reason(reason, &brief.rests_on)
        );
    }
    for unchecked in &brief.still_holds.unchecked {
        let label = brief
            .rests_on
            .iter()
            .find(|item| item.id == unchecked.hypothesis_id)
            .map_or(unchecked.hypothesis_id.as_str(), |item| item.label.as_str());
        let _ = writeln!(
            output,
            "  unchecked: bet \"{}\" — check date {} has passed, no evidence recorded either way",
            summary_cell(label),
            unchecked.check_by.format("%Y-%m-%d")
        );
    }
    if let Some(confidence) = &brief.expressed_confidence {
        let _ = writeln!(
            output,
            "  confidence at capture: {confidence} (decider's words)"
        );
    }
    if brief.dependents_count > 0 {
        let _ = writeln!(output, "  {}", dependents_line(brief.dependents_count));
    }
    if !brief.topic_keys.is_empty() {
        let _ = writeln!(output, "  topics: {}", brief.topic_keys.join(","));
    }
    let _ = writeln!(output, "  ref: {}", brief.decision_id);
}

/// Recorder and decider are distinct actors (hivemind-zdsh.9): the recorder is whoever
/// wrote the decision down (`PROPOSED_BY`), the decider is whoever actually made the call
/// (`ACCEPTED_BY`). Collapsed to one "decided by" line only on self-acceptance, where
/// they're the same actor; an unreviewed decision says so honestly instead of implying the
/// recorder decided.
fn write_decided_by(output: &mut String, decided_by: &DecidedBy) {
    let recorder = decided_by.proposer_id.as_deref().unwrap_or("unknown");
    match decided_by.decider_ids.as_slice() {
        [] => {
            let _ = writeln!(
                output,
                "  recorded by: {} (source={}) -- not yet decided ({:?})",
                recorder, decided_by.source, decided_by.review
            );
        }
        [only] if only == recorder => {
            let _ = writeln!(
                output,
                "  decided by: {} (source={}, review={:?})",
                recorder, decided_by.source, decided_by.review
            );
        }
        deciders => {
            let _ = writeln!(
                output,
                "  recorded by: {} (source={})",
                recorder, decided_by.source
            );
            let _ = writeln!(
                output,
                "  decided by: {} (review={:?})",
                deciders.join(", "),
                decided_by.review
            );
        }
    }
    // Case 2 of the attribution ruling (hivemind-zdsh.6): the agent decided, but within a
    // scope a human delegated — stated on its own line so it can't be missed or confused
    // with an agent that decided alone (which has no such line).
    if let Some(delegated_by) = decided_by.delegated_by.as_deref() {
        let _ = writeln!(output, "  delegated by: {delegated_by}");
    }
}

/// The short reasons a decision no longer holds, in the order they were derived and without
/// repeats: `superseded`, `premise superseded`, `premise rejected`, `assumption refuted`,
/// `contested`. Thin structure is a quality note, not a reason it stopped holding.
fn still_holds_labels(reasons: &[OutcomeReason]) -> Vec<&'static str> {
    let mut labels: Vec<&'static str> = Vec::new();
    for reason in reasons {
        let label = match reason {
            OutcomeReason::SupersededBy { .. } => "superseded",
            OutcomeReason::PremisedOnRefuted { .. } => "assumption refuted",
            OutcomeReason::PremiseSuperseded { .. } => "premise superseded",
            OutcomeReason::PremiseRejected { .. } => "premise rejected",
            OutcomeReason::Contested => "contested",
            OutcomeReason::ThinStructure { .. } => continue,
        };
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

/// One outcome reason as a sentence. Ids are output handles, so a premise decision is named by
/// its title when `rests_on` carries it.
fn format_outcome_reason(reason: &OutcomeReason, rests_on: &[GroundingItem]) -> String {
    let named = |id: &str| -> String {
        rests_on.iter().find(|item| item.id == id).map_or_else(
            || id.to_owned(),
            |item| format!("\"{}\"", summary_cell(&item.label)),
        )
    };
    match reason {
        OutcomeReason::SupersededBy { by_id, gap_events } => {
            let gap = gap_events
                .map(|gap| format!(" ({gap} ledger events later)"))
                .unwrap_or_default();
            format!("superseded by {by_id}{gap}")
        }
        OutcomeReason::PremisedOnRefuted { hypothesis_id } => {
            format!("premised on refuted hypothesis {}", named(hypothesis_id))
        }
        OutcomeReason::PremiseSuperseded { decision_id, by_id } => {
            // The premise's own item carries the superseder's title; the id is the fallback.
            let by = rests_on
                .iter()
                .find(|item| item.id == *decision_id)
                .and_then(|item| match &item.state {
                    GroundingItemState::Superseded {
                        by_label: Some(by_label),
                        ..
                    } => Some(format!("\"{}\"", summary_cell(by_label))),
                    _ => None,
                })
                .unwrap_or_else(|| by_id.clone()); // ubs:ignore: fallback owned copy for the sentence; the reason is only borrowed
            format!(
                "follows from {}, which was superseded by {by}",
                named(decision_id)
            )
        }
        OutcomeReason::PremiseRejected { decision_id } => {
            format!("follows from {}, which was rejected", named(decision_id))
        }
        OutcomeReason::Contested => "contested: accepted and rejected actors disagree".to_owned(),
        OutcomeReason::ThinStructure {
            no_options,
            nothing_declared,
        } => match (no_options, nothing_declared) {
            (true, true) => {
                "thin structure: no options attached, nothing declared about what it rests on"
                    .to_owned()
            }
            (true, false) => "thin structure: no options attached".to_owned(),
            (false, true) => "thin structure: nothing declared about what it rests on".to_owned(),
            (false, false) => "thin structure".to_owned(),
        },
    }
}

/// `N decisions rest on this` (`1 decision rests on this`).
fn dependents_line(count: usize) -> String {
    if count == 1 {
        "1 decision rests on this".to_owned()
    } else {
        format!("{count} decisions rest on this")
    }
}

/// The `rests on:` part of the brief: one line per item with its state and whether it was named
/// at capture or attributed later, a one-line form for a decision that is a lone bet, and an
/// honest "nothing declared" for a decision nobody asked. Answers "what does this rest on?"
/// with the labels a person reads; the ids stay in the JSON.
fn write_rests_on(output: &mut String, brief: &DecisionBrief) {
    if brief.rests_on.is_empty() {
        let _ = writeln!(
            output,
            "  rests on: nothing declared (never asked; add with hivemind ground \"{}\" --rests-on-decision \"...\")",
            summary_cell(&brief.title)
        );
        return;
    }
    if let [bet] = brief.rests_on.as_slice() {
        if bet.kind == GroundingKind::Bet {
            let by = match &bet.added {
                GroundingAdded::AtCapture => format!(
                    "declared at capture by {}",
                    brief
                        .decided_by
                        .decider_ids
                        .first()
                        .or(brief.decided_by.proposer_id.as_ref())
                        .map_or("unknown", String::as_str)
                ),
                later @ GroundingAdded::Later { .. } => later.describe(),
            };
            let state = match &bet.state {
                GroundingItemState::BetOpen { check_by, overdue } => match check_by {
                    Some(check_by) if *overdue => {
                        format!("check by {} (OVERDUE)", check_by.format("%Y-%m-%d"))
                    }
                    Some(check_by) => format!("check by {}", check_by.format("%Y-%m-%d")),
                    None => "no check date".to_owned(),
                },
                other => other.describe().unwrap_or_default(),
            };
            let _ = writeln!(
                output,
                "  rests on: a bet, {by}: \"{}\", {state}",
                summary_cell(&bet.label)
            );
            return;
        }
    }
    let _ = writeln!(output, "  rests on:");
    for item in &brief.rests_on {
        let _ = writeln!(output, "    {}", grounding_line(item));
    }
}

fn grounding_line(item: &GroundingItem) -> String {
    let kind = match item.kind {
        GroundingKind::Decision => "decision",
        GroundingKind::Evidence => "evidence",
        GroundingKind::Assumption => "assumption",
        GroundingKind::Bet => "bet",
    };
    let mut line = format!("{kind:<10} \"{}\"", summary_cell(&item.label));
    if let GroundingItemState::Recorded {
        source: Some(source),
        ..
    } = &item.state
    {
        // A document import stores its whole source reference as JSON; that is a handle for
        // machines, not a place a person can go.
        if !source.trim_start().starts_with('{') {
            line.push_str(&format!(" ({})", summary_cell(source)));
        }
    }
    if let Some(state) = item.state.describe() {
        line.push_str(&format!("  {}", summary_cell(&state)));
    }
    line.push_str(&format!("  ({})", item.added.describe()));
    line
}

/// The compact view's one-line answer to "what does this rest on?": the digest clause, or the
/// honest "nothing declared" with the way to add it.
fn compact_rests_on(view: &CompactView) -> String {
    if view.grounding_state == GroundingState::NothingDeclared {
        format!(
            "rests on: nothing declared (never asked; add with hivemind ground \"{}\" --rests-on-decision \"...\")",
            summary_cell(&view.decision.title)
        )
    } else {
        view.decision.rests_on_clause()
    }
}

/// Renders a resolve-by-description outcome for a fluent verb: an unambiguous match, a numbered
/// candidate list to disambiguate via `--pick N` / `#N` / `--id`, or a not-found notice.
pub(crate) fn render_resolve_outcome_summary(outcome: &ResolveOutcome) -> String {
    match outcome {
        ResolveOutcome::Resolved { candidate } => format!(
            "resolved\t{}\t{}",
            candidate.decision_id,
            summary_cell(&candidate.title)
        ),
        ResolveOutcome::Ambiguous { candidates } => {
            let mut output = String::new();
            if candidates
                .iter()
                .all(|candidate| !candidate.missing_terms.is_empty())
            {
                let _ = writeln!(
                    output,
                    "close: no decision matches every word; {} close candidates — resolve with --pick N, #N, or --id",
                    candidates.len()
                );
            } else {
                let _ = writeln!(
                    output,
                    "ambiguous: {} candidates match — resolve with --pick N, #N, or --id",
                    candidates.len()
                );
            }
            for (index, candidate) in candidates.iter().enumerate() {
                let _ = write!(
                    output,
                    "#{}\t{}\t{}",
                    index + 1,
                    candidate.decision_id,
                    summary_cell(&candidate.title)
                );
                if !candidate.missing_terms.is_empty() {
                    let _ = write!(output, "\tmissing: {}", candidate.missing_terms.join(" "));
                }
                output.push('\n');
            }
            output.trim_end().to_owned()
        }
        ResolveOutcome::NotFound => "no decision matches that description".to_owned(),
    }
}

/// Leads with the root decision's answer (the same block `verify` prints: title, rationale,
/// chosen and rejected options, who decided, whether it still holds), then the graph as
/// tab-separated `root`/`node`/`edge` lines. Nodes carry a trailing `label=` when they have one,
/// and decision nodes a trailing `project=`; the root's project is the brief's `project:` line.
pub(crate) fn render_neighborhood_summary(neighborhood: &NeighborhoodView) -> String {
    let mut output = String::new();
    if let Some(brief) = &neighborhood.root.brief {
        write_decision_brief(&mut output, brief);
    }
    let _ = writeln!(
        output,
        "root\t{}\t{}\tpresent={}\tnodes={}\tedges={}",
        neighborhood.root.kind.table_name(),
        neighborhood.root.id,
        neighborhood.root.present,
        neighborhood.nodes.len(),
        neighborhood.edges.len()
    );
    for node in &neighborhood.nodes {
        let status = match (node.decision_status, node.hypothesis_status) {
            (Some(status), _) => decision_status_label(status),
            (None, Some(status)) => hypothesis_status_label(status),
            (None, None) => "",
        };
        let _ = write!(
            output,
            "node\t{}\t{}\tstatus={}",
            node.kind.table_name(),
            node.id,
            status
        );
        if let Some(label) = &node.label {
            let _ = write!(output, "\tlabel={}", summary_cell(label));
        }
        write_project_field(&mut output, node.project_label.as_deref());
        output.push('\n');
    }
    // `from`/`to` are the arrow, newer -> older; `label` reads the relation along it and
    // `reversed` says the arrow runs against the stored direction (docs/GRAPH_CONTRACT.md).
    for edge in &neighborhood.edges {
        let event_origin = edge
            .event_origin
            .map_or_else(|| "unknown".to_owned(), |origin| origin.to_string());
        let _ = writeln!(
            output,
            "edge\t{}\t{}\t{}\tevent_origin={}\tlabel={}\treversed={}",
            edge.relation.table_name(),
            edge.from,
            edge.to,
            event_origin,
            edge.label,
            edge.reversed
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_active_blockers_summary(results: &DecisionBlockerResults) -> String {
    if results.items.is_empty() {
        return "No active decision blockers found".to_owned();
    }

    let mut output = String::new();
    for blocker in &results.items {
        let decision_id = match &blocker.decision_id {
            Some(decision_id) => decision_id.as_str(),
            None => "",
        };
        let _ = writeln!(
            output,
            "blocker\t{}\tdecision={}\tpriority={}\tstale={}\tblocked_actor={}\t{}",
            blocker.id,
            decision_id,
            blocker.priority.as_str(),
            blocker.stale,
            blocker.blocked_actor_id,
            summary_cell(&blocker.reason)
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_blocker_notifications_summary(
    candidates: &BlockerNotificationCandidates,
) -> String {
    if candidates.items.is_empty() {
        return "No blocker notification candidates found".to_owned();
    }

    let mut output = String::new();
    for candidate in &candidates.items {
        let decision_id = match &candidate.decision_id {
            Some(decision_id) => decision_id.as_str(),
            None => "",
        };
        let _ = writeln!(
            output,
            "notification\tblocker={}\tdecision={}\tpriority={}\trecipient={}\tchannel={}",
            candidate.blocker_id,
            decision_id,
            candidate.priority.as_str(),
            candidate.recipient_actor_id,
            candidate.channel
        );
    }
    output.trim_end().to_owned()
}

fn write_decision_summary_row(output: &mut String, prefix: &str, decision: &DecisionView) {
    let _ = writeln!(
        output,
        "{}\t{}\t{}\t{}\ttopics={}\tproject={}",
        prefix,
        decision_status_label(decision.status),
        decision.id,
        summary_cell(&decision.title),
        decision.topic_keys.join(","),
        summary_cell(&decision.project_label)
    );
}

/// A trailing `project=<label>` field on a tab-separated row; nothing when the row's node has no
/// project (a neighborhood node that is not a decision).
fn write_project_field(output: &mut String, project_label: Option<&str>) {
    if let Some(label) = project_label {
        let _ = write!(output, "\tproject={}", summary_cell(label));
    }
}

fn summary_cell(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

pub(crate) fn decision_status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn hypothesis_status_label(status: HypothesisStatus) -> &'static str {
    match status {
        HypothesisStatus::Open => "open",
        HypothesisStatus::Supported => "supported",
        HypothesisStatus::Refuted => "refuted",
    }
}

fn event_type_label(event_type: EventType) -> &'static str {
    match event_type {
        EventType::DecisionProposed => "decision.proposed",
        EventType::DecisionRequested => "decision.requested",
        EventType::DecisionAccepted => "decision.accepted",
        EventType::DecisionRejected => "decision.rejected",
        EventType::DecisionSuperseded => "decision.superseded",
        EventType::EvidenceRecorded => "evidence.recorded",
        EventType::HypothesisRecorded => "hypothesis.recorded",
        EventType::RelationAdded => "relation.added",
        EventType::RelationRemoved => "relation.removed",
        EventType::BlockerReported => "blocker.reported",
        EventType::BlockerResolved => "blocker.resolved",
        EventType::NotificationSent => "notification.sent",
        EventType::NotificationAcknowledged => "notification.acknowledged",
        EventType::IngestBatchReceived => "ingest.batch_received",
        EventType::IngestBatchClassified => "ingest.batch_classified",
        EventType::DecisionScored => "decision.scored",
        EventType::DecisionMetadataDerived => "decision.metadata_derived",
        EventType::DecisionMoved => "decision.moved",
        EventType::ProjectRegistered => "project.registered",
        EventType::ProjectLinked => "project.linked",
        EventType::ProjectUnlinked => "project.unlinked",
        EventType::ProjectAnchored => "project.anchored",
        EventType::ProjectUnanchored => "project.unanchored",
    }
}

fn change_kind_label(kind: HistoryChangeKind) -> &'static str {
    match kind {
        HistoryChangeKind::NewDecision => "new_decision",
        HistoryChangeKind::StatusChange => "status_change",
        HistoryChangeKind::NewEvidence => "new_evidence",
        HistoryChangeKind::StalePremise => "stale_premise",
        HistoryChangeKind::Supersession => "supersession",
        HistoryChangeKind::ProjectMoved => "project_moved",
        HistoryChangeKind::ContextChange => "context_change",
    }
}

fn read_only_query_label(query: ReadOnlyExportQueryKind) -> &'static str {
    match query {
        ReadOnlyExportQueryKind::RecentActivity => "recent_activity",
        ReadOnlyExportQueryKind::DecisionsChangedSince => "decisions_changed_since",
    }
}

fn read_only_format_label(format: QueryReadOnlyExportFormat) -> &'static str {
    match format {
        QueryReadOnlyExportFormat::Json => "json",
        QueryReadOnlyExportFormat::Markdown => "markdown",
    }
}

pub(crate) fn format_output(as_json: bool, envelope: &OutputEnvelope) -> Result<String> {
    if as_json {
        serde_json::to_string(envelope).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        // Text stays the bare value: scripts take `$(hivemind emit ...)` as the id. A capture's
        // project is announced on stderr instead (`render_placement_line`).
        Ok(envelope.value.clone())
    }
}

/// The text-mode announcement of where a capture landed: the address and how it was
/// determined, and on personal fallback the "saved to your personal project" sentence.
/// Written to stderr so stdout keeps its machine-readable shape.
pub(crate) fn render_placement_line(placement: &DecisionPlacement) -> String {
    let mut line = format!(
        "project: {} ({})",
        placement.project,
        placement.project_source.as_str()
    );
    if let Some(notice) = placement.notice() {
        let _ = write!(line, " — {notice}");
    }
    line
}

pub(crate) fn format_disagree_output(
    as_json: bool,
    output: &DisagreeCommandOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    Ok(format!(
        "event_id={} decision_id={} status={}",
        output.event_id,
        output.decision_id,
        decision_status_label(output.decision_status)
    ))
}

pub(crate) fn format_move_output(as_json: bool, output: &DecisionMoveOutcome) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    Ok(format!(
        "event_id={} decision_id={} from={} to={}",
        output.event_id, output.decision_id, output.from, output.to
    ))
}

pub(crate) fn format_supersede_output(
    as_json: bool,
    output: &SupersedeCommandOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    let mut rendered = format!(
        "proposal_event_id={} superseded_event_id={} old_decision_id={} new_decision_id={} old_status={} new_status={} project={} project_source={}",
        output.proposal_event_id,
        output.superseded_event_id,
        output.old_decision_id,
        output.new_decision_id,
        decision_status_label(output.old_decision_status),
        decision_status_label(output.new_decision_status),
        output.placement.project,
        output.placement.project_source.as_str()
    );
    if !output.premise_stale.is_empty() {
        let _ = write!(
            rendered,
            " premise_stale={}",
            output.premise_stale.join(",")
        );
    }
    Ok(rendered)
}

/// `emit decision.capture`'s reply. Text mode stays the bare decision id every script already
/// parses; a premise that is already superseded or rejected is the one thing that must never be
/// silent, so it adds a line. `--json` carries the full `rests_on`.
pub(crate) fn format_capture_output(
    as_json: bool,
    output: &CaptureCommandOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    let mut rendered = output.value.clone();
    for premise_id in &output.premise_stale {
        let _ = write!(
            rendered,
            "\npremise_stale: {premise_id} (already superseded or rejected; the link is recorded)"
        );
    }
    Ok(rendered)
}

/// `ground`'s reply: what was added, with the decision it was added to. Text shows labels,
/// `--json` keeps the ids too. A premise that is already superseded or rejected is never silent.
pub(crate) fn format_ground_output(as_json: bool, output: &GroundCommandOutput) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    let mut rendered = format!("grounded {}", output.decision_id);
    if let Some(title) = &output.decision_title {
        let _ = write!(rendered, " \"{title}\"");
    }
    let _ = write!(
        rendered,
        ": added {} thing(s) it rests on, attributed to {}",
        output.rests_on.len(),
        output.actor_id
    );
    for item in &output.rests_on {
        let kind = match item.kind {
            RestsOnKind::Decision => "decision",
            RestsOnKind::Evidence => "evidence",
            RestsOnKind::Assumption => "assumption",
            RestsOnKind::Bet => "bet",
        };
        match &item.label {
            Some(label) => {
                let _ = write!(rendered, "\n  {kind} \"{label}\" ({})", item.id);
            }
            None => {
                let _ = write!(rendered, "\n  {kind} {}", item.id);
            }
        }
    }
    for premise_id in &output.premise_stale {
        let _ = write!(
            rendered,
            "\npremise_stale: {premise_id} (already superseded or rejected; the link is recorded)"
        );
    }
    Ok(rendered)
}

pub(crate) fn format_review_output(as_json: bool, output: &ReviewCommandOutput) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    let mut rendered = String::new();
    let _ = writeln!(
        rendered,
        "reviewer={} matched={} reviewed={} skipped={} quit={} truncated={}",
        output.reviewer_actor_id,
        output.matched_count,
        output.reviewed_count,
        output.skipped_count,
        output.quit,
        output.truncated
    );
    if let Some(next_cursor) = &output.next_cursor {
        let _ = writeln!(rendered, "next_cursor={next_cursor}");
    }
    for action in &output.actions {
        let _ = write!(
            rendered,
            "{} decision_id={}",
            action.action, action.decision_id
        );
        if let Some(event_id) = action.event_id {
            let _ = write!(rendered, " event_id={event_id}");
        }
        if let Some(proposal_event_id) = action.proposal_event_id {
            let _ = write!(rendered, " proposal_event_id={proposal_event_id}");
        }
        if let Some(superseded_event_id) = action.superseded_event_id {
            let _ = write!(rendered, " superseded_event_id={superseded_event_id}");
        }
        if let Some(new_decision_id) = &action.new_decision_id {
            let _ = write!(rendered, " new_decision_id={new_decision_id}");
        }
        if let Some(old_status) = action.old_decision_status {
            let _ = write!(
                rendered,
                " old_status={}",
                decision_status_label(old_status)
            );
        }
        if let Some(new_status) = action.new_decision_status {
            let _ = write!(
                rendered,
                " new_status={}",
                decision_status_label(new_status)
            );
        }
        rendered.push('\n');
    }
    Ok(rendered.trim_end().to_owned())
}

pub(crate) fn format_json_value<T: Serialize>(compact: bool, value: &T) -> Result<String> {
    if compact {
        serde_json::to_string(value).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        serde_json::to_string_pretty(value).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    }
}

pub(crate) fn format_import_output(as_json: bool, report: &DocumentImportReport) -> Result<String> {
    if as_json {
        serde_json::to_string(report).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        let mut out = format!(
            "import_run_id={} files_seen={} blocks_imported={} no_op={} conflicts={} resolved={} duplicate_candidates={} validation_errors={} events_written={}",
            report.import_run_id,
            report.summary.files_seen,
            report.summary.blocks_imported,
            report.summary.blocks_noop,
            report.summary.blocks_conflicted,
            report.summary.blocks_resolved,
            report.summary.duplicate_candidates,
            report.summary.validation_errors,
            report.summary.events_written
        );
        if report.summary.prose_candidates_proposed > 0 {
            use std::fmt::Write as _;
            let _ = write!(
                out,
                " prose_candidates_proposed={}",
                report.summary.prose_candidates_proposed
            );
        }
        Ok(out)
    }
}

pub(crate) fn format_prepare_documents_output(
    as_json: bool,
    report: &DocumentPreparationReport,
) -> Result<String> {
    if as_json {
        serde_json::to_string(report).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        })
    } else {
        Ok(format!(
            "preparation_run_id={} files_seen={} files_prepared={} review_required={} needs_ocr={} validation_errors={} pages_seen={} bytes_written={}",
            report.preparation_run_id,
            report.summary.files_seen,
            report.summary.files_prepared,
            report.summary.files_review_required,
            report.summary.files_needing_ocr,
            report.summary.validation_errors,
            report.summary.pages_seen,
            report.summary.bytes_written
        ))
    }
}

pub(crate) fn format_export_output(as_json: bool, report: &ExportReport) -> Result<String> {
    if as_json {
        return serde_json::to_string(report).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}")).into()
        });
    }
    Ok(match report {
        ExportReport::Exported {
            out_dir,
            ledger_offset,
            files_written,
            files_removed,
        } => format!(
            "out_dir={out_dir} ledger_offset={ledger_offset} files_written={files_written} files_removed={files_removed}"
        ),
        ExportReport::NotFound { project } => format!("outcome=not_found project={project}"),
    })
}

pub fn exit_code_for_error(error: &HivemindError) -> CliExit {
    match error {
        HivemindError::Cli(_) => CliExit::Validation,
        HivemindError::Command(CommandError::Validation(_)) => CliExit::Validation,
        HivemindError::Command(CommandError::Invariant(_)) => CliExit::Invariant,
        HivemindError::Ledger(_) | HivemindError::Projector(_) => CliExit::Storage,
        HivemindError::Query(_) => CliExit::Generic,
    }
}

pub fn format_error(as_json: bool, error: &HivemindError) -> String {
    if as_json {
        serde_json::json!({
            "error": {
                "message": error.to_string(),
                "exit_code": exit_code_for_error(error).code()
            }
        })
        .to_string()
    } else {
        format!("error: {error}")
    }
}

pub fn render_decision_dot(graph: &impl GraphView) -> Result<String> {
    render_dot(graph)
}

pub(crate) fn render_dot(graph: &impl GraphView) -> Result<String> {
    let mut dot = String::from("digraph hivemind {\n  rankdir=LR;\n");
    let nodes = graph_nodes(graph)?;
    let edges = graph_edges(graph)?;

    for ((kind, id), properties) in &nodes {
        let label = match kind {
            NodeKind::Decision => {
                let title =
                    graph_property_string(properties, "title").unwrap_or_else(|| id.clone());
                let status = decision_status_name(derive_decision_status(graph, id)?);
                label_with_status(&title, status)
            }
            NodeKind::DecisionRequest => graph_property_string(properties, "reason")
                .map(|reason| prefixed_dot_label("Decision request", &reason))
                .unwrap_or_else(|| id.clone()),
            NodeKind::Hypothesis => {
                let statement =
                    graph_property_string(properties, "statement").unwrap_or_else(|| id.clone());
                let status = hypothesis_status_name(derive_hypothesis_status(graph, id)?);
                label_with_status(&statement, status)
            }
            NodeKind::Blocker => graph_property_string(properties, "reason")
                .map(|reason| prefixed_dot_label("Blocker", &reason))
                .unwrap_or_else(|| id.clone()),
            NodeKind::Notification => graph_property_string(properties, "channel")
                .map(|channel| prefixed_dot_label("Notification", &channel))
                .unwrap_or_else(|| id.clone()),
            _ => graph_property_string(properties, "content")
                .or_else(|| graph_property_string(properties, "label"))
                .unwrap_or_else(|| id.clone()),
        };

        let _ = writeln!(
            dot,
            "  \"{}\" [label=\"{}\", shape=box, style=filled, fillcolor=\"{}\"];",
            node_key(*kind, id),
            escape_dot(&label),
            node_color(*kind)
        );
    }

    for edge in &edges {
        let _ = writeln!(
            dot,
            "  \"{}\" -> \"{}\" [label=\"{}\"];",
            node_key(edge.from_kind, &edge.from_id),
            node_key(edge.to_kind, &edge.to_id),
            edge.label
        );
    }

    dot.push_str("}\n");
    Ok(dot)
}

fn label_with_status(label: &str, status: &str) -> String {
    let mut output = String::with_capacity(label.len() + status.len() + "\\nstatus: ".len());
    output.push_str(label);
    output.push_str("\\nstatus: ");
    output.push_str(status);
    output
}

fn prefixed_dot_label(prefix: &str, value: &str) -> String {
    let mut output = String::with_capacity(prefix.len() + value.len() + 2);
    output.push_str(prefix);
    output.push_str("\\n");
    output.push_str(value);
    output
}

fn graph_nodes(graph: &impl GraphView) -> Result<BTreeMap<(NodeKind, String), GraphProperties>> {
    let mut nodes = BTreeMap::new();
    for kind in NodeKind::ALL {
        let rows = graph.query(&node_dump_query(kind), &GraphParams::new())?;
        for row in rows {
            let id = required_row_string(&row, "id")?;
            nodes.insert((kind, id), node_properties_from_row(kind, &row));
        }
    }
    Ok(nodes)
}

/// Every edge as an arrow, newer -> older (docs/GRAPH_CONTRACT.md), so the DOT export draws the
/// same direction and label as `GET /v1/graph`.
fn graph_edges(graph: &impl GraphView) -> Result<BTreeSet<DotEdge>> {
    Ok(oriented_edges(graph)?
        .into_iter()
        .map(|arrow| DotEdge {
            relation: arrow.relation,
            from_kind: arrow.from_kind,
            from_id: arrow.from_id,
            to_kind: arrow.to_kind,
            to_id: arrow.to_id,
            label: arrow.label,
        })
        .collect())
}

fn node_dump_query(kind: NodeKind) -> String {
    let projection = match kind {
        NodeKind::Decision => {
            "node.id AS id, node.title AS title, node.rationale AS rationale, node.topic_keys AS topic_keys, node.quote AS quote, node.question AS question"
        }
        NodeKind::DecisionRequest => {
            "node.id AS id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.reason AS reason, node.priority AS priority, node.required_owner_id AS required_owner_id, node.authority_class AS authority_class, node.requested_by AS requested_by, node.client_request_id AS client_request_id"
        }
        NodeKind::Actor => "node.id AS id",
        NodeKind::Blocker => {
            "node.id AS id, node.blocked_actor_id AS blocked_actor_id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.blocked_ref AS blocked_ref, node.blocked_ref_type AS blocked_ref_type, node.reason AS reason, node.priority AS priority, node.last_progress_at AS last_progress_at, node.required_owner_id AS required_owner_id"
        }
        NodeKind::Evidence => "node.id AS id, node.content AS content",
        NodeKind::Notification => {
            "node.id AS id, node.blocker_id AS blocker_id, node.recipient_actor_id AS recipient_actor_id, node.channel AS channel, node.threshold_rule AS threshold_rule, node.source_event_ids AS source_event_ids, node.dedupe_key AS dedupe_key, node.sent_at AS sent_at"
        }
        NodeKind::Option => {
            "node.id AS id, node.label AS label, node.description AS description"
        }
        NodeKind::Hypothesis => "node.id AS id, node.statement AS statement",
        NodeKind::Project => {
            "node.id AS id, node.handle AS handle, node.display_name AS display_name, node.purpose AS purpose, node.anchors AS anchors"
        }
    };
    format!(
        "MATCH (node:`{}`) RETURN {projection} ORDER BY node.id;",
        kind.table_name()
    )
}

fn node_properties_from_row(kind: NodeKind, row: &GraphRow) -> GraphProperties {
    let mut properties = GraphProperties::new();
    match kind {
        NodeKind::Decision => {
            insert_if_present(&mut properties, row, "title");
            insert_if_present(&mut properties, row, "rationale");
            insert_if_present(&mut properties, row, "topic_keys");
            insert_if_present(&mut properties, row, "quote");
            insert_if_present(&mut properties, row, "question");
        }
        NodeKind::DecisionRequest => {
            insert_if_present(&mut properties, row, "decision_id");
            insert_if_present(&mut properties, row, "topic_keys");
            insert_if_present(&mut properties, row, "reason");
            insert_if_present(&mut properties, row, "priority");
            insert_if_present(&mut properties, row, "required_owner_id");
            insert_if_present(&mut properties, row, "authority_class");
            insert_if_present(&mut properties, row, "requested_by");
            insert_if_present(&mut properties, row, "client_request_id");
        }
        NodeKind::Actor => {}
        NodeKind::Blocker => {
            insert_if_present(&mut properties, row, "blocked_actor_id");
            insert_if_present(&mut properties, row, "decision_id");
            insert_if_present(&mut properties, row, "topic_keys");
            insert_if_present(&mut properties, row, "blocked_ref");
            insert_if_present(&mut properties, row, "blocked_ref_type");
            insert_if_present(&mut properties, row, "reason");
            insert_if_present(&mut properties, row, "priority");
            insert_if_present(&mut properties, row, "last_progress_at");
            insert_if_present(&mut properties, row, "required_owner_id");
        }
        NodeKind::Evidence => insert_if_present(&mut properties, row, "content"),
        NodeKind::Notification => {
            insert_if_present(&mut properties, row, "blocker_id");
            insert_if_present(&mut properties, row, "recipient_actor_id");
            insert_if_present(&mut properties, row, "channel");
            insert_if_present(&mut properties, row, "threshold_rule");
            insert_if_present(&mut properties, row, "source_event_ids");
            insert_if_present(&mut properties, row, "dedupe_key");
            insert_if_present(&mut properties, row, "sent_at");
        }
        NodeKind::Option => {
            insert_if_present(&mut properties, row, "label");
            insert_if_present(&mut properties, row, "description");
        }
        NodeKind::Hypothesis => insert_if_present(&mut properties, row, "statement"),
        NodeKind::Project => {
            insert_if_present(&mut properties, row, "handle");
            insert_if_present(&mut properties, row, "display_name");
            insert_if_present(&mut properties, row, "purpose");
            insert_if_present(&mut properties, row, "anchors");
        }
    }
    properties
}

fn insert_if_present(properties: &mut GraphProperties, row: &GraphRow, key: &str) {
    if let Some(value) = row.get(key) {
        properties.insert(key.to_owned(), value.clone());
    }
}

fn graph_property_string(properties: &GraphProperties, key: &str) -> Option<String> {
    match properties.get(key) {
        Some(GraphValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn node_key(kind: NodeKind, id: &str) -> String {
    format!("{}:{}", kind.table_name(), id)
}

fn node_color(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Decision => "#d6eaf8",
        NodeKind::DecisionRequest => "#d7bde2",
        NodeKind::Actor => "#d5f5e3",
        NodeKind::Blocker => "#f5b7b1",
        NodeKind::Evidence => "#fcf3cf",
        NodeKind::Notification => "#d2b4de",
        NodeKind::Option => "#f9e79f",
        NodeKind::Hypothesis => "#f5cba7",
        NodeKind::Project => "#aed6f1",
    }
}

fn decision_status_name(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn hypothesis_status_name(status: HypothesisStatus) -> &'static str {
    match status {
        HypothesisStatus::Open => "open",
        HypothesisStatus::Supported => "supported",
        HypothesisStatus::Refuted => "refuted",
    }
}

fn escape_dot(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn required_row_string(row: &GraphRow, key: &str) -> Result<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Ok(value.clone()),
        _ => Err(CliError::InvalidInput(format!("row missing string field: {key}")).into()),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DotEdge {
    relation: GraphRelationKind,
    from_kind: NodeKind,
    from_id: String,
    to_kind: NodeKind,
    to_id: String,
    /// The relation read along the arrow, an active phrase.
    label: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutputEnvelope {
    pub(crate) subcommand: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) value: String,
    /// Where a captured decision was filed (`project`, `project_source`), for the emit
    /// verbs that record one; absent on every other envelope.
    #[serde(flatten)]
    pub(crate) placement: Option<DecisionPlacement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_notice: Option<&'static str>,
    /// Set when `--project-from-context` found no project to attach the capture to: the
    /// folder is not attached, and how to attach it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_reminder: Option<&'static str>,
}

impl OutputEnvelope {
    pub(crate) fn new(subcommand: &'static str, kind: &'static str, value: String) -> Self {
        Self {
            subcommand,
            kind,
            value,
            placement: None,
            project_notice: None,
            project_reminder: None,
        }
    }

    pub(crate) fn with_placement(mut self, placement: DecisionPlacement) -> Self {
        self.project_notice = placement.notice();
        self.placement = Some(placement);
        self
    }

    pub(crate) fn with_project_reminder(mut self, reminder: Option<&'static str>) -> Self {
        self.project_reminder = reminder;
        self
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct DisagreeCommandOutput {
    pub(crate) decision_id: String,
    pub(crate) event_id: EventId,
    pub(crate) decision_status: DecisionStatus,
}

#[derive(Debug, Serialize)]
pub(crate) struct SupersedeCommandOutput {
    pub(crate) old_decision_id: String,
    pub(crate) new_decision_id: String,
    pub(crate) proposal_event_id: EventId,
    pub(crate) relation_event_ids: Vec<EventId>,
    pub(crate) superseded_event_id: EventId,
    pub(crate) old_decision_status: DecisionStatus,
    pub(crate) new_decision_status: DecisionStatus,
    /// Where the superseding decision was filed (`project`, `project_source`).
    #[serde(flatten)]
    pub(crate) placement: DecisionPlacement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_notice: Option<&'static str>,
    /// See `OutputEnvelope::project_reminder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_reminder: Option<&'static str>,
    /// What the new decision rests on, as recorded.
    pub(crate) rests_on: Vec<RestsOn>,
    /// Premise decisions already superseded or rejected when named.
    pub(crate) premise_stale: Vec<String>,
}

/// The `emit decision.capture` reply: the `OutputEnvelope` fields plus what the decision rests
/// on and which premises were already stale.
#[derive(Debug, Serialize)]
pub(crate) struct CaptureCommandOutput {
    pub(crate) subcommand: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) value: String,
    /// Where the decision was filed (`project`, `project_source`).
    #[serde(flatten)]
    pub(crate) placement: DecisionPlacement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_notice: Option<&'static str>,
    /// See `OutputEnvelope::project_reminder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_reminder: Option<&'static str>,
    pub(crate) rests_on: Vec<RestsOn>,
    pub(crate) premise_stale: Vec<String>,
}

/// The `ground` reply: the decision that was grounded and what it now rests on.
#[derive(Debug, Serialize)]
pub(crate) struct GroundCommandOutput {
    pub(crate) decision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decision_title: Option<String>,
    /// Who the grounding is attributed to.
    pub(crate) actor_id: String,
    pub(crate) relation_event_ids: Vec<EventId>,
    /// What was added, as recorded.
    pub(crate) rests_on: Vec<RestsOn>,
    /// Premise decisions already superseded or rejected when named.
    pub(crate) premise_stale: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectRegisterOutput {
    pub(crate) event_id: EventId,
    pub(crate) handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) purpose: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectLinkOutput {
    pub(crate) event_id: EventId,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) kind: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProjectAnchorOutput {
    pub(crate) event_id: EventId,
    pub(crate) handle: String,
    pub(crate) anchor_kind: &'static str,
    pub(crate) value: String,
}

pub(crate) fn format_project_register_output(
    as_json: bool,
    output: &ProjectRegisterOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }
    let mut rendered = format!("event_id={} handle={}", output.event_id, output.handle);
    if let Some(name) = &output.display_name {
        let _ = write!(rendered, " display_name={}", summary_cell(name));
    }
    if let Some(purpose) = &output.purpose {
        let _ = write!(rendered, " purpose={}", summary_cell(purpose));
    }
    Ok(rendered)
}

pub(crate) fn format_project_link_output(
    as_json: bool,
    output: &ProjectLinkOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }
    Ok(format!(
        "event_id={} from={} to={} kind={}",
        output.event_id, output.from, output.to, output.kind
    ))
}

pub(crate) fn format_project_anchor_output(
    as_json: bool,
    output: &ProjectAnchorOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }
    Ok(format!(
        "event_id={} handle={} anchor_kind={} value={}",
        output.event_id,
        output.handle,
        output.anchor_kind,
        summary_cell(&output.value)
    ))
}

pub(crate) fn format_project_list_output(
    as_json: bool,
    response: &QueryResponse<ProjectListResults>,
) -> Result<String> {
    if as_json {
        return format_json_value(true, response);
    }
    let mut output = render_project_list_summary(&response.data);
    append_truncation_notice(
        &mut output,
        response.truncated,
        response.data.next_cursor.as_deref(),
    );
    Ok(output)
}

pub(crate) fn render_project_list_summary(results: &ProjectListResults) -> String {
    if results.items.is_empty() {
        return "No projects registered".to_owned();
    }
    let mut output = String::new();
    for project in &results.items {
        let _ = writeln!(
            output,
            "project\t{}\t{}\tanchors={}\tpart_of={}\tdepends_on={}",
            project.handle,
            summary_cell(project.display_name.as_deref().unwrap_or("-")),
            project.anchors.len(),
            project
                .part_of
                .as_ref()
                .map_or("-", |fact| fact.to.as_str()),
            project.depends_on.len(),
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn format_project_show_output(
    as_json: bool,
    response: &QueryResponse<ProjectOutcome>,
) -> Result<String> {
    if as_json {
        return format_json_value(true, response);
    }
    Ok(render_project_outcome_summary(&response.data))
}

pub(crate) fn format_project_decisions_output(
    as_json: bool,
    response: &QueryResponse<ProjectDecisionsOutcome>,
) -> Result<String> {
    if as_json {
        return format_json_value(true, response);
    }
    let mut output = render_project_decisions_summary(&response.data);
    if let ProjectDecisionsOutcome::Found(page) = &response.data {
        append_truncation_notice(&mut output, response.truncated, page.next_cursor.as_deref());
    }
    Ok(output)
}

/// A project's decision list: a header that says whose or which project this is and how many
/// decisions are in it (all of them, not just this page), then one row per decision.
pub(crate) fn render_project_decisions_summary(outcome: &ProjectDecisionsOutcome) -> String {
    let page = match outcome {
        ProjectDecisionsOutcome::NotFound { handle } => {
            return format!(
                "no project called '{handle}': run `hivemind project register {handle}` to register it"
            );
        }
        ProjectDecisionsOutcome::Found(page) => page,
    };

    let mut output = project_decisions_header(page);
    for item in &page.items {
        let _ = write!(
            output,
            "\n{}\t{}\t{}\tactor={}",
            decision_status_label(item.status),
            item.decision_id,
            summary_cell(&item.title),
            item.proposed_by.as_deref().unwrap_or("-"),
        );
        if let Some(session) = &item.session {
            let _ = write!(output, "\tsession={}", summary_cell(session));
        }
        let _ = write!(
            output,
            "\tproject_source={}",
            item.project_source.as_deref().unwrap_or("-")
        );
    }
    output
}

fn project_decisions_header(page: &ProjectDecisionsPage) -> String {
    match &page.owner {
        Some(owner) => format!(
            "in {}'s personal project, not yet shared: {}",
            summary_cell(owner),
            page.total_matches
        ),
        None => format!("in project {}: {}", page.handle, page.total_matches),
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct CurrentProjectOutput {
    pub(crate) actor: String,
    pub(crate) tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) handle: Option<String>,
}

pub(crate) fn format_current_project_output(
    as_json: bool,
    output: &CurrentProjectOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }
    Ok(format!(
        "actor={}\ttenant={}\thandle={}",
        output.actor,
        output.tenant,
        output.handle.as_deref().unwrap_or("(none)")
    ))
}

pub(crate) fn render_project_outcome_summary(outcome: &ProjectOutcome) -> String {
    match outcome {
        ProjectOutcome::NotFound => "outcome=not_found".to_owned(),
        ProjectOutcome::Found { project } => format!(
            "outcome=found\thandle={}\tpersonal={}\tdisplay_name={}\tanchors={}\tpart_of={}\tdepends_on={}",
            project.handle,
            project.personal,
            summary_cell(project.display_name.as_deref().unwrap_or("-")),
            project.anchors.len(),
            project
                .part_of
                .as_ref()
                .map_or("-", |fact| fact.to.as_str()),
            project.depends_on.len(),
        ),
    }
}

/// A miss is data (Alex's rule, 2026-09-20): `--project` naming an unknown handle is a
/// successful `not_found` envelope that wrote nothing, like `project show`.
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum ExportReport {
    Exported {
        out_dir: String,
        ledger_offset: EventId,
        files_written: usize,
        files_removed: usize,
    },
    NotFound {
        project: String,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct ReviewCommandOutput {
    pub(crate) reviewer_actor_id: String,
    pub(crate) matched_count: usize,
    pub(crate) reviewed_count: usize,
    pub(crate) skipped_count: usize,
    pub(crate) quit: bool,
    pub(crate) truncated: bool,
    pub(crate) next_cursor: Option<String>,
    pub(crate) unreviewed_only: bool,
    pub(crate) reviewed_semantics: &'static str,
    pub(crate) actions: Vec<ReviewActionOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReviewActionOutput {
    pub(crate) decision_id: String,
    pub(crate) action: &'static str,
    pub(crate) event_id: Option<EventId>,
    pub(crate) proposal_event_id: Option<EventId>,
    pub(crate) superseded_event_id: Option<EventId>,
    pub(crate) new_decision_id: Option<String>,
    pub(crate) old_decision_status: Option<DecisionStatus>,
    pub(crate) new_decision_status: Option<DecisionStatus>,
}

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Serialize)]
pub(crate) struct MigrateReport {
    pub(crate) dry_run: bool,
    pub(crate) source_dir: String,
    pub(crate) source_tenant: String,
    pub(crate) destination_tenant: String,
    pub(crate) events_migrated: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parity_check: Option<ParityCheckResult>,
}

#[cfg(feature = "shared-backend-postgres")]
#[derive(Debug, Serialize)]
pub(crate) struct ParityCheckResult {
    pub(crate) source_event_count: u64,
    pub(crate) destination_event_count: u64,
    pub(crate) ok: bool,
}

#[cfg(test)]
mod tests;
