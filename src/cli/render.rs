use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::error::{CliError, CommandError};
use crate::events::{EventId, EventType};
use crate::ingest::{DocumentImportReport, DocumentPreparationReport};
use crate::projector::{
    GraphParams, GraphProperties, GraphRow, GraphValue, GraphView, NodeKind,
    RelationKind as GraphRelationKind,
};
use crate::queries::{
    derive_decision_status, derive_hypothesis_status, BlockerNotificationCandidates, CompactView,
    DecisionBlockerResults, DecisionSearchResults, DecisionStatus, DecisionView,
    DecisionsAddedSinceResults, DecisionsChangedSinceResults, HistoryChangeKind, HypothesisStatus,
    NeighborhoodView, QueryResponse, ReadOnlyExport,
    ReadOnlyExportFormat as QueryReadOnlyExportFormat, ReadOnlyExportQueryKind,
    RecentActivityResults, RecentDecisionsResults, SupersessionChain,
};
use crate::{HivemindError, Result};

use super::args::CliExit;

pub(crate) fn render_compact_view_summary(view: &Option<CompactView>) -> String {
    let Some(v) = view else {
        return "decision not found".to_owned();
    };
    let mut out = format!(
        "CompactView: {} [{:?}]\n  rationale: {}\n",
        v.decision.id, v.decision.status, v.decision.rationale,
    );
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
            "{}\t{}\t{}\t{}\tactor={}\tsource={}\tcitation={}",
            timestamp,
            decision_status_label(item.status),
            item.decision_id,
            summary_cell(&item.title),
            item.actor_ids.join(","),
            item.creation.source.as_str(),
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
            "{}\t{}\t{}\tactor={}\tsource={}\tdecisions={}\tcitation={}",
            item.event_origin,
            change_kind_label(item.change_kind),
            event_type_label(item.event_type),
            item.actor_id,
            item.source.as_str(),
            item.decision_ids.join(","),
            item.citation_id
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
            "{}\t{}\t{}\tactor={}\tsource={}\tdecisions={}\tcitation={}",
            item.event_origin,
            change_kind_label(item.change_kind),
            event_type_label(item.event_type),
            item.actor_id,
            item.source.as_str(),
            item.decision_ids.join(","),
            item.citation_id
        );
    }
    output.trim_end().to_owned()
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
            "match\trank={}\t{}\t{}\t{}\ttopics={}\tmatched={}",
            item.rank,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            item.decision.topic_keys.join(","),
            item.matched_fields.join(",")
        );
    }
    output.trim_end().to_owned()
}

pub(crate) fn render_recall_summary(response: &crate::summarize::RecallResponse) -> String {
    let mut output = String::new();
    if response.ranked.items.is_empty() {
        return "No decisions found matching the query.".to_owned();
    }
    let _ = writeln!(output, "digest\t{}", summary_cell(&response.digest.summary));
    let _ = writeln!(
        output,
        "cited\t{}",
        response.digest.cited_decision_ids.join(",")
    );
    for item in &response.ranked.items {
        let _ = writeln!(
            output,
            "match\trank={}\t{}\t{}\t{}\ttopics={}",
            item.rank,
            decision_status_label(item.decision.status),
            item.decision.id,
            summary_cell(&item.decision.title),
            item.decision.topic_keys.join(","),
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

pub(crate) fn render_neighborhood_summary(neighborhood: &NeighborhoodView) -> String {
    let mut output = String::new();
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
        let _ = writeln!(
            output,
            "node\t{}\t{}\tstatus={}",
            node.kind.table_name(),
            node.id,
            status
        );
    }
    for edge in &neighborhood.edges {
        match edge.event_origin {
            Some(event_origin) => {
                let _ = writeln!(
                    output,
                    "edge\t{}\t{}\t{}\tevent_origin={}",
                    edge.relation.table_name(),
                    edge.from,
                    edge.to,
                    event_origin
                );
            }
            None => {
                let _ = writeln!(
                    output,
                    "edge\t{}\t{}\t{}\tevent_origin=unknown",
                    edge.relation.table_name(),
                    edge.from,
                    edge.to
                );
            }
        }
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
        "{}\t{}\t{}\t{}\ttopics={}",
        prefix,
        decision_status_label(decision.status),
        decision.id,
        summary_cell(&decision.title),
        decision.topic_keys.join(",")
    );
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
    }
}

fn change_kind_label(kind: HistoryChangeKind) -> &'static str {
    match kind {
        HistoryChangeKind::NewDecision => "new_decision",
        HistoryChangeKind::StatusChange => "status_change",
        HistoryChangeKind::NewEvidence => "new_evidence",
        HistoryChangeKind::RefutedPremise => "refuted_premise",
        HistoryChangeKind::Supersession => "supersession",
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
        Ok(envelope.value.clone())
    }
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

pub(crate) fn format_supersede_output(
    as_json: bool,
    output: &SupersedeCommandOutput,
) -> Result<String> {
    if as_json {
        return format_json_value(true, output);
    }

    Ok(format!(
        "proposal_event_id={} superseded_event_id={} old_decision_id={} new_decision_id={} old_status={} new_status={}",
        output.proposal_event_id,
        output.superseded_event_id,
        output.old_decision_id,
        output.new_decision_id,
        decision_status_label(output.old_decision_status),
        decision_status_label(output.new_decision_status)
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
        Ok(format!(
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
        ))
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
            edge.relation.table_name()
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

fn graph_edges(graph: &impl GraphView) -> Result<BTreeSet<DotEdge>> {
    let mut edges = BTreeSet::new();
    for relation in GraphRelationKind::ALL {
        let (from_kind, to_kind) = relation.endpoints();
        let mut query = String::new();
        let _ = write!(
            query,
            "MATCH (from:`{}`)-[rel:`{}`]->(to:`{}`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
            from_kind.table_name(),
            relation.table_name(),
            to_kind.table_name()
        );
        let rows = graph.query(&query, &GraphParams::new())?;
        for row in rows {
            edges.insert(DotEdge {
                relation,
                from_kind,
                from_id: required_row_string(&row, "from_id")?,
                to_kind,
                to_id: required_row_string(&row, "to_id")?,
            });
        }
    }
    Ok(edges)
}

fn node_dump_query(kind: NodeKind) -> String {
    let projection = match kind {
        NodeKind::Decision => {
            "node.id AS id, node.title AS title, node.rationale AS rationale, node.topic_keys AS topic_keys"
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
}

#[derive(Debug, Serialize)]
pub(crate) struct OutputEnvelope {
    pub(crate) subcommand: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) value: String,
}

impl OutputEnvelope {
    pub(crate) fn new(subcommand: &'static str, kind: &'static str, value: String) -> Self {
        Self {
            subcommand,
            kind,
            value,
        }
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
