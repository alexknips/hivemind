use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::commands::{
    DecisionMoveOutcome, DecisionPlacement, DecisionRetitleOutcome, ProjectTopicDeclaration,
    RestatedCapture, RestsOn, RestsOnKind,
};
use crate::error::{CliError, CommandError};
use crate::events::{EventId, EventType, ModelDimension};
use crate::importance::{Importance, ImportanceReport};
use crate::ingest::{DocumentImportReport, DocumentPreparationReport};
use crate::projector::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind,
    RelationKind as GraphRelationKind,
};
use crate::quality_profile::{Assessment, Dimension, ScanReport, ScoreReport};
use crate::queries::{
    annotate_resolution, derive_decision_status, derive_hypothesis_status, option_label_unit,
    oriented_edges, BlockerNotificationCandidates, ChangedDecisionsResults, CompactView, Contest,
    ContestedDecisionsResults, DecidedBy, DecisionBlockerResults, DecisionBrief,
    DecisionSearchResults, DecisionStatus, DecisionTimeline, DecisionView,
    DecisionsAddedSinceResults, DecisionsChangedSinceResults, DraftedFrom, GroundingAdded,
    GroundingItem, GroundingItemState, GroundingKind, GroundingState, HistoryChangeKind,
    HypothesisStatus, MatchReason, MisfiledDecisionCandidate, NeighborhoodView, OptionLabel,
    OutcomeReason, ProjectDecisionsOutcome, ProjectDecisionsPage, ProjectListResults, ProjectMove,
    ProjectOutcome, ProjectTopicFact, QueryResponse, QuestionAnswer, ReadOnlyExport,
    ReadOnlyExportFormat as QueryReadOnlyExportFormat, ReadOnlyExportQueryKind,
    RecentActivityResults, RecentDecisionsResults, ResolveOutcome, ResolvedCandidate, ReviewShape,
    SituationalResults, SupersessionChain, TimelineEntry, TimelineFact, TitleChange,
    WaitingRequestsResults, POLARITY_REASON,
};
use crate::restatement::{AppliedLink, RestatementProposals};
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
    if let Some(question) = &v.decision.question {
        out.push_str(&format!("  answers: {question}\n"));
    }
    if let Some(quote) = &v.decision.quote {
        out.push_str(&format!("  quote: \"{quote}\"\n"));
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
            format_args!(
                "{}{}",
                project_move_suffix(item.project_move.as_ref(), item.ts),
                title_change_suffix(item.title_change.as_ref(), item.ts)
            )
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
            format_args!(
                "{}{}",
                project_move_suffix(item.project_move.as_ref(), item.ts),
                title_change_suffix(item.title_change.as_ref(), item.ts)
            )
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

/// The tail of a `title_changed` history line: what the title became and when (mirrors
/// `project_move_suffix` exactly; hivemind-ydmp). Empty for every other kind of change.
fn title_change_suffix(title_change: Option<&TitleChange>, ts: Option<DateTime<Utc>>) -> String {
    let Some(title_change) = title_change else {
        return String::new();
    };
    let retitled_at = ts.map_or_else(|| "unknown".to_owned(), |ts| ts.to_rfc3339());
    format!(
        "\tretitled={}->{}\tat={retitled_at}",
        title_change.from, title_change.to
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

/// `format_query_response` for a read verb that resolved its target from a description. When the
/// description named a word the decision lacks, or the decision was recorded more than once, the
/// answer says so: `close match:` / `also recorded as:` lines ahead of `--summary` output,
/// `close_match` / `also_recorded_as` beside `data` in `--json` (hivemind-3lko, hivemind-83cj). A
/// full match of a decision recorded once (`None`) prints exactly what `format_query_response`
/// does.
pub(crate) fn format_close_matched_response<T: Serialize>(
    summary: bool,
    response: &QueryResponse<T>,
    close_match: Option<&ResolvedCandidate>,
    render_summary: impl FnOnce(&T) -> String,
) -> Result<String> {
    let Some(candidate) = close_match else {
        return format_query_response(summary, response, render_summary, None);
    };
    if summary {
        let body = format_query_response(true, response, render_summary, None)?;
        return Ok(format!("{}\n{body}", render_resolution_notices(candidate)));
    }
    let mut envelope = serde_json::to_value(response)
        .map_err(|error| CliError::InvalidInput(format!("json serialization failed: {error}")))?;
    annotate_resolution(&mut envelope, candidate);
    format_json_value(true, &envelope)
}

/// The lines that tell a reader how the decision below was resolved: the words it lacks, and the
/// other records of the same decision.
fn render_resolution_notices(candidate: &ResolvedCandidate) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !candidate.missing_terms.is_empty() {
        let words: Vec<String> = candidate
            .missing_terms
            .iter()
            .map(|term| format!("\"{term}\""))
            .collect();
        lines.push(format!(
            "close match: this one has no {} (name a decision exactly with --id)",
            words.join(", ")
        ));
    }
    if !candidate.also_recorded_as.is_empty() {
        let copies: Vec<String> = candidate
            .also_recorded_as
            .iter()
            .map(|copy| format!("{} \"{}\"", copy.decision_id, copy.title))
            .collect();
        lines.push(format!("also recorded as: {}", copies.join("; ")));
    }
    lines.join("\n")
}

/// The proposals as one line per link, then what was scanned and how to record them.
pub(crate) fn render_restatement_proposals(proposals: &RestatementProposals) -> String {
    let mut output = String::new();
    for link in &proposals.proposed {
        let _ = writeln!(
            output,
            "{later} = {earlier}\toverlap={overlap:.2}\tshared={shared}\t{title:?} | {earlier_title:?}",
            later = link.decision_id,
            earlier = link.restates_id,
            overlap = link.overlap,
            shared = link.shared_terms.join(","),
            title = link.title,
            earlier_title = link.restates_title,
        );
    }
    let _ = write!(
        output,
        "{} link(s) proposed over {} decision(s). Record them with `restatements apply --all`, or pick with `--link LATER=EARLIER`.",
        proposals.proposed.len(),
        proposals.decisions_scanned
    );
    output
}

/// One line per link applied, saying whether it was written or already there.
pub(crate) fn render_applied_links(applied: &[AppliedLink]) -> String {
    let mut output = String::new();
    for link in applied {
        let _ = match link.event_id {
            Some(event_id) => writeln!(
                output,
                "linked {} = {} (event {event_id})",
                link.decision_id, link.restates_id
            ),
            None => writeln!(
                output,
                "already linked {} = {}",
                link.decision_id, link.restates_id
            ),
        };
    }
    let written = applied
        .iter()
        .filter(|link| link.event_id.is_some())
        .count();
    let _ = write!(
        output,
        "{written} link(s) recorded, {} already there.",
        applied.len() - written
    );
    output
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
        // A newer accepted answer to the question this decision answers, and any accepted
        // answer that chose differently, sit on their own lines: never folded into `holds`.
        for newer in &item.newer_answers {
            let _ = writeln!(
                output,
                "answer\tnewer\t{}\t{}",
                newer.decision_id,
                format_question_answer(newer)
            );
        }
        for reason in &item.outcome.reasons {
            if let OutcomeReason::ConflictingAnswer { other_id } = reason {
                let _ = writeln!(output, "answer\tconflict\t{other_id}");
            }
        }
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
        // A close match says which words of the question it lacks.
        if !item.missing_terms.is_empty() {
            let _ = write!(output, "\tmissing={}", item.missing_terms.join(","));
        }
        // A decision recorded more than once is one match; the other records are named.
        if !item.also_recorded_as.is_empty() {
            let copies: Vec<&str> = item
                .also_recorded_as
                .iter()
                .map(|copy| copy.decision_id.as_str())
                .collect();
            let _ = write!(output, "\talso_recorded_as={}", copies.join(","));
        }
        output.push('\n');
    }
    output.trim_end().to_owned()
}

/// The name a serde enum has on the wire (`information`, `agent_only`), so the text summary and
/// the JSON never name a thing differently.
fn wire_name<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// One line for one dimension: its level, the node ids behind it and the reasons in words; or, for
/// a dimension that was not assessed, why. No number stands in for either.
fn dimension_summary_line(dimension: Dimension, assessment: &Assessment) -> String {
    let name = wire_name(&dimension);
    match assessment {
        Assessment::Assessed {
            level,
            reasons,
            node_ids,
        } => {
            let reasons: Vec<String> = reasons
                .iter()
                .map(|reason| summary_cell(&reason.text))
                .collect();
            format!(
                "dimension\t{name}\tlevel={}\tids={}\treasons={}",
                wire_name(level),
                node_ids.join(","),
                reasons.join(" | ")
            )
        }
        Assessment::NotAssessed { why } => {
            format!("dimension\t{name}\tnot_assessed\twhy={}", summary_cell(why))
        }
    }
}

/// One line for what a model said about one dimension, printed right after that dimension's
/// floor line: its level, explanation and the passage it quotes (a `none` answer may give no
/// quote, and then the line carries no `quote` cell), or why it did not assess it.
fn model_dimension_line(dimension: Dimension, answer: &ModelDimension) -> String {
    let name = wire_name(&dimension);
    match answer {
        ModelDimension::Assessed {
            level,
            explanation,
            quote,
        } => {
            let mut line = format!(
                "model_dimension\t{name}\tlevel={}\texplanation={}",
                wire_name(level),
                summary_cell(explanation)
            );
            if let Some(quote) = quote {
                let _ = write!(line, "\tquote={}", summary_cell(quote));
            }
            line
        }
        ModelDimension::NotAssessed { reason } => format!(
            "model_dimension\t{name}\tnot_assessed\twhy={}",
            summary_cell(reason)
        ),
    }
}

pub(crate) fn render_score_report_summary(report: &Option<ScoreReport>) -> String {
    let Some(report) = report else {
        return "No decision found".to_owned();
    };
    let profile = &report.profile;
    let mut output = String::new();
    let _ = writeln!(
        output,
        "profile\t{}\tfloor_version={}",
        profile.decision_id, profile.floor_version
    );
    if let Some(model) = &profile.model_assessment {
        let _ = writeln!(
            output,
            "model_assessment\t{}\tprompt_version={}\tevent_origin={}",
            summary_cell(&model.model),
            summary_cell(&model.prompt_version),
            model
                .event_origin
                .map_or_else(|| "-".to_owned(), |origin| origin.to_string())
        );
    }
    for (dimension, assessment) in profile.iter() {
        let _ = writeln!(output, "{}", dimension_summary_line(dimension, assessment));
        if let Some(answer) = profile.model_answer(dimension) {
            let _ = writeln!(output, "{}", model_dimension_line(dimension, answer));
        }
    }
    for attention in &profile.attention {
        let _ = writeln!(
            output,
            "attention\t{}\t{}\tids={}\t{}",
            wire_name(&attention.kind),
            wire_name(&attention.dimension),
            attention.node_ids.join(","),
            summary_cell(&attention.text)
        );
    }
    let provenance = &report.provenance;
    let _ = write!(
        output,
        "provenance\tauthorship={}\treview={}",
        wire_name(&provenance.authorship),
        wire_name(&provenance.review)
    );
    if let Some(line) = provenance.line {
        let _ = write!(output, "\t{line}");
    }
    output
}

pub(crate) fn render_scan_report_summary(report: &ScanReport) -> String {
    if report.findings.is_empty() {
        return "No decision needs a look".to_owned();
    }
    let mut output = String::new();
    let _ = writeln!(
        output,
        "attention\tas_of={}\tevidence_window_days={}",
        report
            .as_of
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        report.evidence_window_days
    );
    for scanned in &report.findings {
        let finding = &scanned.finding;
        let basis = finding.basis_at.map_or_else(
            || "-".to_owned(),
            |at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        let _ = writeln!(
            output,
            "finding\t{}\t{}\t{}\t{}\tbasis_at={basis}\tids={}\t{}",
            finding.finding_id,
            finding.kind.as_str(),
            finding.decision_id,
            summary_cell(&finding.decision_title),
            finding.node_ids.join(","),
            summary_cell(&finding.reason)
        );
        for line in &scanned.dimensions {
            let _ = writeln!(
                output,
                "{}",
                dimension_summary_line(line.dimension, &line.assessment)
            );
        }
    }
    output.trim_end().to_owned()
}

/// The ranking as lines a person can read: the totals, who decided the ranked decisions, then one
/// line per decision with its position, its standing, who decided it and why it sits there.
pub(crate) fn render_importance_summary(report: &ImportanceReport) -> String {
    if report.decisions.is_empty() {
        return "No decisions to rank".to_owned();
    }
    let mut output = String::new();
    let _ = writeln!(
        output,
        "importance\tranked={}\tnot_assessed={}\tleft_out_superseded={}\tleft_out_rejected={}",
        report.ranked_total,
        report.not_assessed_total,
        report.left_out.superseded,
        report.left_out.rejected
    );
    let tally = &report.ranked_decided_by;
    let _ = writeln!(
        output,
        "ranked_decided_by\tperson={}\tagent={}\tagent_within_delegation={}\tmixed={}\tunknown={}\tnone_recorded={}",
        tally.person,
        tally.agent,
        tally.agent_within_delegation,
        tally.mixed,
        tally.unknown,
        tally.none_recorded
    );
    for row in &report.decisions {
        match (row.importance, row.rank) {
            (Importance::Ranked, Some(rank)) => {
                let _ = write!(output, "#{rank}");
            }
            _ => output.push_str("not_assessed"),
        }
        let _ = write!(
            output,
            "\t{}\t{}\tstatus={}\tdecided_by={}",
            row.decision_id,
            summary_cell(&row.title),
            decision_status_label(row.status),
            row.decided_by.kind.as_str()
        );
        let mut separator = '(';
        for decider in &row.decided_by.deciders {
            let _ = write!(output, "{separator}{}", decider.id);
            separator = ',';
        }
        if !row.decided_by.deciders.is_empty() {
            output.push(')');
        }
        if let Some(delegated_by) = &row.decided_by.delegated_by {
            let _ = write!(output, " delegated_by={delegated_by}");
        }
        let _ = writeln!(
            output,
            "\trecorded_by={}\t{}",
            row.recorded_by.as_deref().unwrap_or("-"),
            summary_cell(&row.reasons.join("; "))
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_misfiled_scan_summary(candidates: &[MisfiledDecisionCandidate]) -> String {
    if candidates.is_empty() {
        return "No misfiled candidates found".to_owned();
    }
    let mut output = String::new();
    for candidate in candidates {
        let _ = write!(
            output,
            "misfiled\t{}\t{}\tproject={}\ttopics={}\tactors={}",
            candidate.decision_id,
            candidate.title,
            candidate.project.as_deref().unwrap_or("-"),
            candidate.matched_topic_keys.join(","),
            candidate.actor_ids.join(",")
        );
        if let Some(destination) = &candidate.proposed_move_to {
            let _ = write!(
                output,
                "\tmove=hivemind move --decision {} --to {destination}",
                candidate.decision_id
            );
        }
        output.push('\n');
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

/// Options as a person reads them, comma-separated, each followed by the record's own text when
/// that reads differently. A label that holds a comma is quoted, so each option counts as one.
fn option_labels(options: &[OptionLabel]) -> String {
    let mut labels = String::new();
    for (index, option) in options.iter().enumerate() {
        if index > 0 {
            labels.push_str(", ");
        }
        labels.push_str(&option_label_unit(&option.label));
        if let Some(recorded) = &option.recorded_as {
            let _ = write!(labels, " (recorded as: {recorded})");
        }
    }
    labels
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
    if let Some(question) = &brief.question {
        let _ = writeln!(output, "  answers: {}", summary_cell(question));
    }
    if let Some(quote) = &brief.quote {
        let _ = writeln!(output, "  quote: \"{}\"", summary_cell(quote));
    }
    for other in &brief.other_answers {
        let _ = writeln!(
            output,
            "  also answered by: {}",
            format_question_answer(other)
        );
    }
    // An option reads as its label; when the capture recorded it as something else (a slug, a
    // lettered code) the record's own text follows, so the reading never hides the record.
    if let Some(chosen) = &brief.chosen_option {
        let _ = write!(output, "  chose: {}", summary_cell(&chosen.label));
        if let Some(recorded) = &chosen.recorded_as {
            let _ = write!(output, " (recorded as: {})", summary_cell(recorded));
        }
        output.push('\n');
    }
    if !brief.rejected_options.is_empty() {
        let _ = writeln!(
            output,
            "  rejected: {} (shares the rationale above — no distinct per-option reason is recorded)",
            option_labels(&brief.rejected_options)
        );
    }
    // Nobody turned these down: no choice is recorded, so the question is still open.
    if !brief.open_options.is_empty() {
        let _ = writeln!(
            output,
            "  options: {} (open — no choice is recorded)",
            option_labels(&brief.open_options)
        );
    }
    write_decided_by(output, &brief.decided_by);
    if let Some(asked_at) = brief.asked_at {
        let _ = writeln!(output, "  asked_at: {}", asked_at.to_rfc3339());
    }
    if let Some(occurred_at) = brief.occurred_at {
        let _ = writeln!(output, "  answered_at: {}", occurred_at.to_rfc3339());
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
            format_outcome_reason(reason, &brief.rests_on, &brief.other_answers)
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
/// recorder decided, and a rejected one names who turned it down rather than reading as
/// unreviewed.
fn write_decided_by(output: &mut String, decided_by: &DecidedBy) {
    let recorder = decided_by.proposer_id.as_deref().unwrap_or("unknown");
    match decided_by.decider_ids.as_slice() {
        [] if decided_by.review == ReviewShape::Rejected => {
            let _ = writeln!(
                output,
                "  recorded by: {} (source={})",
                recorder, decided_by.source
            );
            let _ = writeln!(
                output,
                "  rejected by: {} (review={:?}) -- nobody has accepted it",
                decided_by.rejecter_ids.join(", "),
                decided_by.review
            );
        }
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
    if let Some(drafted) = &decided_by.drafted_from {
        write_drafted_from(output, decided_by, drafted);
    }
}

/// Where a classifier-drafted decision was held, as far as the ledger states it, and, while
/// nobody has accepted it, why it is still open. A draft records what a transcript says and is
/// never accepted on anyone's behalf, even when it names a chosen option; an unreceived
/// conversation prints no "drafted in" line rather than a guess.
fn write_drafted_from(output: &mut String, decided_by: &DecidedBy, drafted: &DraftedFrom) {
    let mut held: Vec<String> = Vec::new();
    if !drafted.session_ids.is_empty() {
        held.push(format!("session {}", drafted.session_ids.join(", ")));
    }
    if let Some(initiator) = &drafted.initiated_by {
        held.push(format!("started by {initiator}"));
    }
    if !drafted.participants.is_empty() {
        held.push(format!("with {}", drafted.participants.join(", ")));
    }
    if !held.is_empty() {
        let _ = writeln!(output, "  drafted in: {}", held.join(", "));
    }
    if decided_by.decider_ids.is_empty() && decided_by.review != ReviewShape::Rejected {
        let _ = writeln!(
            output,
            "  undecided: a classifier draft is never self-accepted, and the transcript names no one who accepted it"
        );
    }
}

/// The short reasons a decision no longer holds, in the order they were derived and without
/// repeats: `superseded`, `premise superseded`, `premise rejected`, `assumption refuted`,
/// `contested`, `rejected`.
fn still_holds_labels(reasons: &[OutcomeReason]) -> Vec<&'static str> {
    let mut labels: Vec<&'static str> = Vec::new();
    for reason in reasons {
        let label = match reason {
            OutcomeReason::SupersededBy { .. } => "superseded",
            OutcomeReason::PremisedOnRefuted { .. } => "assumption refuted",
            OutcomeReason::PremiseSuperseded { .. } => "premise superseded",
            OutcomeReason::PremiseRejected { .. } => "premise rejected",
            OutcomeReason::Contested => "contested",
            OutcomeReason::Rejected { .. } => "rejected",
            // A disagreement to resolve, not a reason this decision stopped holding.
            OutcomeReason::ConflictingAnswer { .. } => continue,
        };
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

/// One decision that answers the same question, as `<title> [status]` plus what it chose.
fn format_question_answer(answer: &QuestionAnswer) -> String {
    let mut line = format!(
        "{} [{}]",
        summary_cell(&answer.title),
        decision_status_label(answer.status)
    );
    if let Some(chosen) = &answer.chosen_option {
        let _ = write!(line, " (chose {})", summary_cell(chosen));
    }
    line
}

/// One outcome reason as a sentence. Ids are output handles, so a premise decision is named by
/// its title when `rests_on` carries it.
fn format_outcome_reason(
    reason: &OutcomeReason,
    rests_on: &[GroundingItem],
    other_answers: &[QuestionAnswer],
) -> String {
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
        OutcomeReason::Rejected { by } => {
            format!("rejected by {} and accepted by no one", by.join(", "))
        }
        OutcomeReason::ConflictingAnswer { other_id } => {
            let other = other_answers
                .iter()
                .find(|answer| answer.decision_id == *other_id)
                .map_or_else(
                    || other_id.clone(), // ubs:ignore: fallback owned copy for the sentence; the reason is only borrowed
                    |answer| format!("\"{}\"", summary_cell(&answer.title)),
                );
            format!(
                "conflicting answer: {other} is also accepted and answers the same question with a different choice"
            )
        }
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
            if candidates.iter().all(ResolvedCandidate::is_close) {
                // Every candidate lacks a word, or has the opposite polarity of a negated question.
                let reason = if candidates
                    .iter()
                    .any(|candidate| !candidate.missing_terms.is_empty())
                {
                    "no decision matches every word"
                } else {
                    "the question is negated and no decision matching its words is"
                };
                let count = candidates.len();
                let plural = if count == 1 { "" } else { "s" };
                let _ = writeln!(
                    output,
                    "close: {reason}; {count} close candidate{plural} — resolve with --pick N, #N, or --id"
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
                if candidate.polarity_mismatch {
                    let _ = write!(output, "\tpolarity: {POLARITY_REASON}");
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
    if let Some(timeline) = &neighborhood.timeline {
        write_timeline(&mut output, timeline);
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

/// The `get_waiting_requests` reply: open asks with no answering decision yet, oldest first
/// (hivemind-bbnw.4).
pub(crate) fn render_waiting_requests_summary(results: &WaitingRequestsResults) -> String {
    if results.items.is_empty() {
        return "No waiting requests found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = writeln!(
            output,
            "request\t{}\tasked_at={}\trequested_by={}\t{}",
            item.request_id,
            item.asked_at.to_rfc3339(),
            item.requested_by.as_deref().unwrap_or(""),
            summary_cell(&item.text)
        );
    }
    output.trim_end().to_owned()
}

/// The `get_contested_decisions` reply: decisions in contest, oldest first, with who is on each
/// side (hivemind-bbnw.7).
pub(crate) fn render_contested_decisions_summary(results: &ContestedDecisionsResults) -> String {
    if results.items.is_empty() {
        return "No contested decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = write!(
            output,
            "contested\t{}\tstatus={}\t{}",
            item.decision_id,
            decision_status_label(item.status),
            summary_cell(&item.title)
        );
        match &item.contest {
            Contest::Disagreement {
                accepted_by,
                rejected_by,
            } => {
                let _ = write!(
                    output,
                    "\taccepted_by={}\trejected_by={}",
                    accepted_by.join(","),
                    rejected_by.join(",")
                );
            }
            Contest::ConflictingAnswers {
                question,
                conflicts_with,
                ..
            } => {
                let others: Vec<String> =
                    conflicts_with.iter().map(format_question_answer).collect();
                let _ = write!(
                    output,
                    "\tquestion={}\tconflicts_with={}",
                    summary_cell(question),
                    others.join("; ")
                );
            }
        }
        write_project_field(&mut output, Some(&item.project_label));
        output.push('\n');
    }
    output.trim_end().to_owned()
}

/// The `get_changed_decisions` reply: decisions revised, superseded or left without a premise
/// in the window, most recently changed first, each followed by its dated changes
/// (hivemind-bbnw.7).
pub(crate) fn render_changed_decisions_summary(results: &ChangedDecisionsResults) -> String {
    if results.items.is_empty() {
        return "No changed decisions found".to_owned();
    }

    let mut output = String::new();
    for item in &results.items {
        let _ = write!(
            output,
            "changed\t{}\tstatus={}\tlast_changed_at={}\t{}",
            item.decision_id,
            decision_status_label(item.status),
            item.last_changed_at
                .map_or_else(|| "undated".to_owned(), |ts| ts.to_rfc3339()),
            summary_cell(&item.title)
        );
        write_project_field(&mut output, Some(&item.project_label));
        output.push('\n');
        for change in &item.changes {
            write_timeline_entry(&mut output, change);
        }
    }
    output.trim_end().to_owned()
}

/// A decision's dated story, oldest first, each entry cited by its ledger event, and the one
/// duration it can honestly give (hivemind-bbnw.7).
fn write_timeline(output: &mut String, timeline: &DecisionTimeline) {
    if timeline.entries.is_empty() {
        return;
    }
    let _ = writeln!(output, "  timeline:");
    for entry in &timeline.entries {
        write_timeline_entry(output, entry);
    }
    if let Some(seconds) = timeline.asked_to_decided_seconds {
        let _ = writeln!(output, "  asked to decided: {}", format_duration(seconds));
    }
}

fn write_timeline_entry(output: &mut String, entry: &TimelineEntry) {
    let (label, detail) = timeline_fact_summary(&entry.fact);
    let when = entry
        .ts
        .map_or_else(|| "undated".to_owned(), |ts| ts.to_rfc3339());
    let _ = write!(
        output,
        "    {when}  {label}  by {}",
        entry.actor_id.as_deref().unwrap_or("unknown")
    );
    if !detail.is_empty() {
        let _ = write!(output, "  {detail}");
    }
    let _ = writeln!(output, "  ({})", entry.citation_id);
}

fn timeline_fact_summary(fact: &TimelineFact) -> (&'static str, String) {
    match fact {
        TimelineFact::Asked { request_id, .. } => ("asked", format!("request {request_id}")),
        TimelineFact::Recorded => ("recorded", String::new()),
        TimelineFact::Accepted => ("accepted", String::new()),
        TimelineFact::Rejected { reason } => (
            "rejected",
            reason
                .as_deref()
                .map(|reason| format!("reason: {}", summary_cell(reason)))
                .unwrap_or_default(),
        ),
        TimelineFact::Superseded { by_id } => ("superseded", format!("by {by_id}")),
        TimelineFact::Supersedes { replaces_id } => ("supersedes", replaces_id.clone()),
        TimelineFact::Retitled { from, to, reason } => (
            "retitled",
            format!(
                "\"{}\" -> \"{}\"{}",
                summary_cell(from),
                summary_cell(to),
                reason
                    .as_deref()
                    .map(|reason| format!(" ({})", summary_cell(reason)))
                    .unwrap_or_default()
            ),
        ),
        TimelineFact::Moved { from, to, reason } => (
            "moved",
            format!(
                "{from} -> {to}{}",
                reason
                    .as_deref()
                    .map(|reason| format!(" ({})", summary_cell(reason)))
                    .unwrap_or_default()
            ),
        ),
        TimelineFact::PremiseRefuted {
            hypothesis_id,
            evidence_id,
        } => (
            "premise refuted",
            format!("hypothesis {hypothesis_id} refuted by evidence {evidence_id}"),
        ),
        TimelineFact::PremiseSuperseded { decision_id, by_id } => (
            "premise superseded",
            format!("decision {decision_id} superseded by {by_id}"),
        ),
        TimelineFact::PremiseRejected { decision_id } => (
            "premise rejected",
            format!("decision {decision_id} rejected"),
        ),
    }
}

/// Whole seconds as the two largest units that are not zero: `3d 4h`, `1h 12m`, `45s`.
fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, minutes) => format!("{minutes}m {}s", seconds % 60),
        (0, hours, minutes) => format!("{hours}h {minutes}m"),
        (days, hours, _) => format!("{days}d {hours}h"),
    }
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
        EventType::QuestionRecorded => "question.recorded",
        EventType::QuestionAsked => "question.asked",
        EventType::RelationAdded => "relation.added",
        EventType::RelationRemoved => "relation.removed",
        EventType::BlockerReported => "blocker.reported",
        EventType::BlockerResolved => "blocker.resolved",
        EventType::NotificationSent => "notification.sent",
        EventType::NotificationAcknowledged => "notification.acknowledged",
        EventType::SuggestionSurfaced => "suggestion.surfaced",
        EventType::IngestBatchReceived => "ingest.batch_received",
        EventType::IngestBatchClassified => "ingest.batch_classified",
        EventType::DecisionScored => "decision.scored",
        EventType::DecisionMetadataDerived => "decision.metadata_derived",
        EventType::DecisionMoved => "decision.moved",
        EventType::DecisionRetitled => "decision.retitled",
        EventType::ProjectRegistered => "project.registered",
        EventType::ProjectLinked => "project.linked",
        EventType::ProjectUnlinked => "project.unlinked",
        EventType::ProjectAnchored => "project.anchored",
        EventType::ProjectUnanchored => "project.unanchored",
        EventType::ProjectTopicDeclared => "project.topic_declared",
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
        HistoryChangeKind::TitleChanged => "title_changed",
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
    if !placement.declared_topics.is_empty() {
        let _ = write!(
            line,
            "; declared topics for {}: {}",
            placement.project,
            placement.declared_topics.join(", ")
        );
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

pub(crate) fn format_retitle_output(
    as_json: bool,
    output: &DecisionRetitleOutcome,
) -> Result<String> {
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
    if output.rests_on.is_empty() && output.answers.is_some() {
        let _ = write!(rendered, ": attributed to {}", output.actor_id);
    } else {
        let _ = write!(
            rendered,
            ": added {} thing(s) it rests on, attributed to {}",
            output.rests_on.len(),
            output.actor_id
        );
    }
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
    if let Some(answers) = &output.answers {
        let how = if answers.reused {
            "existing question"
        } else {
            "new question"
        };
        let _ = write!(
            rendered,
            "\n  answers \"{}\" ({how} {})",
            answers.text, answers.question_id
        );
    }
    for premise_id in &output.premise_stale {
        let _ = write!(
            rendered,
            "\npremise_stale: {premise_id} (already superseded or rejected; the link is recorded)"
        );
    }
    Ok(rendered)
}

/// `ask`'s reply: the request recorded and the question it names (hivemind-bbnw.4).
pub(crate) fn format_ask_output(as_json: bool, output: &AskCommandOutput) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    let how = if output.reused {
        "existing question"
    } else {
        "new question"
    };
    Ok(format!(
        "asked \"{}\" ({how} {}): request {}, attributed to {}",
        output.text, output.question_id, output.request_id, output.actor_id
    ))
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
                .or_else(|| graph_property_string(properties, "text"))
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
        NodeKind::Question => "node.id AS id, node.text AS text",
        NodeKind::Project => {
            "node.id AS id, node.handle AS handle, node.display_name AS display_name, node.purpose AS purpose, node.anchors AS anchors"
        }
        NodeKind::Ask => "node.id AS id, node.question_id AS question_id, node.text AS text, node.asked_at AS asked_at",
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
        NodeKind::Question => insert_if_present(&mut properties, row, "text"),
        NodeKind::Project => {
            insert_if_present(&mut properties, row, "handle");
            insert_if_present(&mut properties, row, "display_name");
            insert_if_present(&mut properties, row, "purpose");
            insert_if_present(&mut properties, row, "anchors");
        }
        NodeKind::Ask => {
            insert_if_present(&mut properties, row, "question_id");
            insert_if_present(&mut properties, row, "text");
            insert_if_present(&mut properties, row, "asked_at");
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
        NodeKind::Question => "#d4e6f1",
        NodeKind::Project => "#aed6f1",
        NodeKind::Ask => "#a9dfbf",
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
    /// Set when `--project-from-context` has something to say about how the project was worked
    /// out: it found none to attach the capture to (the folder is not attached, and how to
    /// attach it), or the change spans several projects (recorded for their parent, or saved to
    /// the personal project with how to move it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_reminder: Option<String>,
    /// Captures of an `emit ingest.batch_classified` that named a decision they restate, and
    /// whether each was linked or not recorded; absent when none did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) restated: Vec<RestatedCapture>,
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
            restated: Vec::new(),
        }
    }

    pub(crate) fn with_restated(mut self, restated: Vec<RestatedCapture>) -> Self {
        self.restated = restated;
        self
    }

    pub(crate) fn with_placement(mut self, placement: DecisionPlacement) -> Self {
        self.project_notice = placement.notice();
        self.placement = Some(placement);
        self
    }

    pub(crate) fn with_project_reminder(mut self, reminder: Option<String>) -> Self {
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
    pub(crate) project_reminder: Option<String>,
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
    pub(crate) project_reminder: Option<String>,
    pub(crate) rests_on: Vec<RestsOn>,
    pub(crate) premise_stale: Vec<String>,
    /// Decisions the rationale names by id that the capture does not rest on; see
    /// `GroundedProposal::cited_not_linked`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) cited_not_linked: Vec<String>,
    /// The question node the decision was linked to, when the capture named a question.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) question_id: Option<String>,
}

/// The question a `ground --answers` linked the decision to.
#[derive(Debug, Serialize)]
pub(crate) struct GroundAnswerOutput {
    pub(crate) question_id: String,
    pub(crate) text: String,
    /// True when the question node already existed; false when this call created it.
    pub(crate) reused: bool,
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
    /// The question the decision now answers, when `--answers` named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answers: Option<GroundAnswerOutput>,
}

/// The `ask` reply: the request recorded and the question it names (hivemind-bbnw.4).
#[derive(Debug, Serialize)]
pub(crate) struct AskCommandOutput {
    pub(crate) request_id: String,
    pub(crate) question_id: String,
    pub(crate) text: String,
    /// Who the ask is attributed to.
    pub(crate) actor_id: String,
    /// True when an earlier ask or capture already recorded this question's node.
    pub(crate) reused: bool,
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

#[derive(Debug, Serialize)]
pub(crate) struct ProjectDeclareTopicOutput {
    pub(crate) handle: String,
    /// `true` when the keys came from `--in-use` (the decisions' own keys) rather than the
    /// command line.
    pub(crate) in_use: bool,
    pub(crate) topics: Vec<ProjectTopicDeclaration>,
}

/// One line per key: declared now (with its event id) or already there. `--in-use` with
/// nothing left to declare says so instead of printing nothing.
pub(crate) fn format_project_declare_topic_output(
    as_json: bool,
    output: &ProjectDeclareTopicOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }
    if output.topics.is_empty() {
        return Ok(format!(
            "every topic key in use in {} is already declared",
            output.handle
        ));
    }
    let lines: Vec<String> = output
        .topics
        .iter()
        .map(|topic| match topic.event_id {
            Some(event_id) => format!(
                "event_id={event_id} handle={} declared_topic={}",
                topic.handle, topic.topic_key
            ),
            None => format!(
                "handle={} topic={} already_declared",
                topic.handle, topic.topic_key
            ),
        })
        .collect();
    Ok(lines.join("\n"))
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
            "project\t{}\t{}\tanchors={}\tpart_of={}\tdepends_on={}\ttopics={}",
            project.handle,
            summary_cell(project.display_name.as_deref().unwrap_or("-")),
            project.anchors.len(),
            project
                .part_of
                .as_ref()
                .map_or("-", |fact| fact.to.as_str()),
            project.depends_on.len(),
            project.topics.len(),
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
            "outcome=found\thandle={}\tpersonal={}\tdisplay_name={}\tanchors={}\tpart_of={}\tdepends_on={}\ttopics={}",
            project.handle,
            project.personal,
            summary_cell(project.display_name.as_deref().unwrap_or("-")),
            project.anchors.len(),
            project
                .part_of
                .as_ref()
                .map_or("-", |fact| fact.to.as_str()),
            project.depends_on.len(),
            project_topics_cell(&project.topics),
        ),
    }
}

/// A project's declared topic keys, comma-joined in key order; `-` when it has none.
fn project_topics_cell(topics: &[ProjectTopicFact]) -> String {
    if topics.is_empty() {
        return "-".to_owned();
    }
    topics
        .iter()
        .map(|topic| topic.topic_key.as_str())
        .collect::<Vec<_>>()
        .join(",")
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

/// What `hivemind migrate` did, or with `--dry-run` would do.
#[derive(Debug, Serialize)]
pub(crate) struct MigrateReport {
    pub(crate) dry_run: bool,
    pub(crate) source_dir: String,
    pub(crate) source_tenant: String,
    /// `postgres`, or the cell's base URL. Never the database URL, which carries a credential.
    pub(crate) destination: String,
    pub(crate) destination_tenant: String,
    /// Events the source ledger holds for its tenant.
    pub(crate) source_event_count: usize,
    /// Source events the destination did not hold: moved by a real run, only counted by a dry run.
    pub(crate) new_events: usize,
    /// Source events whose uuid the destination already held; none of them was written again.
    pub(crate) already_present: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parity_check: Option<ParityCheckResult>,
}

/// Every source event uuid looked up in the destination after a real run.
#[derive(Debug, Serialize)]
pub(crate) struct ParityCheckResult {
    pub(crate) source_event_count: usize,
    pub(crate) present_in_destination: usize,
    pub(crate) missing: usize,
    pub(crate) ok: bool,
}

#[cfg(test)]
mod tests;
