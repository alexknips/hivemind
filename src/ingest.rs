use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::commands::{Commands, DecisionProposalEventUuids};
use crate::error::{CliError, CommandError};
use crate::events::{Event, EventProvenance, EventSource, EventType};
use crate::ledger::EventLedger;
use crate::Result;

pub const DEFAULT_SLACK_MENTION: &str = "@hivemind";

#[derive(Debug, Clone, Deserialize)]
pub struct SlackThreadFixture {
    #[serde(alias = "workspace_id")]
    pub team_id: String,
    pub channel_id: String,
    pub thread_ts: String,
    pub messages: Vec<SlackMessageFixture>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackMessageFixture {
    #[serde(alias = "user")]
    pub user_id: String,
    pub ts: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackDecisionDraft {
    pub actor_id: String,
    pub source_ref: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub option_labels: Vec<String>,
    pub chosen_option_label: Option<String>,
    pub thread_context: String,
}

pub fn parse_slack_thread_fixture(input: &str) -> Result<SlackThreadFixture> {
    serde_json::from_str(input).map_err(|error| {
        CommandError::Validation(format!("invalid slack thread fixture: {error}")).into()
    })
}

pub fn extract_slack_decision_draft(
    thread: &SlackThreadFixture,
    mention: &str,
) -> Result<SlackDecisionDraft> {
    validate_thread(thread)?;
    let mention = mention.trim();
    require_non_empty("mention", mention)?;

    if !thread
        .messages
        .iter()
        .any(|message| message.text.contains(mention))
    {
        return Err(CommandError::Validation(format!(
            "slack thread is missing required mention '{mention}'"
        ))
        .into());
    }

    let markers = parse_decision_markers(thread)?;
    let source_ref = slack_thread_source_ref(thread);

    Ok(SlackDecisionDraft {
        actor_id: markers.actor_id,
        source_ref: source_ref.clone(),
        title: markers.title,
        rationale: markers.rationale,
        topic_keys: markers.topic_keys,
        option_labels: markers.option_labels,
        chosen_option_label: markers.chosen_option_label,
        thread_context: render_thread_context(thread, &source_ref),
    })
}

pub fn slack_thread_source_ref(thread: &SlackThreadFixture) -> String {
    format!(
        "slack://{}/{}/{}",
        thread.team_id, thread.channel_id, thread.thread_ts
    )
}

fn validate_thread(thread: &SlackThreadFixture) -> Result<()> {
    require_non_empty("team_id", &thread.team_id)?;
    require_non_empty("channel_id", &thread.channel_id)?;
    require_non_empty("thread_ts", &thread.thread_ts)?;
    if thread.messages.is_empty() {
        return Err(CommandError::Validation("messages must not be empty".to_owned()).into());
    }

    for message in &thread.messages {
        require_non_empty("message.user_id", &message.user_id)?;
        require_non_empty("message.ts", &message.ts)?;
        require_non_empty("message.text", &message.text)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlackDecisionMarkers {
    actor_id: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    option_labels: Vec<String>,
    chosen_option_label: Option<String>,
}

fn parse_decision_markers(thread: &SlackThreadFixture) -> Result<SlackDecisionMarkers> {
    let mut actor_user_id = None;
    let mut title = None;
    let mut rationale = None;
    let mut topic_keys = Vec::new();
    let mut option_labels = Vec::new();
    let mut chosen_option_label = None;

    for message in &thread.messages {
        for line in message.text.lines() {
            let line = line.trim();
            if let Some(value) = marker_value(line, "Decision") {
                title = Some(value.to_owned());
                actor_user_id.get_or_insert_with(|| message.user_id.clone());
            } else if let Some(value) = marker_value(line, "Rationale") {
                rationale = Some(value.to_owned());
            } else if let Some(value) = marker_value(line, "Options") {
                option_labels = split_marker_list(value);
            } else if let Some(value) =
                marker_value(line, "Topics").or_else(|| marker_value(line, "Topic"))
            {
                topic_keys = split_marker_list(value);
            } else if let Some(value) =
                marker_value(line, "Chosen").or_else(|| marker_value(line, "Chose"))
            {
                chosen_option_label = Some(value.to_owned());
            }
        }
    }

    let title = required_marker(title, "Decision")?;
    let rationale = required_marker(rationale, "Rationale")?;
    if option_labels.is_empty() {
        return Err(CommandError::Validation(
            "Options marker must contain at least one option".to_owned(),
        )
        .into());
    }
    if topic_keys.is_empty() {
        topic_keys.push("slack".to_owned());
    }

    let actor_user_id = actor_user_id.ok_or_else(|| {
        CommandError::Validation("Decision marker must identify an author".to_owned())
    })?;
    let actor_id = format!("slack:{}:{}", thread.team_id, actor_user_id);

    Ok(SlackDecisionMarkers {
        actor_id,
        title,
        rationale,
        topic_keys,
        option_labels,
        chosen_option_label,
    })
}

fn marker_value<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case(marker)
        .then_some(value.trim())
        .filter(|value| !value.is_empty())
}

fn split_marker_list(value: &str) -> Vec<String> {
    value
        .split([',', '|'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn required_marker(value: Option<String>, marker: &'static str) -> Result<String> {
    value.ok_or_else(|| {
        CommandError::Validation(format!(
            "{marker} marker is required for slack thread draft extraction"
        ))
        .into()
    })
}

fn render_thread_context(thread: &SlackThreadFixture, source_ref: &str) -> String {
    let mut context = format!("Slack thread {source_ref}\n");
    for message in &thread.messages {
        context.push_str(&format!(
            "{} {}: {}\n",
            message.ts,
            message.user_id,
            message.text.trim()
        ));
    }
    context
}

fn require_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackIngestOutcome {
    Imported {
        decision_id: String,
        evidence_id: String,
        option_ids: Vec<String>,
    },
    AlreadyImported {
        decision_id: String,
    },
}

impl SlackIngestOutcome {
    pub fn decision_id(&self) -> &str {
        match self {
            Self::Imported { decision_id, .. } | Self::AlreadyImported { decision_id } => {
                decision_id
            }
        }
    }
}

pub fn import_slack_thread<L: EventLedger>(
    ledger: &L,
    draft: &SlackDecisionDraft,
) -> Result<SlackIngestOutcome> {
    if let Some(decision_id) = find_existing_slack_decision(ledger, &draft.source_ref)? {
        return Ok(SlackIngestOutcome::AlreadyImported { decision_id });
    }

    let commands =
        Commands::new_with_provenance(ledger, EventProvenance::slack(draft.source_ref.clone()));

    let evidence_id = commands.record_evidence(&draft.actor_id, &draft.thread_context)?;

    let mut option_ids = Vec::with_capacity(draft.option_labels.len());
    let mut chosen_option_id = None;
    for label in &draft.option_labels {
        let option_id = commands.record_option(
            &draft.actor_id,
            label,
            &format!("Slack option '{label}' captured from {}", draft.source_ref),
        )?;
        if draft.chosen_option_label.as_deref() == Some(label.as_str()) {
            chosen_option_id = Some(option_id.clone());
        }
        option_ids.push(option_id);
    }

    if draft.chosen_option_label.is_some() && chosen_option_id.is_none() {
        return Err(CommandError::Validation(
            "Chosen marker must match one of the Options entries".to_owned(),
        )
        .into());
    }

    let decision_id = commands.propose_decision(
        &draft.actor_id,
        &draft.title,
        &draft.rationale,
        &draft.topic_keys,
        &option_ids,
        chosen_option_id.as_deref(),
        &[],
        std::slice::from_ref(&evidence_id),
    )?;

    Ok(SlackIngestOutcome::Imported {
        decision_id,
        evidence_id,
        option_ids,
    })
}

fn find_existing_slack_decision<L: EventLedger>(
    ledger: &L,
    source_ref: &str,
) -> Result<Option<String>> {
    const PAGE_SIZE: usize = 1024;
    let mut offset = 0;
    loop {
        let events = ledger.read(offset, PAGE_SIZE)?;
        if events.is_empty() {
            return Ok(None);
        }
        for event in &events {
            if event.event_type == EventType::DecisionProposed
                && event.source == EventSource::Slack
                && event.source_ref.as_deref() == Some(source_ref)
            {
                if let Some(decision_id) = decision_id_of(event) {
                    return Ok(Some(decision_id));
                }
            }
        }
        match events.last().and_then(|event| event.event_id) {
            Some(last) => offset = last,
            None => return Ok(None),
        }
    }
}

fn decision_id_of(event: &Event) -> Option<String> {
    event
        .payload
        .get("decision_id")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentImportFormat {
    Auto,
    Markdown,
    Text,
}

impl DocumentImportFormat {
    fn accepts_path(self, path: &Path) -> bool {
        match self {
            Self::Markdown | Self::Text => true,
            Self::Auto => path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "md" | "markdown" | "txt"
                    )
                })
                .unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentImportRequest {
    pub paths: Vec<PathBuf>,
    pub importer_actor_id: String,
    pub format: DocumentImportFormat,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentImportReport {
    pub import_run_id: String,
    pub summary: DocumentImportSummary,
    pub files: Vec<DocumentFileImportReport>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DocumentImportSummary {
    pub files_seen: usize,
    pub files_skipped: usize,
    pub blocks_seen: usize,
    pub blocks_imported: usize,
    pub blocks_noop: usize,
    pub blocks_conflicted: usize,
    pub duplicate_candidates: usize,
    pub validation_errors: usize,
    pub events_written: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentFileImportReport {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    pub status: DocumentFileImportStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub blocks: Vec<DocumentBlockImportReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFileImportStatus {
    Processed,
    SkippedUnsupported,
    SkippedUnmarked,
    ValidationError,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentBlockImportReport {
    pub block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    pub status: DocumentBlockImportStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_span: Option<DocumentSourceSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_snippet: Option<String>,
    pub event_ids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentBlockImportStatus {
    Imported,
    NoOp,
    Conflict,
    DuplicateCandidate,
    ValidationError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSourceSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentSourceRef {
    source: String,
    path: String,
    sha256: String,
    import_run_id: String,
    block_id: String,
    source_span: DocumentSourceSpan,
    source_snippet: String,
    importer_actor_id: String,
    original_actor_id: Option<String>,
    provisional_actor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocumentDecisionDraft {
    block_id: String,
    title: String,
    status: ImportedDecisionStatus,
    original_actor_id: Option<String>,
    topic_keys: Vec<String>,
    rationale: String,
    option_labels: Vec<String>,
    chosen_option_label: Option<String>,
    evidence: Vec<String>,
    hypotheses: Vec<String>,
    supersedes: Vec<String>,
    span: DocumentSourceSpan,
    snippet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportedDecisionStatus {
    Proposed,
    Accepted,
    Rejected,
}

impl ImportedDecisionStatus {
    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "proposed" => Ok(Self::Proposed),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            other => Err(CommandError::Validation(format!(
                "unsupported decision status '{other}'; expected proposed, accepted, or rejected"
            ))
            .into()),
        }
    }
}

pub fn collect_document_import_paths(
    paths: &[PathBuf],
    format: DocumentImportFormat,
) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Err(CliError::InvalidInput(
            "import documents requires at least one --file or path".to_owned(),
        )
        .into());
    }

    let mut files = Vec::new();
    for path in paths {
        collect_document_import_path(path, format, true, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

pub fn import_documents<L: EventLedger>(
    ledger: &L,
    request: &DocumentImportRequest,
) -> Result<DocumentImportReport> {
    let importer_actor_id = request.importer_actor_id.trim();
    if importer_actor_id.is_empty() {
        return Err(
            CommandError::Validation("importer_actor_id must not be empty".to_owned()).into(),
        );
    }

    let files = collect_document_import_paths(&request.paths, request.format)?;
    let import_run_id = import_run_id(&files);
    let mut summary = DocumentImportSummary {
        files_seen: files.len(),
        ..DocumentImportSummary::default()
    };
    let mut file_reports = Vec::with_capacity(files.len());

    for file in files {
        let report = import_document_file(ledger, &file, request, &import_run_id)?;
        accumulate_file_summary(&mut summary, &report);
        file_reports.push(report);
    }

    Ok(DocumentImportReport {
        import_run_id,
        summary,
        files: file_reports,
    })
}

fn collect_document_import_path(
    path: &Path,
    format: DocumentImportFormat,
    explicit: bool,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|error| {
        CliError::InvalidInput(format!(
            "cannot read import path {}: {error}",
            path.display()
        ))
    })?;

    if metadata.is_file() {
        if explicit || format.accepts_path(path) {
            files.push(path.to_owned());
        }
        return Ok(());
    }

    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot list import directory {}: {error}",
                    path.display()
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot read import directory entry in {}: {error}",
                    path.display()
                ))
            })?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            collect_document_import_path(&entry.path(), format, false, files)?;
        }
        return Ok(());
    }

    Err(CliError::InvalidInput(format!(
        "import path is neither file nor directory: {}",
        path.display()
    ))
    .into())
}

fn import_document_file<L: EventLedger>(
    ledger: &L,
    path: &Path,
    request: &DocumentImportRequest,
    import_run_id: &str,
) -> Result<DocumentFileImportReport> {
    if !request.format.accepts_path(path) {
        return Ok(DocumentFileImportReport {
            path: path.display().to_string(),
            canonical_path: None,
            source_hash: None,
            status: DocumentFileImportStatus::SkippedUnsupported,
            message: Some("unsupported document extension".to_owned()),
            blocks: Vec::new(),
        });
    }

    let bytes = fs::read(path).map_err(|error| {
        CliError::InvalidInput(format!("cannot read document {}: {error}", path.display()))
    })?;
    let source_hash = sha256_hex(&bytes);
    let text = String::from_utf8(bytes).map_err(|error| {
        CliError::InvalidInput(format!(
            "document {} is not valid UTF-8: {error}",
            path.display()
        ))
    })?;
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        CliError::InvalidInput(format!(
            "cannot canonicalize document path {}: {error}",
            path.display()
        ))
    })?;
    let canonical_path = canonical_path.display().to_string();
    let raw_blocks = find_document_decision_blocks(&text);

    if raw_blocks.is_empty() {
        return Ok(DocumentFileImportReport {
            path: path.display().to_string(),
            canonical_path: Some(canonical_path),
            source_hash: Some(source_hash),
            status: DocumentFileImportStatus::SkippedUnmarked,
            message: Some("no explicit Decision: blocks found".to_owned()),
            blocks: Vec::new(),
        });
    }

    let namespace = document_namespace(&canonical_path);
    let mut block_reports = Vec::with_capacity(raw_blocks.len());
    for raw_block in raw_blocks {
        let block_report = match parse_document_decision_block(&raw_block) {
            Ok(draft) => import_document_decision_block(
                ledger,
                &draft,
                &canonical_path,
                &source_hash,
                &namespace,
                request.importer_actor_id.trim(),
                import_run_id,
            )?,
            Err(error) => DocumentBlockImportReport {
                block_id: raw_block.fallback_id(),
                decision_id: None,
                status: DocumentBlockImportStatus::ValidationError,
                message: Some(error.to_string()),
                source_span: Some(raw_block.span),
                source_snippet: Some(compact_snippet(&raw_block.text)),
                event_ids: Vec::new(),
            },
        };
        block_reports.push(block_report);
    }

    let status = if block_reports
        .iter()
        .all(|block| block.status == DocumentBlockImportStatus::ValidationError)
    {
        DocumentFileImportStatus::ValidationError
    } else {
        DocumentFileImportStatus::Processed
    };

    Ok(DocumentFileImportReport {
        path: path.display().to_string(),
        canonical_path: Some(canonical_path),
        source_hash: Some(source_hash),
        status,
        message: None,
        blocks: block_reports,
    })
}

fn import_document_decision_block<L: EventLedger>(
    ledger: &L,
    draft: &DocumentDecisionDraft,
    canonical_path: &str,
    source_hash: &str,
    namespace: &str,
    importer_actor_id: &str,
    import_run_id: &str,
) -> Result<DocumentBlockImportReport> {
    let identities = DocumentImportIdentities::new(draft, canonical_path, source_hash, namespace)?;

    if event_uuid_exists(ledger, identities.proposal_uuid)? {
        return Ok(DocumentBlockImportReport {
            block_id: draft.block_id.clone(),
            decision_id: Some(identities.decision_id),
            status: DocumentBlockImportStatus::NoOp,
            message: Some("identical decision block already imported".to_owned()),
            source_span: Some(draft.span),
            source_snippet: Some(draft.snippet.clone()),
            event_ids: Vec::new(),
        });
    }

    if let Some(existing_path) =
        find_document_duplicate_candidate(ledger, canonical_path, source_hash, &draft.block_id)?
    {
        return Ok(DocumentBlockImportReport {
            block_id: draft.block_id.clone(),
            decision_id: Some(identities.decision_id),
            status: DocumentBlockImportStatus::DuplicateCandidate,
            message: Some(format!(
                "same source hash and block id were already imported from {existing_path}"
            )),
            source_span: Some(draft.span),
            source_snippet: Some(draft.snippet.clone()),
            event_ids: Vec::new(),
        });
    }

    if decision_id_exists(ledger, &identities.decision_id)? {
        return Ok(DocumentBlockImportReport {
            block_id: draft.block_id.clone(),
            decision_id: Some(identities.decision_id),
            status: DocumentBlockImportStatus::Conflict,
            message: Some(
                "stable decision id already exists with different imported content".to_owned(),
            ),
            source_span: Some(draft.span),
            source_snippet: Some(draft.snippet.clone()),
            event_ids: Vec::new(),
        });
    }

    for superseded_decision_id in &identities.supersedes_decision_ids {
        if !decision_id_exists(ledger, superseded_decision_id)? {
            return Ok(DocumentBlockImportReport {
                block_id: draft.block_id.clone(),
                decision_id: Some(identities.decision_id),
                status: DocumentBlockImportStatus::ValidationError,
                message: Some(format!(
                    "superseded decision does not exist: {superseded_decision_id}"
                )),
                source_span: Some(draft.span),
                source_snippet: Some(draft.snippet.clone()),
                event_ids: Vec::new(),
            });
        }
    }

    let actor_id = draft
        .original_actor_id
        .as_deref()
        .unwrap_or(importer_actor_id);
    let source_ref = serde_json::to_string(&DocumentSourceRef {
        source: "document".to_owned(),
        path: canonical_path.to_owned(),
        sha256: source_hash.to_owned(),
        import_run_id: import_run_id.to_owned(),
        block_id: draft.block_id.clone(),
        source_span: draft.span,
        source_snippet: draft.snippet.clone(),
        importer_actor_id: importer_actor_id.to_owned(),
        original_actor_id: draft.original_actor_id.clone(),
        provisional_actor: draft.original_actor_id.is_none(),
    })
    .map_err(|error| {
        CommandError::Validation(format!("source_ref serialization failed: {error}"))
    })?;

    let commands =
        Commands::new_with_provenance(ledger, EventProvenance::document(source_ref.clone()));
    let mut event_ids = Vec::new();

    for (evidence_id, evidence, event_uuid) in identities
        .evidence_ids
        .iter()
        .zip(&draft.evidence)
        .zip(&identities.evidence_event_uuids)
        .map(|((id, content), uuid)| (id, content, *uuid))
    {
        event_ids.push(commands.record_evidence_with_id(
            actor_id,
            evidence_id,
            evidence,
            Some(source_ref.as_str()),
            event_uuid,
        )?);
    }

    for (hypothesis_id, hypothesis, event_uuid) in identities
        .hypothesis_ids
        .iter()
        .zip(&draft.hypotheses)
        .zip(&identities.hypothesis_event_uuids)
        .map(|((id, statement), uuid)| (id, statement, *uuid))
    {
        event_ids.push(commands.record_hypothesis_with_id(
            actor_id,
            hypothesis_id,
            hypothesis,
            event_uuid,
        )?);
    }

    for (option_id, label) in identities.option_ids.iter().zip(&draft.option_labels) {
        commands.record_option_with_id(
            actor_id,
            option_id,
            label,
            &format!("Option imported from document block {}", draft.block_id),
        )?;
    }

    let proposal_events = commands.propose_decision_with_id(
        actor_id,
        &identities.decision_id,
        &draft.title,
        &draft.rationale,
        &draft.topic_keys,
        &identities.option_ids,
        identities.chosen_option_id.as_deref(),
        &identities.hypothesis_ids,
        &identities.evidence_ids,
        identities.proposal_event_uuids,
    )?;
    event_ids.push(proposal_events.proposal_event_id);
    event_ids.extend(proposal_events.relation_event_ids);

    match draft.status {
        ImportedDecisionStatus::Proposed => {}
        ImportedDecisionStatus::Accepted => {
            event_ids.push(commands.accept_decision_with_uuid(
                &identities.decision_id,
                actor_id,
                identities.status_event_uuid,
            )?);
        }
        ImportedDecisionStatus::Rejected => {
            event_ids.push(commands.reject_decision_with_uuid(
                &identities.decision_id,
                actor_id,
                identities.status_event_uuid,
            )?);
        }
    }

    for (superseded_decision_id, event_uuid) in identities
        .supersedes_decision_ids
        .iter()
        .zip(identities.supersedes_event_uuids)
    {
        event_ids.push(commands.supersede_decision_with_uuid(
            superseded_decision_id,
            &identities.decision_id,
            actor_id,
            event_uuid,
        )?);
    }

    Ok(DocumentBlockImportReport {
        block_id: draft.block_id.clone(),
        decision_id: Some(identities.decision_id),
        status: DocumentBlockImportStatus::Imported,
        message: None,
        source_span: Some(draft.span),
        source_snippet: Some(draft.snippet.clone()),
        event_ids,
    })
}

#[derive(Debug, Clone)]
struct DocumentImportIdentities {
    decision_id: String,
    option_ids: Vec<String>,
    chosen_option_id: Option<String>,
    evidence_ids: Vec<String>,
    hypothesis_ids: Vec<String>,
    supersedes_decision_ids: Vec<String>,
    proposal_uuid: Uuid,
    evidence_event_uuids: Vec<Uuid>,
    hypothesis_event_uuids: Vec<Uuid>,
    proposal_event_uuids: DecisionProposalEventUuids,
    status_event_uuid: Uuid,
    supersedes_event_uuids: Vec<Uuid>,
}

impl DocumentImportIdentities {
    fn new(
        draft: &DocumentDecisionDraft,
        canonical_path: &str,
        source_hash: &str,
        namespace: &str,
    ) -> Result<Self> {
        let block_component = stable_component(&draft.block_id);
        let decision_id = stable_decision_id(namespace, &draft.block_id);
        let role_prefix = format!(
            "import:v1:{canonical_path}:{source_hash}:{}:{}-{}",
            draft.block_id, draft.span.byte_start, draft.span.byte_end
        );

        let option_ids = draft
            .option_labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                format!(
                    "option:document:{namespace}:{block_component}:{}-{}",
                    index + 1,
                    stable_component(label)
                )
            })
            .collect::<Vec<_>>();
        let chosen_option_id = match &draft.chosen_option_label {
            Some(label) => {
                let index = draft
                    .option_labels
                    .iter()
                    .position(|option| option == label)
                    .ok_or_else(|| {
                        CommandError::Validation(format!(
                            "chose/selected option '{label}' must match one of options"
                        ))
                    })?;
                Some(option_ids[index].clone())
            }
            None => None,
        };
        let evidence_ids = draft
            .evidence
            .iter()
            .enumerate()
            .map(|(index, content)| {
                format!(
                    "evidence:document:{namespace}:{block_component}:{}-{}",
                    index + 1,
                    stable_component(content)
                )
            })
            .collect::<Vec<_>>();
        let hypothesis_ids = draft
            .hypotheses
            .iter()
            .enumerate()
            .map(|(index, statement)| {
                format!(
                    "hypothesis:document:{namespace}:{block_component}:{}-{}",
                    index + 1,
                    stable_component(statement)
                )
            })
            .collect::<Vec<_>>();
        let supersedes_decision_ids = draft
            .supersedes
            .iter()
            .map(|id| stable_decision_reference(namespace, id))
            .collect::<Vec<_>>();
        let proposal_uuid = import_uuid(&format!("{role_prefix}:decision.proposed"));

        Ok(Self {
            decision_id,
            option_ids,
            chosen_option_id,
            evidence_ids,
            hypothesis_ids,
            supersedes_decision_ids,
            proposal_uuid,
            evidence_event_uuids: repeated_role_uuids(
                &role_prefix,
                "evidence.recorded",
                draft.evidence.len(),
            ),
            hypothesis_event_uuids: repeated_role_uuids(
                &role_prefix,
                "hypothesis.recorded",
                draft.hypotheses.len(),
            ),
            proposal_event_uuids: DecisionProposalEventUuids {
                proposal: proposal_uuid,
                has_option: repeated_role_uuids(
                    &role_prefix,
                    "relation.has_option",
                    draft.option_labels.len(),
                ),
                chose: draft
                    .chosen_option_label
                    .as_ref()
                    .map(|_| import_uuid(&format!("{role_prefix}:relation.chose"))),
                assumes: repeated_role_uuids(
                    &role_prefix,
                    "relation.assumes",
                    draft.hypotheses.len(),
                ),
                based_on: repeated_role_uuids(
                    &role_prefix,
                    "relation.based_on",
                    draft.evidence.len(),
                ),
            },
            status_event_uuid: import_uuid(&format!("{role_prefix}:decision.status")),
            supersedes_event_uuids: repeated_role_uuids(
                &role_prefix,
                "decision.superseded",
                draft.supersedes.len(),
            ),
        })
    }
}

#[derive(Debug, Clone)]
struct RawDocumentDecisionBlock {
    span: DocumentSourceSpan,
    text: String,
    lines: Vec<SourceLine>,
}

impl RawDocumentDecisionBlock {
    fn fallback_id(&self) -> String {
        format!("line-{}", self.span.line_start)
    }
}

#[derive(Debug, Clone)]
struct SourceLine {
    number: usize,
    byte_start: usize,
    text: String,
}

fn find_document_decision_blocks(input: &str) -> Vec<RawDocumentDecisionBlock> {
    let lines = source_lines(input);
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_decision_start(&line.text).then_some(index))
        .collect::<Vec<_>>();
    let mut blocks = Vec::with_capacity(starts.len());

    for (start_position, start_index) in starts.iter().copied().enumerate() {
        let end_index = starts
            .get(start_position + 1)
            .copied()
            .unwrap_or(lines.len());
        let byte_start = lines[start_index].byte_start;
        let byte_end = lines
            .get(end_index)
            .map(|line| line.byte_start)
            .unwrap_or_else(|| input.len());
        let line_end = lines[end_index.saturating_sub(1)].number;
        blocks.push(RawDocumentDecisionBlock {
            span: DocumentSourceSpan {
                byte_start,
                byte_end,
                line_start: lines[start_index].number,
                line_end,
            },
            text: input[byte_start..byte_end].to_owned(),
            lines: lines[start_index..end_index].to_vec(),
        });
    }

    blocks
}

fn source_lines(input: &str) -> Vec<SourceLine> {
    let mut lines = Vec::new();
    let mut byte_start = 0;
    for (index, line) in input.split_inclusive('\n').enumerate() {
        let text = line
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_owned();
        lines.push(SourceLine {
            number: index + 1,
            byte_start,
            text,
        });
        byte_start += line.len();
    }
    if !input.is_empty() && !input.ends_with('\n') && lines.is_empty() {
        lines.push(SourceLine {
            number: 1,
            byte_start: 0,
            text: input.to_owned(),
        });
    }
    lines
}

fn parse_document_decision_block(raw: &RawDocumentDecisionBlock) -> Result<DocumentDecisionDraft> {
    let mut fields = ParsedDecisionFields::default();
    let mut active_list: Option<ListField> = None;
    let mut active_scalar: Option<ScalarField> = None;

    for (index, source_line) in raw.lines.iter().enumerate() {
        let trimmed = source_line.text.trim();
        if trimmed.is_empty() {
            continue;
        }

        if index == 0 {
            if let Some(value) = marker_value(trimmed, "Decision") {
                if !value.is_empty() {
                    fields.title = Some(value.to_owned());
                }
                continue;
            }
        }

        if let Some(item) = trimmed.strip_prefix("- ") {
            let item = item.trim();
            if item.is_empty() {
                return Err(CommandError::Validation(format!(
                    "empty list item at line {}",
                    source_line.number
                ))
                .into());
            }
            match active_list {
                Some(ListField::Options) => fields.option_labels.push(item.to_owned()),
                Some(ListField::Evidence) => fields.evidence.push(item.to_owned()),
                Some(ListField::Hypotheses) => fields.hypotheses.push(item.to_owned()),
                Some(ListField::Supersedes) => fields.supersedes.push(item.to_owned()),
                None => {
                    return Err(CommandError::Validation(format!(
                        "list item without active field at line {}",
                        source_line.number
                    ))
                    .into());
                }
            }
            continue;
        }

        if let Some((raw_key, value)) = trimmed.split_once(':') {
            let key = normalized_marker_key(raw_key);
            let value = value.trim();
            active_list = None;
            active_scalar = None;
            match key.as_str() {
                "decision" if !value.is_empty() => fields.title = Some(value.to_owned()),
                "decision" => {}
                "id" => fields.block_id = non_empty_value(value, "id")?,
                "title" => fields.title = non_empty_value(value, "title")?,
                "status" => fields.status = non_empty_value(value, "status")?,
                "actor" | "actor-id" => fields.actor_id = non_empty_value(value, "actor")?,
                "topic" | "topics" | "topic-keys" => {
                    fields.topic_keys = split_document_marker_list(value);
                }
                "rationale" => {
                    if value.is_empty() {
                        fields.rationale_lines.clear();
                        active_scalar = Some(ScalarField::Rationale);
                    } else {
                        fields.rationale_lines = vec![value.to_owned()];
                    }
                }
                "options" => {
                    if value.is_empty() {
                        active_list = Some(ListField::Options);
                    } else {
                        fields.option_labels = split_document_marker_list(value);
                    }
                }
                "chose" | "chosen" | "selected" => {
                    fields.chosen_option_label = non_empty_value(value, "chose")?;
                }
                "evidence" => {
                    if value.is_empty() {
                        active_list = Some(ListField::Evidence);
                    } else {
                        fields.evidence = split_document_marker_list(value);
                    }
                }
                "hypothesis" | "hypotheses" => {
                    if value.is_empty() {
                        active_list = Some(ListField::Hypotheses);
                    } else {
                        fields.hypotheses = split_document_marker_list(value);
                    }
                }
                "supersedes" => {
                    if value.is_empty() {
                        active_list = Some(ListField::Supersedes);
                    } else {
                        fields.supersedes = split_document_marker_list(value);
                    }
                }
                _ => {}
            }
            continue;
        }

        if active_scalar == Some(ScalarField::Rationale)
            && source_line
                .text
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            fields.rationale_lines.push(trimmed.to_owned());
        }
    }

    let block_id = required_field(fields.block_id, "id")?;
    let title = required_field(fields.title, "title")?;
    let status = ImportedDecisionStatus::parse(&required_field(fields.status, "status")?)?;
    let rationale = fields.rationale_lines.join(" ").trim().to_owned();
    if rationale.is_empty() {
        return Err(CommandError::Validation("rationale is required".to_owned()).into());
    }
    if fields.option_labels.is_empty() {
        return Err(CommandError::Validation(
            "options must contain at least one option".to_owned(),
        )
        .into());
    }
    if fields.topic_keys.is_empty() {
        fields.topic_keys.push("document".to_owned());
    }

    Ok(DocumentDecisionDraft {
        block_id,
        title,
        status,
        original_actor_id: fields.actor_id,
        topic_keys: fields.topic_keys,
        rationale,
        option_labels: fields.option_labels,
        chosen_option_label: fields.chosen_option_label,
        evidence: fields.evidence,
        hypotheses: fields.hypotheses,
        supersedes: fields.supersedes,
        span: raw.span,
        snippet: compact_snippet(&raw.text),
    })
}

#[derive(Default)]
struct ParsedDecisionFields {
    block_id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    actor_id: Option<String>,
    topic_keys: Vec<String>,
    rationale_lines: Vec<String>,
    option_labels: Vec<String>,
    chosen_option_label: Option<String>,
    evidence: Vec<String>,
    hypotheses: Vec<String>,
    supersedes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListField {
    Options,
    Evidence,
    Hypotheses,
    Supersedes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarField {
    Rationale,
}

fn is_decision_start(line: &str) -> bool {
    line.trim_start()
        .split_once(':')
        .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("Decision"))
}

fn normalized_marker_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

fn non_empty_value(value: &str, field: &'static str) -> Result<Option<String>> {
    if value.trim().is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(Some(value.trim().to_owned()))
    }
}

fn required_field(value: Option<String>, field: &'static str) -> Result<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CommandError::Validation(format!("{field} is required")).into())
}

fn split_document_marker_list(value: &str) -> Vec<String> {
    value
        .split([',', '|'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn compact_snippet(input: &str) -> String {
    let mut snippet = String::new();
    let mut last_was_whitespace = false;
    let mut char_count = 0;
    for character in input.chars() {
        if char_count >= 240 {
            break;
        }
        if character.is_whitespace() {
            if !last_was_whitespace && !snippet.is_empty() {
                snippet.push(' ');
                last_was_whitespace = true;
                char_count += 1;
            }
        } else {
            snippet.push(character);
            last_was_whitespace = false;
            char_count += 1;
        }
    }
    snippet.trim().to_owned()
}

fn accumulate_file_summary(summary: &mut DocumentImportSummary, file: &DocumentFileImportReport) {
    if matches!(
        file.status,
        DocumentFileImportStatus::SkippedUnmarked | DocumentFileImportStatus::SkippedUnsupported
    ) {
        summary.files_skipped += 1;
    }
    summary.blocks_seen += file.blocks.len();
    for block in &file.blocks {
        summary.events_written += block.event_ids.len();
        match block.status {
            DocumentBlockImportStatus::Imported => summary.blocks_imported += 1,
            DocumentBlockImportStatus::NoOp => summary.blocks_noop += 1,
            DocumentBlockImportStatus::Conflict => summary.blocks_conflicted += 1,
            DocumentBlockImportStatus::DuplicateCandidate => summary.duplicate_candidates += 1,
            DocumentBlockImportStatus::ValidationError => summary.validation_errors += 1,
        }
    }
}

fn event_uuid_exists<L: EventLedger>(ledger: &L, event_uuid: Uuid) -> Result<bool> {
    scan_ledger(ledger, |event| event.event_uuid == event_uuid)
}

fn decision_id_exists<L: EventLedger>(ledger: &L, decision_id: &str) -> Result<bool> {
    scan_ledger(ledger, |event| {
        event.event_type == EventType::DecisionProposed
            && event
                .payload
                .get("decision_id")
                .and_then(|value| value.as_str())
                == Some(decision_id)
    })
}

fn find_document_duplicate_candidate<L: EventLedger>(
    ledger: &L,
    canonical_path: &str,
    source_hash: &str,
    block_id: &str,
) -> Result<Option<String>> {
    let mut offset = 0;
    const PAGE_SIZE: usize = 1024;

    loop {
        let events = ledger.read(offset, PAGE_SIZE)?;
        if events.is_empty() {
            return Ok(None);
        }

        for event in &events {
            if event.event_type != EventType::DecisionProposed
                || event.source != EventSource::Document
            {
                continue;
            }
            let Some(source_ref) = event.source_ref.as_deref() else {
                continue;
            };
            let Ok(source_ref) = serde_json::from_str::<DocumentSourceRef>(source_ref) else {
                continue;
            };
            if source_ref.sha256 == source_hash
                && source_ref.block_id == block_id
                && source_ref.path != canonical_path
            {
                return Ok(Some(source_ref.path));
            }
        }

        if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
            offset = last_event_id;
        } else {
            return Ok(None);
        }
    }
}

fn scan_ledger<L: EventLedger>(ledger: &L, predicate: impl Fn(&Event) -> bool) -> Result<bool> {
    let mut offset = 0;
    const PAGE_SIZE: usize = 1024;

    loop {
        let events = ledger.read(offset, PAGE_SIZE)?;
        if events.is_empty() {
            return Ok(false);
        }

        for event in &events {
            if predicate(event) {
                return Ok(true);
            }
        }

        if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
            offset = last_event_id;
        } else {
            return Ok(false);
        }
    }
}

fn stable_decision_id(namespace: &str, block_id: &str) -> String {
    format!(
        "decision:document:{namespace}:{}",
        stable_component(block_id)
    )
}

fn stable_decision_reference(namespace: &str, raw_id: &str) -> String {
    let trimmed = raw_id.trim();
    if trimmed.starts_with("decision:") || trimmed.starts_with("decision-") {
        trimmed.to_owned()
    } else {
        stable_decision_id(namespace, trimmed)
    }
}

fn document_namespace(canonical_path: &str) -> String {
    let path = Path::new(canonical_path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(stable_component)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "document".to_owned());
    format!("{stem}-{}", &sha256_hex(canonical_path.as_bytes())[..12])
}

fn stable_component(input: &str) -> String {
    let transliterated = deunicode::deunicode(input);
    let mut normalized = String::new();
    let mut last_was_separator = false;
    for character in transliterated.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            last_was_separator = false;
        } else if !normalized.is_empty() && !last_was_separator {
            normalized.push('-');
            last_was_separator = true;
        }
        if normalized.len() >= 80 {
            break;
        }
    }
    while normalized.ends_with('-') {
        normalized.pop();
    }
    if normalized.is_empty() {
        sha256_hex(input.as_bytes())[..12].to_owned()
    } else {
        normalized
    }
}

fn repeated_role_uuids(role_prefix: &str, role: &str, count: usize) -> Vec<Uuid> {
    (0..count)
        .map(|index| import_uuid(&format!("{role_prefix}:{role}:{}", index + 1)))
        .collect()
}

fn import_uuid(key: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, key.as_bytes())
}

fn import_run_id(files: &[PathBuf]) -> String {
    let seed = files
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "import:{}:{}",
        Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        &sha256_hex(seed.as_bytes())[..12]
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests;
