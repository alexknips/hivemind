use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::commands::{Commands, DecisionProposalEventUuids};
use crate::error::{CliError, CommandError};
use crate::events::{self, Event, EventPayload, EventProvenance, EventSource, EventType};
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
        let _ = writeln!(
            context,
            "{} {}: {}",
            message.ts,
            message.user_id,
            message.text.trim()
        );
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
        let mut description = String::with_capacity(
            "Slack option '' captured from ".len() + label.len() + draft.source_ref.len(),
        );
        let _ = write!(
            description,
            "Slack option '{label}' captured from {}",
            draft.source_ref
        );
        let option_id = commands.record_option(&draft.actor_id, label, &description)?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentPreparationFormat {
    Auto,
    Pdf,
    Text,
    OcrText,
}

impl DocumentPreparationFormat {
    fn source_kind_for_path(self, path: &Path) -> Option<DocumentPreparationSourceKind> {
        match self {
            Self::Pdf => Some(DocumentPreparationSourceKind::PdfText),
            Self::Text => Some(DocumentPreparationSourceKind::Text),
            Self::OcrText => Some(DocumentPreparationSourceKind::OcrText),
            Self::Auto => {
                let extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(|extension| extension.to_ascii_lowercase());
                let file_name = path
                    .file_name()
                    .and_then(|file_name| file_name.to_str())
                    .map(|file_name| file_name.to_ascii_lowercase())
                    .unwrap_or_default();
                match extension.as_deref() {
                    Some("pdf") => Some(DocumentPreparationSourceKind::PdfText),
                    Some("ocr") => Some(DocumentPreparationSourceKind::OcrText),
                    Some("txt") | Some("md") | Some("markdown") => {
                        if file_name.contains(".ocr.") || file_name.ends_with("-ocr.txt") {
                            Some(DocumentPreparationSourceKind::OcrText)
                        } else {
                            Some(DocumentPreparationSourceKind::Text)
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    fn accepts_path(self, path: &Path) -> bool {
        self.source_kind_for_path(path).is_some()
    }
}

#[derive(Debug, Clone)]
pub struct DocumentImportRequest {
    pub paths: Vec<PathBuf>,
    pub importer_actor_id: String,
    pub format: DocumentImportFormat,
    pub conflict_resolution: DocumentConflictResolutionAction,
}

#[derive(Debug, Clone)]
pub struct DocumentPreparationRequest {
    pub paths: Vec<PathBuf>,
    pub format: DocumentPreparationFormat,
    pub output_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentPreparationReport {
    pub preparation_run_id: String,
    pub summary: DocumentPreparationSummary,
    pub files: Vec<DocumentPreparedFileReport>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DocumentPreparationSummary {
    pub files_seen: usize,
    pub files_prepared: usize,
    pub files_review_required: usize,
    pub files_needing_ocr: usize,
    pub files_skipped: usize,
    pub validation_errors: usize,
    pub pages_seen: usize,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentPreparedFileReport {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<DocumentPreparationSourceKind>,
    pub status: DocumentPreparationFileStatus,
    pub review_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepared_path: Option<String>,
    pub pages: Vec<DocumentPreparedPageReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intermediate_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPreparationFileStatus {
    Prepared,
    ReviewRequired,
    NeedsOcr,
    SkippedUnsupported,
    ValidationError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPreparationSourceKind {
    PdfText,
    Text,
    OcrText,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentPreparedPageReport {
    pub page_number: usize,
    pub source_span: DocumentSourceSpan,
    pub source_snippet: String,
    #[serde(skip)]
    pub text: String,
    pub review_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_confidence: Option<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ocr_uncertainty: Vec<String>,
    pub source_ref: DocumentPreparedSourceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPreparedSourceRef {
    pub source: String,
    pub path: String,
    pub sha256: String,
    pub preparation_run_id: String,
    pub extraction_kind: DocumentPreparationSourceKind,
    pub page_number: usize,
    pub source_span: DocumentSourceSpan,
    pub source_snippet: String,
    pub ocr_review_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_confidence: Option<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ocr_uncertainty: Vec<String>,
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
    pub blocks_resolved: usize,
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
    pub reviewer_action: Option<DocumentReviewerAction>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub similarity_matches: Vec<DocumentSimilarityMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_span: Option<DocumentSourceSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_snippet: Option<String>,
    pub event_ids: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<DocumentImportConflictReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentBlockImportStatus {
    Imported,
    NoOp,
    Conflict,
    ConflictKeptExisting,
    ConflictSuperseded,
    ConflictContested,
    ConflictContextAdded,
    DuplicateCandidate,
    ValidationError,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentConflictResolutionAction {
    #[default]
    Report,
    KeepExisting,
    Supersede,
    Contest,
    AddContext,
}

impl DocumentConflictResolutionAction {
    fn available_actions() -> Vec<Self> {
        vec![
            Self::KeepExisting,
            Self::Supersede,
            Self::Contest,
            Self::AddContext,
        ]
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::KeepExisting => "keep_existing",
            Self::Supersede => "supersede",
            Self::Contest => "contest",
            Self::AddContext => "add_context",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentConflictDecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Contested,
    Superseded,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentImportConflictReport {
    pub selected_action: DocumentConflictResolutionAction,
    pub available_actions: Vec<DocumentConflictResolutionAction>,
    pub existing: DocumentConflictExistingItem,
    pub proposed_update: DocumentConflictProposedUpdate,
    pub affected_dependencies: DocumentConflictAffectedDependencies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_decision_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentConflictExistingItem {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub status: DocumentConflictDecisionStatus,
    pub actor_id: String,
    pub event_origin: u64,
    pub source: EventSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentConflictProposedUpdate {
    pub decision_id: String,
    pub title: String,
    pub status: ImportedDecisionStatus,
    pub topic_keys: Vec<String>,
    pub rationale: String,
    pub option_labels: Vec<String>,
    pub evidence: Vec<String>,
    pub hypotheses: Vec<String>,
    pub supersedes: Vec<String>,
    pub source: DocumentConflictSourceProvenance,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentConflictSourceProvenance {
    pub source: String,
    pub path: String,
    pub sha256: String,
    pub import_run_id: String,
    pub block_id: String,
    pub source_span: DocumentSourceSpan,
    pub source_snippet: String,
    pub importer_actor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_actor_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DocumentConflictAffectedDependencies {
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypothesis_ids: Vec<String>,
    pub supersedes_decision_ids: Vec<String>,
    pub superseded_by_decision_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentReviewerAction {
    ResolveImportConflict,
    ReviewFuzzyDuplicateCandidate,
    ReviewAmbiguousFuzzyMatches,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentSimilarityMatch {
    pub decision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_origin: Option<u64>,
    pub score: u32,
    pub review_required: bool,
    pub basis: DocumentSimilarityBasis,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentSimilarityBasis {
    pub algorithm: &'static str,
    pub title_token_overlap: u32,
    pub rationale_token_overlap: u32,
    pub topic_key_overlap: u32,
    pub same_stable_block_id: bool,
    pub matched_fields: Vec<&'static str>,
    pub source_path: String,
    pub source_block_id: String,
    pub source_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conflict_resolution: Option<DocumentConflictSourceRefResolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepared_from: Option<DocumentPreparedSourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentConflictSourceRefResolution {
    action: DocumentConflictResolutionAction,
    existing_decision_id: String,
    resolved_decision_id: Option<String>,
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
    prepared_source_ref: Option<DocumentPreparedSourceRef>,
}

struct DocumentImportContext<'a> {
    canonical_path: &'a str,
    source_hash: &'a str,
    namespace: &'a str,
    importer_actor_id: &'a str,
    import_run_id: &'a str,
}

struct ConflictBlockOutcome<'a> {
    status: DocumentBlockImportStatus,
    message: &'a str,
}

impl<'a> ConflictBlockOutcome<'a> {
    fn new(status: DocumentBlockImportStatus, message: &'a str) -> Self {
        Self { status, message }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportedDecisionStatus {
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

pub fn prepare_document_texts(
    request: &DocumentPreparationRequest,
) -> Result<DocumentPreparationReport> {
    let files = collect_document_preparation_paths(&request.paths, request.format)?;
    let preparation_run_id = preparation_run_id(&files);
    let mut summary = DocumentPreparationSummary {
        files_seen: files.len(),
        ..DocumentPreparationSummary::default()
    };
    let mut file_reports = Vec::with_capacity(files.len());

    if let Some(output_dir) = &request.output_dir {
        fs::create_dir_all(output_dir).map_err(|error| {
            CliError::InvalidInput(format!(
                "cannot create prepared document output directory {}: {error}",
                output_dir.display()
            ))
        })?;
    }

    for file in files {
        let report = prepare_document_file(&file, request, &preparation_run_id)?;
        accumulate_preparation_summary(&mut summary, &report);
        file_reports.push(report);
    }

    Ok(DocumentPreparationReport {
        preparation_run_id,
        summary,
        files: file_reports,
    })
}

fn collect_document_preparation_paths(
    paths: &[PathBuf],
    format: DocumentPreparationFormat,
) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Err(CliError::InvalidInput(
            "import prepare-documents requires at least one --file or path".to_owned(),
        )
        .into());
    }

    let mut files = Vec::new();
    for path in paths {
        collect_document_preparation_path(path, format, true, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_document_preparation_path(
    path: &Path,
    format: DocumentPreparationFormat,
    explicit: bool,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|error| {
        CliError::InvalidInput(format!(
            "cannot read document preparation path {}: {error}",
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
                    "cannot list document preparation directory {}: {error}",
                    path.display()
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot read document preparation directory entry in {}: {error}",
                    path.display()
                ))
            })?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            collect_document_preparation_path(&entry.path(), format, false, files)?;
        }
        return Ok(());
    }

    Err(CliError::InvalidInput(format!(
        "document preparation path is neither file nor directory: {}",
        path.display()
    ))
    .into())
}

fn prepare_document_file(
    path: &Path,
    request: &DocumentPreparationRequest,
    preparation_run_id: &str,
) -> Result<DocumentPreparedFileReport> {
    let Some(source_kind) = request.format.source_kind_for_path(path) else {
        return Ok(DocumentPreparedFileReport {
            path: path.display().to_string(),
            canonical_path: None,
            source_hash: None,
            source_kind: None,
            status: DocumentPreparationFileStatus::SkippedUnsupported,
            review_required: false,
            message: Some("unsupported document preparation extension".to_owned()),
            prepared_path: None,
            pages: Vec::new(),
            intermediate_text: None,
        });
    };

    let bytes = fs::read(path).map_err(|error| {
        CliError::InvalidInput(format!(
            "cannot read document preparation source {}: {error}",
            path.display()
        ))
    })?;
    let source_hash = sha256_hex(&bytes);
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        CliError::InvalidInput(format!(
            "cannot canonicalize document preparation source {}: {error}",
            path.display()
        ))
    })?;
    let canonical_path = canonical_path.display().to_string();

    let extracted_pages = match source_kind {
        DocumentPreparationSourceKind::PdfText => match pdf_extract::extract_text_by_pages(path) {
            Ok(pages) => pages,
            Err(error) => {
                return Ok(DocumentPreparedFileReport {
                    path: path.display().to_string(),
                    canonical_path: Some(canonical_path),
                    source_hash: Some(source_hash),
                    source_kind: Some(source_kind),
                    status: DocumentPreparationFileStatus::ValidationError,
                    review_required: false,
                    message: Some(format!("could not extract PDF text: {error}")),
                    prepared_path: None,
                    pages: Vec::new(),
                    intermediate_text: None,
                });
            }
        },
        DocumentPreparationSourceKind::Text | DocumentPreparationSourceKind::OcrText => {
            vec![String::from_utf8(bytes).map_err(|error| {
                CliError::InvalidInput(format!(
                    "document preparation source {} is not valid UTF-8: {error}",
                    path.display()
                ))
            })?]
        }
    };

    if source_kind == DocumentPreparationSourceKind::PdfText
        && extracted_pages.iter().all(|page| page.trim().is_empty())
    {
        return Ok(DocumentPreparedFileReport {
            path: path.display().to_string(),
            canonical_path: Some(canonical_path),
            source_hash: Some(source_hash),
            source_kind: Some(source_kind),
            status: DocumentPreparationFileStatus::NeedsOcr,
            review_required: true,
            message: Some(
                "PDF has no extractable text layer; run OCR and prepare the OCR text output"
                    .to_owned(),
            ),
            prepared_path: None,
            pages: Vec::new(),
            intermediate_text: None,
        });
    }

    let pages = extracted_pages
        .into_iter()
        .enumerate()
        .filter_map(|(index, text)| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return None;
            }
            let page_number = index + 1;
            Some(prepared_page_report(
                &canonical_path,
                &source_hash,
                preparation_run_id,
                source_kind,
                page_number,
                &text,
            ))
        })
        .collect::<Vec<_>>();

    if pages.is_empty() {
        return Ok(DocumentPreparedFileReport {
            path: path.display().to_string(),
            canonical_path: Some(canonical_path),
            source_hash: Some(source_hash),
            source_kind: Some(source_kind),
            status: DocumentPreparationFileStatus::NeedsOcr,
            review_required: true,
            message: Some("no usable extracted text; OCR review is required".to_owned()),
            prepared_path: None,
            pages,
            intermediate_text: None,
        });
    }

    let review_required = pages.iter().any(|page| page.review_required);
    let intermediate_text = render_prepared_document_text(preparation_run_id, &pages)?;
    let (prepared_path, bytes_written) = maybe_write_prepared_document(
        path,
        request.output_dir.as_deref(),
        &source_hash,
        &intermediate_text,
    )?;
    let status = if review_required {
        DocumentPreparationFileStatus::ReviewRequired
    } else {
        DocumentPreparationFileStatus::Prepared
    };

    let mut report = DocumentPreparedFileReport {
        path: path.display().to_string(),
        canonical_path: Some(canonical_path),
        source_hash: Some(source_hash),
        source_kind: Some(source_kind),
        status,
        review_required,
        message: review_required
            .then(|| "OCR uncertainty must be reviewed before import".to_owned()),
        prepared_path,
        pages,
        intermediate_text: Some(intermediate_text),
    };
    if bytes_written > 0 {
        report
            .message
            .get_or_insert_with(|| "prepared intermediate text written for review".to_owned());
    }
    Ok(report)
}

fn prepared_page_report(
    canonical_path: &str,
    source_hash: &str,
    preparation_run_id: &str,
    source_kind: DocumentPreparationSourceKind,
    page_number: usize,
    text: &str,
) -> DocumentPreparedPageReport {
    let span = span_for_prepared_text(text);
    let source_snippet = compact_snippet(text);
    let ocr_uncertainty = if source_kind == DocumentPreparationSourceKind::OcrText {
        vec!["ocr_confidence_unavailable".to_owned()]
    } else {
        Vec::new()
    };
    let review_required = !ocr_uncertainty.is_empty();
    let source_ref = DocumentPreparedSourceRef {
        source: "document_preparation".to_owned(),
        path: canonical_path.to_owned(),
        sha256: source_hash.to_owned(),
        preparation_run_id: preparation_run_id.to_owned(),
        extraction_kind: source_kind,
        page_number,
        source_span: span,
        source_snippet: source_snippet.clone(),
        ocr_review_required: review_required,
        ocr_confidence: None,
        ocr_uncertainty: ocr_uncertainty.clone(),
    };

    DocumentPreparedPageReport {
        page_number,
        source_span: span,
        source_snippet,
        text: text.to_owned(),
        review_required,
        ocr_confidence: None,
        ocr_uncertainty,
        source_ref,
    }
}

fn render_prepared_document_text(
    preparation_run_id: &str,
    pages: &[DocumentPreparedPageReport],
) -> Result<String> {
    let mut output = String::new();
    output.push_str("# hivemind-prepared-document: v1\n");
    output.push_str(&format!("# preparation_run_id: {preparation_run_id}\n"));
    output.push_str("# reviewer_note: Review this extracted text before importing with `hivemind import documents`.\n\n");

    for page in pages {
        let source_ref = serde_json::to_string(&page.source_ref).map_err(|error| {
            CommandError::Validation(format!("prepared source_ref serialization failed: {error}"))
        })?;
        output.push_str(&format!("# hivemind-source-ref: {source_ref}\n"));
        output.push_str(&format!("# source_page: {}\n", page.page_number));
        if page.review_required {
            output.push_str("# ocr_review_required: true\n");
            for uncertainty in &page.ocr_uncertainty {
                output.push_str(&format!("# ocr_uncertainty: {uncertainty}\n"));
            }
        }
        output.push_str(page.text.trim_end());
        output.push_str("\n\n");
    }

    Ok(output)
}

fn maybe_write_prepared_document(
    source_path: &Path,
    output_dir: Option<&Path>,
    source_hash: &str,
    intermediate_text: &str,
) -> Result<(Option<String>, usize)> {
    let Some(output_dir) = output_dir else {
        return Ok((None, 0));
    };

    let output_path = prepared_output_path(source_path, output_dir, source_hash);
    fs::write(&output_path, intermediate_text).map_err(|error| {
        CliError::InvalidInput(format!(
            "cannot write prepared document {}: {error}",
            output_path.display()
        ))
    })?;
    Ok((
        Some(output_path.display().to_string()),
        intermediate_text.len(),
    ))
}

fn prepared_output_path(source_path: &Path, output_dir: &Path, source_hash: &str) -> PathBuf {
    let stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(stable_component)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "document".to_owned());
    output_dir.join(format!("{stem}-{}.prepared.txt", &source_hash[..12]))
}

fn span_for_prepared_text(text: &str) -> DocumentSourceSpan {
    DocumentSourceSpan {
        byte_start: 0,
        byte_end: text.len(),
        line_start: 1,
        line_end: text.lines().count().max(1),
    }
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
    let context = DocumentImportContext {
        canonical_path: &canonical_path,
        source_hash: &source_hash,
        namespace: &namespace,
        importer_actor_id: request.importer_actor_id.trim(),
        import_run_id,
    };
    let mut block_reports = Vec::with_capacity(raw_blocks.len());
    for raw_block in raw_blocks {
        let block_report = match parse_document_decision_block(&raw_block) {
            Ok(draft) => import_document_decision_block(
                ledger,
                &draft,
                &context,
                request.conflict_resolution,
            )?,
            Err(error) => DocumentBlockImportReport {
                block_id: raw_block.fallback_id(),
                decision_id: None,
                status: DocumentBlockImportStatus::ValidationError,
                message: Some(error.to_string()),
                reviewer_action: None,
                similarity_matches: Vec::new(),
                source_span: Some(raw_block.span),
                source_snippet: Some(compact_snippet(&raw_block.text)),
                event_ids: Vec::new(),
                conflict: None,
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
    context: &DocumentImportContext<'_>,
    conflict_resolution: DocumentConflictResolutionAction,
) -> Result<DocumentBlockImportReport> {
    let identities = DocumentImportIdentities::new(
        draft,
        context.canonical_path,
        context.source_hash,
        context.namespace,
    )?;

    if event_uuid_exists(ledger, identities.proposal_uuid)? {
        return Ok(DocumentBlockImportReport {
            block_id: draft.block_id.clone(),
            decision_id: Some(identities.decision_id),
            status: DocumentBlockImportStatus::NoOp,
            message: Some("identical decision block already imported".to_owned()),
            reviewer_action: None,
            similarity_matches: Vec::new(),
            source_span: Some(draft.span),
            source_snippet: Some(draft.snippet.clone()),
            event_ids: Vec::new(),
            conflict: None,
        });
    }

    if let Some(existing_path) = find_document_duplicate_candidate(
        ledger,
        context.canonical_path,
        context.source_hash,
        &draft.block_id,
    )? {
        return Ok(DocumentBlockImportReport {
            block_id: draft.block_id.clone(),
            decision_id: Some(identities.decision_id),
            status: DocumentBlockImportStatus::DuplicateCandidate,
            message: Some(format!(
                "same source hash and block id were already imported from {existing_path}"
            )),
            reviewer_action: None,
            similarity_matches: Vec::new(),
            source_span: Some(draft.span),
            source_snippet: Some(draft.snippet.clone()),
            event_ids: Vec::new(),
            conflict: None,
        });
    }

    let similarity_matches =
        find_document_similarity_matches(ledger, draft, &identities.decision_id)?;

    if decision_id_exists(ledger, &identities.decision_id)? {
        return resolve_document_import_conflict(
            ledger,
            draft,
            &identities.decision_id,
            context,
            conflict_resolution,
            similarity_matches,
        );
    }

    if !similarity_matches.is_empty() {
        let reviewer_action = if similarity_matches.len() > 1 {
            DocumentReviewerAction::ReviewAmbiguousFuzzyMatches
        } else {
            DocumentReviewerAction::ReviewFuzzyDuplicateCandidate
        };
        return Ok(DocumentBlockImportReport {
            block_id: draft.block_id.clone(),
            decision_id: Some(identities.decision_id),
            status: DocumentBlockImportStatus::DuplicateCandidate,
            message: Some("fuzzy duplicate candidate requires explicit reviewer action".to_owned()),
            reviewer_action: Some(reviewer_action),
            similarity_matches,
            source_span: Some(draft.span),
            source_snippet: Some(draft.snippet.clone()),
            event_ids: Vec::new(),
            conflict: None,
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
                reviewer_action: None,
                similarity_matches: Vec::new(),
                source_span: Some(draft.span),
                source_snippet: Some(draft.snippet.clone()),
                event_ids: Vec::new(),
                conflict: None,
            });
        }
    }

    let actor_id = draft
        .original_actor_id
        .as_deref()
        .unwrap_or(context.importer_actor_id);
    let source_ref = document_source_ref(
        draft,
        context.canonical_path,
        context.source_hash,
        context.importer_actor_id,
        context.import_run_id,
        None,
    )?;
    let event_ids =
        write_document_decision_events(ledger, draft, &identities, actor_id, &source_ref)?;

    Ok(DocumentBlockImportReport {
        block_id: draft.block_id.clone(),
        decision_id: Some(identities.decision_id),
        status: DocumentBlockImportStatus::Imported,
        message: draft.prepared_source_ref.as_ref().and_then(|source_ref| {
            source_ref
                .ocr_review_required
                .then(|| "prepared OCR source requires reviewer verification".to_owned())
        }),
        reviewer_action: None,
        similarity_matches: Vec::new(),
        source_span: Some(draft.span),
        source_snippet: Some(draft.snippet.clone()),
        event_ids,
        conflict: None,
    })
}

fn document_source_ref(
    draft: &DocumentDecisionDraft,
    canonical_path: &str,
    source_hash: &str,
    importer_actor_id: &str,
    import_run_id: &str,
    conflict_resolution: Option<DocumentConflictSourceRefResolution>,
) -> Result<String> {
    serde_json::to_string(&DocumentSourceRef {
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
        conflict_resolution,
        prepared_from: draft.prepared_source_ref.clone(),
    })
    .map_err(|error| {
        CommandError::Validation(format!("source_ref serialization failed: {error}")).into()
    })
}

fn write_document_decision_events<L: EventLedger>(
    ledger: &L,
    draft: &DocumentDecisionDraft,
    identities: &DocumentImportIdentities,
    actor_id: &str,
    source_ref: &str,
) -> Result<Vec<u64>> {
    let commands =
        Commands::new_with_provenance(ledger, EventProvenance::document(source_ref.to_owned()));
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
            Some(source_ref),
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

    let mut imported_option_description =
        String::with_capacity("Option imported from document block ".len() + draft.block_id.len());
    imported_option_description.push_str("Option imported from document block ");
    imported_option_description.push_str(&draft.block_id);
    for (option_id, label) in identities.option_ids.iter().zip(&draft.option_labels) {
        commands.record_option_with_id(actor_id, option_id, label, &imported_option_description)?;
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
        identities.proposal_event_uuids.clone(),
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
        .zip(identities.supersedes_event_uuids.clone())
    {
        event_ids.push(commands.supersede_decision_with_uuid(
            superseded_decision_id,
            &identities.decision_id,
            actor_id,
            event_uuid,
        )?);
    }

    Ok(event_ids)
}

fn resolve_document_import_conflict<L: EventLedger>(
    ledger: &L,
    draft: &DocumentDecisionDraft,
    existing_decision_id: &str,
    context: &DocumentImportContext<'_>,
    selected_action: DocumentConflictResolutionAction,
    similarity_matches: Vec<DocumentSimilarityMatch>,
) -> Result<DocumentBlockImportReport> {
    let existing = existing_document_conflict_item(ledger, existing_decision_id)?;
    let affected_dependencies = affected_dependencies_for_decision(ledger, existing_decision_id)?;
    let proposed_update = proposed_conflict_update(
        draft,
        existing_decision_id,
        context.canonical_path,
        context.source_hash,
        context.importer_actor_id,
        context.import_run_id,
    );
    let mut conflict = DocumentImportConflictReport {
        selected_action,
        available_actions: DocumentConflictResolutionAction::available_actions(),
        existing,
        proposed_update,
        affected_dependencies,
        resolved_decision_id: None,
    };
    let report_reviewer_action = match selected_action {
        DocumentConflictResolutionAction::Report if similarity_matches.len() > 1 => {
            Some(DocumentReviewerAction::ReviewAmbiguousFuzzyMatches)
        }
        DocumentConflictResolutionAction::Report => {
            Some(DocumentReviewerAction::ResolveImportConflict)
        }
        _ => None,
    };

    match selected_action {
        DocumentConflictResolutionAction::Report => conflict_block_report(
            draft,
            existing_decision_id,
            ConflictBlockOutcome::new(
                DocumentBlockImportStatus::Conflict,
                "stable decision id already exists with different imported content",
            ),
            Vec::new(),
            ConflictBlockReportDetails {
                reviewer_action: report_reviewer_action,
                similarity_matches,
                conflict,
            },
        ),
        DocumentConflictResolutionAction::KeepExisting => conflict_block_report(
            draft,
            existing_decision_id,
            ConflictBlockOutcome::new(
                DocumentBlockImportStatus::ConflictKeptExisting,
                "kept existing ledger-derived decision; no events written",
            ),
            Vec::new(),
            ConflictBlockReportDetails {
                reviewer_action: None,
                similarity_matches,
                conflict,
            },
        ),
        DocumentConflictResolutionAction::Supersede => {
            let identities = DocumentImportIdentities::new_conflict_supersession(
                draft,
                context.canonical_path,
                context.source_hash,
                context.namespace,
                existing_decision_id,
            )?;
            if let Some(missing_decision_id) = missing_superseded_decision(ledger, &identities)? {
                return conflict_block_report(
                    draft,
                    existing_decision_id,
                    ConflictBlockOutcome::new(
                        DocumentBlockImportStatus::ValidationError,
                        &format!("superseded decision does not exist: {missing_decision_id}"),
                    ),
                    Vec::new(),
                    ConflictBlockReportDetails {
                        reviewer_action: None,
                        similarity_matches,
                        conflict,
                    },
                );
            }
            let resolved_decision_id = identities.decision_id.clone();
            let source_ref = document_source_ref(
                draft,
                context.canonical_path,
                context.source_hash,
                context.importer_actor_id,
                context.import_run_id,
                Some(DocumentConflictSourceRefResolution {
                    action: selected_action,
                    existing_decision_id: existing_decision_id.to_owned(),
                    resolved_decision_id: Some(resolved_decision_id.clone()),
                }),
            )?;
            let actor_id = draft
                .original_actor_id
                .as_deref()
                .unwrap_or(context.importer_actor_id);
            let event_ids =
                write_document_decision_events(ledger, draft, &identities, actor_id, &source_ref)?;
            conflict.resolved_decision_id = Some(resolved_decision_id);
            conflict_block_report(
                draft,
                existing_decision_id,
                ConflictBlockOutcome::new(
                    DocumentBlockImportStatus::ConflictSuperseded,
                    "captured proposed update as a superseding decision",
                ),
                event_ids,
                ConflictBlockReportDetails {
                    reviewer_action: None,
                    similarity_matches,
                    conflict,
                },
            )
        }
        DocumentConflictResolutionAction::Contest => {
            let source_ref = document_source_ref(
                draft,
                context.canonical_path,
                context.source_hash,
                context.importer_actor_id,
                context.import_run_id,
                Some(DocumentConflictSourceRefResolution {
                    action: selected_action,
                    existing_decision_id: existing_decision_id.to_owned(),
                    resolved_decision_id: None,
                }),
            )?;
            let commands =
                Commands::new_with_provenance(ledger, EventProvenance::document(source_ref));
            let event_uuid = conflict_resolution_uuid(
                selected_action,
                context.canonical_path,
                context.source_hash,
                draft,
                "decision.rejected",
                0,
            );
            let event_ids = if event_uuid_exists(ledger, event_uuid)? {
                Vec::new()
            } else {
                vec![commands.reject_decision_with_uuid(
                    existing_decision_id,
                    context.importer_actor_id,
                    event_uuid,
                )?]
            };
            conflict_block_report(
                draft,
                existing_decision_id,
                ConflictBlockOutcome::new(
                    DocumentBlockImportStatus::ConflictContested,
                    "contested existing decision with an explicit rejection event",
                ),
                event_ids,
                ConflictBlockReportDetails {
                    reviewer_action: None,
                    similarity_matches,
                    conflict,
                },
            )
        }
        DocumentConflictResolutionAction::AddContext => {
            let event_ids = add_conflict_context_events(
                ledger,
                draft,
                existing_decision_id,
                context.canonical_path,
                context.source_hash,
                context.namespace,
                context.importer_actor_id,
                context.import_run_id,
            )?;
            let status = if draft.evidence.is_empty() && draft.hypotheses.is_empty() {
                DocumentBlockImportStatus::ValidationError
            } else {
                DocumentBlockImportStatus::ConflictContextAdded
            };
            let message = if draft.evidence.is_empty() && draft.hypotheses.is_empty() {
                "add_context requires at least one evidence or hypothesis item"
            } else if event_ids.is_empty() {
                "proposed evidence and hypotheses were already attached"
            } else {
                "added proposed evidence and hypotheses to the existing decision"
            };
            conflict_block_report(
                draft,
                existing_decision_id,
                ConflictBlockOutcome::new(status, message),
                event_ids,
                ConflictBlockReportDetails {
                    reviewer_action: None,
                    similarity_matches,
                    conflict,
                },
            )
        }
    }
}

struct ConflictBlockReportDetails {
    reviewer_action: Option<DocumentReviewerAction>,
    similarity_matches: Vec<DocumentSimilarityMatch>,
    conflict: DocumentImportConflictReport,
}

fn conflict_block_report(
    draft: &DocumentDecisionDraft,
    existing_decision_id: &str,
    outcome: ConflictBlockOutcome<'_>,
    event_ids: Vec<u64>,
    details: ConflictBlockReportDetails,
) -> Result<DocumentBlockImportReport> {
    Ok(DocumentBlockImportReport {
        block_id: draft.block_id.clone(),
        decision_id: Some(existing_decision_id.to_owned()),
        status: outcome.status,
        message: Some(outcome.message.to_owned()),
        reviewer_action: details.reviewer_action,
        similarity_matches: details.similarity_matches,
        source_span: Some(draft.span),
        source_snippet: Some(draft.snippet.clone()),
        event_ids,
        conflict: Some(details.conflict),
    })
}

fn proposed_conflict_update(
    draft: &DocumentDecisionDraft,
    decision_id: &str,
    canonical_path: &str,
    source_hash: &str,
    importer_actor_id: &str,
    import_run_id: &str,
) -> DocumentConflictProposedUpdate {
    DocumentConflictProposedUpdate {
        decision_id: decision_id.to_owned(),
        title: draft.title.clone(),
        status: draft.status,
        topic_keys: draft.topic_keys.clone(),
        rationale: draft.rationale.clone(),
        option_labels: draft.option_labels.clone(),
        evidence: draft.evidence.clone(),
        hypotheses: draft.hypotheses.clone(),
        supersedes: draft.supersedes.clone(),
        source: DocumentConflictSourceProvenance {
            source: "document".to_owned(),
            path: canonical_path.to_owned(),
            sha256: source_hash.to_owned(),
            import_run_id: import_run_id.to_owned(),
            block_id: draft.block_id.clone(),
            source_span: draft.span,
            source_snippet: draft.snippet.clone(),
            importer_actor_id: importer_actor_id.to_owned(),
            original_actor_id: draft.original_actor_id.clone(),
        },
    }
}

fn existing_document_conflict_item<L: EventLedger>(
    ledger: &L,
    decision_id: &str,
) -> Result<DocumentConflictExistingItem> {
    let mut offset = 0;
    const PAGE_SIZE: usize = 1024;
    let mut item = None;
    let mut accepted = false;
    let mut rejected = false;
    let mut superseded_by = BTreeSet::new();

    loop {
        let events = ledger.read(offset, PAGE_SIZE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            if !matches!(
                event.event_type,
                EventType::DecisionProposed
                    | EventType::DecisionAccepted
                    | EventType::DecisionRejected
                    | EventType::DecisionSuperseded
            ) {
                continue;
            }
            match events::validate(event).map_err(|error| {
                CommandError::Invariant(format!("invalid ledger event during import scan: {error}"))
            })? {
                EventPayload::DecisionProposed(payload)
                    if payload.decision_id == decision_id && item.is_none() =>
                {
                    item = Some(DocumentConflictExistingItem {
                        decision_id: payload.decision_id,
                        title: payload.title,
                        rationale: payload.rationale,
                        topic_keys: payload.topic_keys,
                        status: DocumentConflictDecisionStatus::Proposed,
                        actor_id: event.actor_id.clone(),
                        event_origin: event.event_id.unwrap_or_default(),
                        source: event.source,
                        source_ref: event.source_ref.clone(),
                        import_run_id: import_run_id_from_source_ref(event.source_ref.as_deref()),
                    });
                }
                EventPayload::DecisionAccepted(payload) if payload.decision_id == decision_id => {
                    accepted = true;
                }
                EventPayload::DecisionRejected(payload) if payload.decision_id == decision_id => {
                    rejected = true;
                }
                EventPayload::DecisionSuperseded(payload)
                    if payload.old_decision_id == decision_id =>
                {
                    superseded_by.insert(payload.new_decision_id);
                }
                _ => {}
            }
        }

        if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
            offset = last_event_id;
        } else {
            break;
        }
    }

    let mut item = item.ok_or_else(|| {
        CommandError::Invariant(format!(
            "decision exists check passed but proposal was not found: {decision_id}"
        ))
    })?;
    item.status = conflict_decision_status(accepted, rejected, !superseded_by.is_empty());
    Ok(item)
}

fn affected_dependencies_for_decision<L: EventLedger>(
    ledger: &L,
    decision_id: &str,
) -> Result<DocumentConflictAffectedDependencies> {
    let mut offset = 0;
    const PAGE_SIZE: usize = 1024;
    let mut dependencies = DocumentConflictAffectedDependencies::default();
    let mut option_ids = BTreeSet::new();
    let mut evidence_ids = BTreeSet::new();
    let mut hypothesis_ids = BTreeSet::new();
    let mut supersedes_decision_ids = BTreeSet::new();
    let mut superseded_by_decision_ids = BTreeSet::new();

    loop {
        let events = ledger.read(offset, PAGE_SIZE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            if !matches!(
                event.event_type,
                EventType::DecisionProposed
                    | EventType::RelationAdded
                    | EventType::DecisionSuperseded
            ) {
                continue;
            }
            match events::validate(event).map_err(|error| {
                CommandError::Invariant(format!("invalid ledger event during import scan: {error}"))
            })? {
                EventPayload::DecisionProposed(payload) if payload.decision_id == decision_id => {
                    option_ids.extend(payload.option_ids);
                    evidence_ids.extend(payload.evidence_ids);
                    hypothesis_ids.extend(payload.hypothesis_ids);
                }
                EventPayload::RelationAdded(payload) if payload.from_id == decision_id => {
                    match payload.relation {
                        events::RelationKind::HasOption | events::RelationKind::Chose => {
                            option_ids.insert(payload.to_id);
                        }
                        events::RelationKind::BasedOn => {
                            evidence_ids.insert(payload.to_id);
                        }
                        events::RelationKind::Assumes => {
                            hypothesis_ids.insert(payload.to_id);
                        }
                        events::RelationKind::Supports | events::RelationKind::Refutes => {}
                    }
                }
                EventPayload::DecisionSuperseded(payload)
                    if payload.old_decision_id == decision_id =>
                {
                    superseded_by_decision_ids.insert(payload.new_decision_id);
                }
                EventPayload::DecisionSuperseded(payload)
                    if payload.new_decision_id == decision_id =>
                {
                    supersedes_decision_ids.insert(payload.old_decision_id);
                }
                _ => {}
            }
        }

        if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
            offset = last_event_id;
        } else {
            break;
        }
    }

    dependencies.option_ids = option_ids.into_iter().collect();
    dependencies.evidence_ids = evidence_ids.into_iter().collect();
    dependencies.hypothesis_ids = hypothesis_ids.into_iter().collect();
    dependencies.supersedes_decision_ids = supersedes_decision_ids.into_iter().collect();
    dependencies.superseded_by_decision_ids = superseded_by_decision_ids.into_iter().collect();
    Ok(dependencies)
}

#[allow(clippy::too_many_arguments)]
fn add_conflict_context_events<L: EventLedger>(
    ledger: &L,
    draft: &DocumentDecisionDraft,
    existing_decision_id: &str,
    canonical_path: &str,
    source_hash: &str,
    namespace: &str,
    importer_actor_id: &str,
    import_run_id: &str,
) -> Result<Vec<u64>> {
    if draft.evidence.is_empty() && draft.hypotheses.is_empty() {
        return Ok(Vec::new());
    }

    let source_ref = document_source_ref(
        draft,
        canonical_path,
        source_hash,
        importer_actor_id,
        import_run_id,
        Some(DocumentConflictSourceRefResolution {
            action: DocumentConflictResolutionAction::AddContext,
            existing_decision_id: existing_decision_id.to_owned(),
            resolved_decision_id: None,
        }),
    )?;
    let commands =
        Commands::new_with_provenance(ledger, EventProvenance::document(source_ref.clone()));
    let actor_id = importer_actor_id;
    let block_component = stable_component(&format!(
        "{}-context-{}",
        draft.block_id,
        &source_hash[..12]
    ));
    let mut event_ids = Vec::new();

    for (index, evidence) in draft.evidence.iter().enumerate() {
        let evidence_id = format!(
            "evidence:document:{namespace}:{block_component}:{}-{}",
            index + 1,
            stable_component(evidence)
        );
        let record_uuid = conflict_resolution_uuid(
            DocumentConflictResolutionAction::AddContext,
            canonical_path,
            source_hash,
            draft,
            "evidence.recorded",
            index + 1,
        );
        if !event_uuid_exists(ledger, record_uuid)? && !evidence_id_exists(ledger, &evidence_id)? {
            event_ids.push(commands.record_evidence_with_id(
                actor_id,
                &evidence_id,
                evidence,
                Some(source_ref.as_str()),
                record_uuid,
            )?);
        }
        let relation_uuid = conflict_resolution_uuid(
            DocumentConflictResolutionAction::AddContext,
            canonical_path,
            source_hash,
            draft,
            "relation.based_on",
            index + 1,
        );
        if !event_uuid_exists(ledger, relation_uuid)? {
            event_ids.push(commands.attach_evidence_with_uuid(
                existing_decision_id,
                &evidence_id,
                actor_id,
                relation_uuid,
            )?);
        }
    }

    for (index, hypothesis) in draft.hypotheses.iter().enumerate() {
        let hypothesis_id = format!(
            "hypothesis:document:{namespace}:{block_component}:{}-{}",
            index + 1,
            stable_component(hypothesis)
        );
        let record_uuid = conflict_resolution_uuid(
            DocumentConflictResolutionAction::AddContext,
            canonical_path,
            source_hash,
            draft,
            "hypothesis.recorded",
            index + 1,
        );
        if !event_uuid_exists(ledger, record_uuid)?
            && !hypothesis_id_exists(ledger, &hypothesis_id)?
        {
            event_ids.push(commands.record_hypothesis_with_id(
                actor_id,
                &hypothesis_id,
                hypothesis,
                record_uuid,
            )?);
        }
        let relation_uuid = conflict_resolution_uuid(
            DocumentConflictResolutionAction::AddContext,
            canonical_path,
            source_hash,
            draft,
            "relation.assumes",
            index + 1,
        );
        if !event_uuid_exists(ledger, relation_uuid)? {
            event_ids.push(commands.assume_hypothesis_with_uuid(
                existing_decision_id,
                &hypothesis_id,
                actor_id,
                relation_uuid,
            )?);
        }
    }

    Ok(event_ids)
}

fn conflict_resolution_uuid(
    action: DocumentConflictResolutionAction,
    canonical_path: &str,
    source_hash: &str,
    draft: &DocumentDecisionDraft,
    role: &str,
    index: usize,
) -> Uuid {
    import_uuid(&format!(
        "import:v1:{canonical_path}:{source_hash}:{}:{}-{}:conflict:{}:{role}:{index}",
        draft.block_id,
        draft.span.byte_start,
        draft.span.byte_end,
        action.label()
    ))
}

fn conflict_decision_status(
    accepted: bool,
    rejected: bool,
    superseded: bool,
) -> DocumentConflictDecisionStatus {
    if superseded {
        DocumentConflictDecisionStatus::Superseded
    } else {
        match (accepted, rejected) {
            (true, true) => DocumentConflictDecisionStatus::Contested,
            (true, false) => DocumentConflictDecisionStatus::Accepted,
            (false, true) => DocumentConflictDecisionStatus::Rejected,
            (false, false) => DocumentConflictDecisionStatus::Proposed,
        }
    }
}

fn import_run_id_from_source_ref(source_ref: Option<&str>) -> Option<String> {
    let raw = source_ref?;
    let parsed = serde_json::from_str::<DocumentSourceRef>(raw).ok()?;
    Some(parsed.import_run_id)
}

fn evidence_id_exists<L: EventLedger>(ledger: &L, evidence_id: &str) -> Result<bool> {
    scan_ledger(ledger, |event| {
        event.event_type == EventType::EvidenceRecorded
            && event
                .payload
                .get("evidence_id")
                .and_then(|value| value.as_str())
                == Some(evidence_id)
    })
}

fn hypothesis_id_exists<L: EventLedger>(ledger: &L, hypothesis_id: &str) -> Result<bool> {
    scan_ledger(ledger, |event| {
        event.event_type == EventType::HypothesisRecorded
            && event
                .payload
                .get("hypothesis_id")
                .and_then(|value| value.as_str())
                == Some(hypothesis_id)
    })
}

fn missing_superseded_decision<L: EventLedger>(
    ledger: &L,
    identities: &DocumentImportIdentities,
) -> Result<Option<String>> {
    for superseded_decision_id in &identities.supersedes_decision_ids {
        if !decision_id_exists(ledger, superseded_decision_id)? {
            return Ok(Some(superseded_decision_id.clone()));
        }
    }
    Ok(None)
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
        Self::new_with_identity_block(
            draft,
            canonical_path,
            source_hash,
            namespace,
            &draft.block_id,
            &[],
        )
    }

    fn new_conflict_supersession(
        draft: &DocumentDecisionDraft,
        canonical_path: &str,
        source_hash: &str,
        namespace: &str,
        existing_decision_id: &str,
    ) -> Result<Self> {
        let identity_block_id = format!("{}-supersedes-{}", draft.block_id, &source_hash[..12]);
        Self::new_with_identity_block(
            draft,
            canonical_path,
            source_hash,
            namespace,
            &identity_block_id,
            &[existing_decision_id.to_owned()],
        )
    }

    fn new_with_identity_block(
        draft: &DocumentDecisionDraft,
        canonical_path: &str,
        source_hash: &str,
        namespace: &str,
        identity_block_id: &str,
        extra_supersedes_decision_ids: &[String],
    ) -> Result<Self> {
        let block_component = stable_component(identity_block_id);
        let decision_id = stable_decision_id(namespace, identity_block_id);
        let role_prefix = format!(
            "import:v1:{canonical_path}:{source_hash}:{}:{}-{}",
            identity_block_id, draft.span.byte_start, draft.span.byte_end
        );

        let option_ids = draft
            .option_labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                stable_document_child_id(
                    "option",
                    namespace,
                    &block_component,
                    index + 1,
                    stable_component(label),
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
                Some(option_ids.get(index).cloned().ok_or_else(|| {
                    CommandError::Invariant(
                        "chosen option index must map to a generated option id".to_owned(),
                    )
                })?)
            }
            None => None,
        };
        let evidence_ids = draft
            .evidence
            .iter()
            .enumerate()
            .map(|(index, content)| {
                stable_document_child_id(
                    "evidence",
                    namespace,
                    &block_component,
                    index + 1,
                    stable_component(content),
                )
            })
            .collect::<Vec<_>>();
        let hypothesis_ids = draft
            .hypotheses
            .iter()
            .enumerate()
            .map(|(index, statement)| {
                stable_document_child_id(
                    "hypothesis",
                    namespace,
                    &block_component,
                    index + 1,
                    stable_component(statement),
                )
            })
            .collect::<Vec<_>>();
        let mut supersedes_decision_ids = Vec::new();
        for id in extra_supersedes_decision_ids {
            if !supersedes_decision_ids.contains(id) {
                supersedes_decision_ids.push(id.clone());
            }
        }
        for id in draft
            .supersedes
            .iter()
            .map(|id| stable_decision_reference(namespace, id))
        {
            if !supersedes_decision_ids.contains(&id) {
                supersedes_decision_ids.push(id);
            }
        }
        let supersedes_count = supersedes_decision_ids.len();
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
                supersedes_count,
            ),
        })
    }
}

#[derive(Debug, Clone)]
struct RawDocumentDecisionBlock {
    span: DocumentSourceSpan,
    text: String,
    lines: Vec<SourceLine>,
    prepared_source_ref: Option<DocumentPreparedSourceRef>,
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
        let Some(start_line) = lines.get(start_index) else {
            continue;
        };
        let end_index = starts
            .get(start_position + 1)
            .copied()
            .unwrap_or(lines.len());
        let Some(last_line) = end_index.checked_sub(1).and_then(|index| lines.get(index)) else {
            continue;
        };
        let byte_start = start_line.byte_start;
        let byte_end = lines
            .get(end_index)
            .map(|line| line.byte_start)
            .unwrap_or_else(|| input.len());
        let Some(text) = input.get(byte_start..byte_end) else {
            continue;
        };
        let Some(block_lines) = lines.get(start_index..end_index) else {
            continue;
        };
        blocks.push(RawDocumentDecisionBlock {
            span: DocumentSourceSpan {
                byte_start,
                byte_end,
                line_start: start_line.number,
                line_end: last_line.number,
            },
            text: text.to_owned(),
            lines: block_lines.to_vec(),
            prepared_source_ref: prepared_source_ref_before(&lines, start_index),
        });
    }

    blocks
}

fn prepared_source_ref_before(
    lines: &[SourceLine],
    start_index: usize,
) -> Option<DocumentPreparedSourceRef> {
    lines[..start_index]
        .iter()
        .rev()
        .filter_map(|line| {
            line.text
                .trim()
                .strip_prefix("# hivemind-source-ref:")
                .map(str::trim)
        })
        .find_map(|value| serde_json::from_str::<DocumentPreparedSourceRef>(value).ok())
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
        prepared_source_ref: raw.prepared_source_ref.clone(),
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
            DocumentBlockImportStatus::ConflictKeptExisting
            | DocumentBlockImportStatus::ConflictSuperseded
            | DocumentBlockImportStatus::ConflictContested
            | DocumentBlockImportStatus::ConflictContextAdded => summary.blocks_resolved += 1,
            DocumentBlockImportStatus::DuplicateCandidate => summary.duplicate_candidates += 1,
            DocumentBlockImportStatus::ValidationError => summary.validation_errors += 1,
        }
    }
}

fn accumulate_preparation_summary(
    summary: &mut DocumentPreparationSummary,
    file: &DocumentPreparedFileReport,
) {
    summary.pages_seen += file.pages.len();
    if file.prepared_path.is_some() {
        if let Some(text) = file.intermediate_text.as_deref() {
            summary.bytes_written += text.len();
        }
    }
    match file.status {
        DocumentPreparationFileStatus::Prepared => summary.files_prepared += 1,
        DocumentPreparationFileStatus::ReviewRequired => {
            summary.files_prepared += 1;
            summary.files_review_required += 1;
        }
        DocumentPreparationFileStatus::NeedsOcr => summary.files_needing_ocr += 1,
        DocumentPreparationFileStatus::SkippedUnsupported => summary.files_skipped += 1,
        DocumentPreparationFileStatus::ValidationError => summary.validation_errors += 1,
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

const DOCUMENT_FUZZY_ALGORITHM: &str = "document_fuzzy_v1";
const DOCUMENT_FUZZY_MATCH_THRESHOLD: u32 = 70;
const DOCUMENT_FUZZY_STABLE_BLOCK_ID_SCORE: u32 = 75;
const DOCUMENT_FUZZY_MAX_MATCHES: usize = 5;
const DOCUMENT_FUZZY_STOP_WORDS: &[&str] = &[
    "a", "an", "and", "as", "be", "by", "for", "from", "in", "is", "it", "of", "on", "or", "our",
    "the", "to", "use", "using", "with",
];

fn find_document_similarity_matches<L: EventLedger>(
    ledger: &L,
    draft: &DocumentDecisionDraft,
    decision_id: &str,
) -> Result<Vec<DocumentSimilarityMatch>> {
    let mut offset = 0;
    let mut matches = Vec::new();
    const PAGE_SIZE: usize = 1024;

    loop {
        let events = ledger.read(offset, PAGE_SIZE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            if event.event_type != EventType::DecisionProposed
                || event.source != EventSource::Document
            {
                continue;
            }
            let Some(source_ref_raw) = event.source_ref.as_deref() else {
                continue;
            };
            let Ok(source_ref) = serde_json::from_str::<DocumentSourceRef>(source_ref_raw) else {
                continue;
            };
            if let Some(similarity_match) =
                document_similarity_match(draft, decision_id, event, &source_ref, source_ref_raw)
            {
                matches.push(similarity_match);
            }
        }

        if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
            offset = last_event_id;
        } else {
            break;
        }
    }

    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.event_origin.cmp(&right.event_origin))
            .then_with(|| left.decision_id.cmp(&right.decision_id))
    });
    matches.truncate(DOCUMENT_FUZZY_MAX_MATCHES);
    Ok(matches)
}

fn document_similarity_match(
    draft: &DocumentDecisionDraft,
    decision_id: &str,
    event: &Event,
    source_ref: &DocumentSourceRef,
    source_ref_raw: &str,
) -> Option<DocumentSimilarityMatch> {
    let existing_decision_id = event.payload.get("decision_id")?.as_str()?.to_owned();
    let existing_title = event.payload.get("title")?.as_str()?;
    let existing_rationale = event.payload.get("rationale")?.as_str()?;
    let existing_topic_keys = payload_string_list(&event.payload, "topic_keys");

    let title_overlap = token_overlap_percent(
        &comparable_tokens(&draft.title),
        &comparable_tokens(existing_title),
    );
    let rationale_overlap = token_overlap_percent(
        &comparable_tokens(&draft.rationale),
        &comparable_tokens(existing_rationale),
    );
    let topic_overlap = token_overlap_percent(
        &comparable_topic_keys(&draft.topic_keys),
        &comparable_topic_keys(&existing_topic_keys),
    );
    let same_stable_block_id = stable_component(&source_ref.block_id)
        == stable_component(&draft.block_id)
        || existing_decision_id == decision_id;

    let mut score = ((title_overlap * 45) + (rationale_overlap * 35) + (topic_overlap * 20)) / 100;
    if same_stable_block_id {
        score = score.max(DOCUMENT_FUZZY_STABLE_BLOCK_ID_SCORE);
    }
    if score < DOCUMENT_FUZZY_MATCH_THRESHOLD {
        return None;
    }

    let mut matched_fields = Vec::new();
    if same_stable_block_id {
        matched_fields.push("block_id");
    }
    if title_overlap >= 50 {
        matched_fields.push("title");
    }
    if rationale_overlap >= 50 {
        matched_fields.push("rationale");
    }
    if topic_overlap >= 50 {
        matched_fields.push("topic_keys");
    }
    if matched_fields.is_empty() {
        matched_fields.push("weighted_score");
    }

    Some(DocumentSimilarityMatch {
        decision_id: existing_decision_id,
        event_origin: event.event_id,
        score,
        review_required: true,
        basis: DocumentSimilarityBasis {
            algorithm: DOCUMENT_FUZZY_ALGORITHM,
            title_token_overlap: title_overlap,
            rationale_token_overlap: rationale_overlap,
            topic_key_overlap: topic_overlap,
            same_stable_block_id,
            matched_fields,
            source_path: source_ref.path.clone(),
            source_block_id: source_ref.block_id.clone(),
            source_hash: source_ref.sha256.clone(),
            source_ref: Some(source_ref_raw.to_owned()),
        },
    })
}

fn payload_string_list(payload: &serde_json::Value, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect()
}

fn comparable_tokens(input: &str) -> BTreeSet<String> {
    let transliterated = deunicode::deunicode(input);
    let mut tokens = BTreeSet::new();
    let mut current = String::new();

    for character in transliterated.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            current.push(character);
            continue;
        }
        push_similarity_token(&mut tokens, &mut current);
    }
    push_similarity_token(&mut tokens, &mut current);

    tokens
}

fn comparable_topic_keys(keys: &[String]) -> BTreeSet<String> {
    keys.iter()
        .map(|key| stable_component(key))
        .filter(|key| !key.is_empty())
        .collect()
}

fn push_similarity_token(tokens: &mut BTreeSet<String>, current: &mut String) {
    if current.len() > 1 && !DOCUMENT_FUZZY_STOP_WORDS.contains(&current.as_str()) {
        tokens.insert(current.clone());
    }
    current.clear();
}

fn token_overlap_percent(left: &BTreeSet<String>, right: &BTreeSet<String>) -> u32 {
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    let intersection = left.intersection(right).count() as u32;
    ((intersection * 200) / (left.len() as u32 + right.len() as u32)).min(100)
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
    format!("{stem}-{}", short_sha256_hex(canonical_path.as_bytes()))
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
        short_sha256_hex(input.as_bytes())
    } else {
        normalized
    }
}

fn stable_document_child_id(
    kind: &str,
    namespace: &str,
    block_component: &str,
    index: usize,
    component: String,
) -> String {
    let mut id = String::with_capacity(
        kind.len() + namespace.len() + block_component.len() + component.len() + 24,
    );
    let _ = write!(
        id,
        "{kind}:document:{namespace}:{block_component}:{index}-{component}"
    );
    id
}

fn repeated_role_uuids(role_prefix: &str, role: &str, count: usize) -> Vec<Uuid> {
    let mut uuids = Vec::with_capacity(count);
    for index in 0..count {
        let mut key = String::with_capacity(role_prefix.len() + role.len() + 16);
        let _ = write!(key, "{role_prefix}:{role}:{}", index + 1);
        uuids.push(import_uuid(&key));
    }
    uuids
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
        short_sha256_hex(seed.as_bytes())
    )
}

fn short_sha256_hex(bytes: &[u8]) -> String {
    sha256_hex(bytes).chars().take(12).collect()
}

fn preparation_run_id(files: &[PathBuf]) -> String {
    let seed = files
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "prepare:{}:{}",
        Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        short_sha256_hex(seed.as_bytes())
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests;
