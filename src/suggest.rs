use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CliError, CommandError};
use crate::ingest::{collect_document_import_paths, DocumentImportFormat, DocumentSourceSpan};
use crate::Result;

const DOCUMENT_CANDIDATE_PROTOCOL: &str = "hivemind.document_extraction_candidates.v1";
const DOCUMENT_EXTRACTOR_EXECUTABLE: &str = "hivemind-document-extractor";

#[derive(Debug, Clone)]
pub struct DocumentCandidateRequest {
    pub paths: Vec<PathBuf>,
    pub format: DocumentImportFormat,
    pub extractor: DocumentCandidateExtractor,
}

#[derive(Debug, Clone)]
pub enum DocumentCandidateExtractor {
    Command { args: Vec<String> },
    ResponseFile(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionCandidateReport {
    pub workflow: String,
    pub candidate_run_id: String,
    pub review_required: bool,
    pub summary: DocumentExtractionCandidateSummary,
    pub files: Vec<DocumentExtractionSourceDocument>,
    pub candidates: Vec<DocumentExtractionCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionCandidateSummary {
    pub files_seen: usize,
    pub candidates_proposed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionSourceDocument {
    pub file_index: usize,
    pub path: String,
    pub canonical_path: String,
    pub sha256: String,
    pub full_span: DocumentSourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionCandidate {
    pub candidate_id: String,
    pub review_status: String,
    pub decision: DocumentExtractionCandidateDecision,
    pub source: DocumentExtractionCandidateSource,
    pub explanation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_explanation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionCandidateDecision {
    pub id: String,
    pub title: String,
    pub status: String,
    pub topic_keys: Vec<String>,
    pub rationale: String,
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chose: Option<String>,
    pub evidence: Vec<String>,
    pub hypotheses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractionCandidateSource {
    pub path: String,
    pub sha256: String,
    pub span: DocumentSourceSpan,
    pub snippet: String,
}

#[derive(Debug, Serialize)]
struct ExtractorPrompt {
    protocol: &'static str,
    instructions: &'static str,
    files: Vec<ExtractorPromptFile>,
    expected_response: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ExtractorPromptFile {
    file_index: usize,
    path: String,
    canonical_path: String,
    sha256: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct RawExtractorResponse {
    candidates: Vec<RawExtractionCandidate>,
}

#[derive(Debug, Deserialize)]
struct RawExtractionCandidate {
    file_index: Option<usize>,
    source_path: Option<String>,
    source_span: Option<DocumentSourceSpan>,
    title: String,
    #[serde(default = "default_candidate_status")]
    status: String,
    #[serde(default, alias = "topics")]
    topic_keys: Vec<String>,
    rationale: String,
    #[serde(default, alias = "options")]
    option_labels: Vec<String>,
    #[serde(default, alias = "chosen", alias = "chosen_option", alias = "chose")]
    chosen_option_label: Option<String>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default, alias = "hypothesis")]
    hypotheses: Vec<String>,
    explanation: String,
    confidence_explanation: Option<String>,
}

#[derive(Debug, Clone)]
struct SourceDocument {
    file_index: usize,
    path: String,
    canonical_path: String,
    sha256: String,
    text: String,
    full_span: DocumentSourceSpan,
}

pub fn propose_document_extraction_candidates(
    request: &DocumentCandidateRequest,
) -> Result<DocumentExtractionCandidateReport> {
    let source_documents = load_source_documents(&request.paths, request.format)?;
    let prompt = extractor_prompt(&source_documents);
    let response = read_extractor_response(&request.extractor, &prompt)?;
    let raw_response =
        serde_json::from_str::<RawExtractorResponse>(&response).map_err(|error| {
            CliError::InvalidInput(format!(
                "invalid document extraction candidate response: {error}"
            ))
        })?;

    let public_files = source_documents
        .iter()
        .map(|document| DocumentExtractionSourceDocument {
            file_index: document.file_index,
            path: document.path.clone(),
            canonical_path: document.canonical_path.clone(),
            sha256: document.sha256.clone(),
            full_span: document.full_span,
        })
        .collect::<Vec<_>>();

    let path_index = path_index(&source_documents);
    let mut candidates = Vec::with_capacity(raw_response.candidates.len());
    for (ordinal, raw_candidate) in raw_response.candidates.into_iter().enumerate() {
        let document = resolve_candidate_document(&source_documents, &path_index, &raw_candidate)?;
        let span = raw_candidate.source_span.unwrap_or(document.full_span);
        validate_span(document, span)?;
        let snippet = compact_snippet(&document.text[span.byte_start..span.byte_end]);
        let candidate = normalize_candidate(raw_candidate, document, span, snippet, ordinal + 1)?;
        candidates.push(candidate);
    }

    Ok(DocumentExtractionCandidateReport {
        workflow: DOCUMENT_CANDIDATE_PROTOCOL.to_owned(),
        candidate_run_id: candidate_run_id(&source_documents),
        review_required: true,
        summary: DocumentExtractionCandidateSummary {
            files_seen: source_documents.len(),
            candidates_proposed: candidates.len(),
        },
        files: public_files,
        candidates,
    })
}

fn load_source_documents(
    paths: &[PathBuf],
    format: DocumentImportFormat,
) -> Result<Vec<SourceDocument>> {
    if paths.is_empty() {
        return Err(CliError::InvalidInput(
            "document-candidates requires at least one --file or path".to_owned(),
        )
        .into());
    }
    let files = collect_document_import_paths(paths, format)?;
    let mut documents = Vec::with_capacity(files.len());
    for (file_index, path) in files.into_iter().enumerate() {
        let bytes = fs::read(&path).map_err(|error| {
            CliError::InvalidInput(format!("cannot read document {}: {error}", path.display()))
        })?;
        let sha256 = sha256_hex(&bytes);
        let text = String::from_utf8(bytes).map_err(|error| {
            CliError::InvalidInput(format!(
                "document {} is not valid UTF-8: {error}",
                path.display()
            ))
        })?;
        let canonical_path = fs::canonicalize(&path).map_err(|error| {
            CliError::InvalidInput(format!(
                "cannot canonicalize document path {}: {error}",
                path.display()
            ))
        })?;
        let full_span = full_document_span(&text);
        documents.push(SourceDocument {
            file_index,
            path: path.display().to_string(),
            canonical_path: canonical_path.display().to_string(),
            sha256,
            text,
            full_span,
        });
    }
    Ok(documents)
}

fn extractor_prompt(documents: &[SourceDocument]) -> ExtractorPrompt {
    ExtractorPrompt {
        protocol: DOCUMENT_CANDIDATE_PROTOCOL,
        instructions: "Extract only defensible organizational decision-memory candidates. Return candidate blocks for decisions with rationale, options, evidence, hypotheses, source spans, snippets, and a short explanation. Do not claim that events were written.",
        files: documents
            .iter()
            .map(|document| ExtractorPromptFile {
                file_index: document.file_index,
                path: document.path.clone(),
                canonical_path: document.canonical_path.clone(),
                sha256: document.sha256.clone(),
                text: document.text.clone(),
            })
            .collect(),
        expected_response: serde_json::json!({
            "candidates": [{
                "file_index": 0,
                "source_span": {
                    "byte_start": 0,
                    "byte_end": 0,
                    "line_start": 1,
                    "line_end": 1
                },
                "title": "Decision title",
                "status": "proposed",
                "topic_keys": ["topic"],
                "rationale": "Why this decision was made or proposed.",
                "option_labels": ["selected option", "alternative option"],
                "chosen_option_label": "selected option",
                "evidence": ["source-backed evidence"],
                "hypotheses": ["assumption to track"],
                "explanation": "Why the extractor believes this is a decision candidate.",
                "confidence_explanation": "Optional non-numeric basis for confidence."
            }]
        }),
    }
}

fn read_extractor_response(
    extractor: &DocumentCandidateExtractor,
    prompt: &ExtractorPrompt,
) -> Result<String> {
    match extractor {
        DocumentCandidateExtractor::ResponseFile(path) => {
            fs::read_to_string(path).map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot read LLM response file {}: {error}",
                    path.display()
                ))
                .into()
            })
        }
        DocumentCandidateExtractor::Command { args } => {
            let prompt_json = serde_json::to_vec(prompt).map_err(|error| {
                CliError::InvalidInput(format!("extractor prompt serialization failed: {error}"))
            })?;
            let mut child = Command::new(DOCUMENT_EXTRACTOR_EXECUTABLE)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    CliError::InvalidInput(format!(
                        "cannot run extractor command {DOCUMENT_EXTRACTOR_EXECUTABLE}: {error}"
                    ))
                })?;
            let Some(mut stdin) = child.stdin.take() else {
                return Err(CliError::InvalidInput(
                    "extractor command did not expose stdin".to_owned(),
                )
                .into());
            };
            stdin.write_all(&prompt_json).map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot write prompt to extractor command {DOCUMENT_EXTRACTOR_EXECUTABLE}: {error}"
                ))
            })?;
            drop(stdin);
            let output = child.wait_with_output().map_err(|error| {
                CliError::InvalidInput(format!(
                    "extractor command {DOCUMENT_EXTRACTOR_EXECUTABLE} failed to finish: {error}"
                ))
            })?;
            if !output.status.success() {
                return Err(CliError::InvalidInput(format!(
                    "extractor command {DOCUMENT_EXTRACTOR_EXECUTABLE} exited with status {}; stderr: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
                .into());
            }
            String::from_utf8(output.stdout).map_err(|error| {
                CliError::InvalidInput(format!(
                    "extractor command {DOCUMENT_EXTRACTOR_EXECUTABLE} did not return UTF-8 JSON: {error}"
                ))
                .into()
            })
        }
    }
}

fn path_index(documents: &[SourceDocument]) -> HashMap<String, usize> {
    let mut index = HashMap::new();
    for document in documents {
        index.insert(document.path.clone(), document.file_index);
        index.insert(document.canonical_path.clone(), document.file_index);
    }
    index
}

fn resolve_candidate_document<'a>(
    documents: &'a [SourceDocument],
    path_index: &HashMap<String, usize>,
    candidate: &RawExtractionCandidate,
) -> Result<&'a SourceDocument> {
    if let Some(file_index) = candidate.file_index {
        return documents.get(file_index).ok_or_else(|| {
            CliError::InvalidInput(format!(
                "candidate references file_index {file_index}, but only {} files were provided",
                documents.len()
            ))
            .into()
        });
    }

    if let Some(source_path) = candidate.source_path.as_deref() {
        let file_index = path_index.get(source_path).ok_or_else(|| {
            CliError::InvalidInput(format!(
                "candidate references source_path '{source_path}', which is not in the request"
            ))
        })?;
        return documents.get(*file_index).ok_or_else(|| {
            CliError::InvalidInput(format!(
                "candidate source_path '{source_path}' resolved to missing file index {file_index}"
            ))
            .into()
        });
    }

    if documents.len() == 1 {
        return Ok(&documents[0]);
    }

    Err(CliError::InvalidInput(
        "candidate must include file_index or source_path when multiple files are provided"
            .to_owned(),
    )
    .into())
}

fn normalize_candidate(
    raw: RawExtractionCandidate,
    document: &SourceDocument,
    span: DocumentSourceSpan,
    snippet: String,
    ordinal: usize,
) -> Result<DocumentExtractionCandidate> {
    let title = required_trimmed("candidate title", &raw.title)?;
    let rationale = required_trimmed("candidate rationale", &raw.rationale)?;
    let explanation = required_trimmed("candidate explanation", &raw.explanation)?;
    let status = normalize_status(&raw.status)?;
    let options = normalize_list("candidate options", raw.option_labels)?;
    if options.is_empty() {
        return Err(
            CommandError::Validation("candidate options must not be empty".to_owned()).into(),
        );
    }
    let chose = normalize_optional(raw.chosen_option_label);
    if let Some(chosen) = chose.as_deref() {
        if !options.iter().any(|option| option == chosen) {
            return Err(CommandError::Validation(format!(
                "candidate chose value '{chosen}' must match one of options"
            ))
            .into());
        }
    }
    let mut topic_keys = normalize_list("candidate topic_keys", raw.topic_keys)?;
    if topic_keys.is_empty() {
        topic_keys.push("document".to_owned());
    }

    let id_seed = format!(
        "{}:{}:{}:{}:{ordinal}",
        document.sha256, span.byte_start, span.byte_end, title
    );
    let short_id = &sha256_hex(id_seed.as_bytes())[..12];
    let candidate_id = format!("candidate:document:{short_id}");
    let decision_id = format!("llm-candidate-{short_id}");

    Ok(DocumentExtractionCandidate {
        candidate_id: candidate_id.clone(),
        review_status: "pending_review".to_owned(),
        decision: DocumentExtractionCandidateDecision {
            id: decision_id,
            title,
            status,
            topic_keys,
            rationale,
            options,
            chose,
            evidence: normalize_list("candidate evidence", raw.evidence)?,
            hypotheses: normalize_list("candidate hypotheses", raw.hypotheses)?,
        },
        source: DocumentExtractionCandidateSource {
            path: document.canonical_path.clone(),
            sha256: document.sha256.clone(),
            span,
            snippet,
        },
        explanation,
        confidence_explanation: normalize_optional(raw.confidence_explanation),
    })
}

fn validate_span(document: &SourceDocument, span: DocumentSourceSpan) -> Result<()> {
    if span.byte_start > span.byte_end || span.byte_end > document.text.len() {
        return Err(CliError::InvalidInput(format!(
            "candidate span {}-{} is outside document {} ({} bytes)",
            span.byte_start,
            span.byte_end,
            document.canonical_path,
            document.text.len()
        ))
        .into());
    }
    if !document.text.is_char_boundary(span.byte_start)
        || !document.text.is_char_boundary(span.byte_end)
    {
        return Err(CliError::InvalidInput(format!(
            "candidate span {}-{} is not on UTF-8 character boundaries in {}",
            span.byte_start, span.byte_end, document.canonical_path
        ))
        .into());
    }
    if span.line_start == 0 || span.line_end < span.line_start {
        return Err(CliError::InvalidInput(format!(
            "candidate line span {}-{} is invalid for {}",
            span.line_start, span.line_end, document.canonical_path
        ))
        .into());
    }
    Ok(())
}

fn full_document_span(text: &str) -> DocumentSourceSpan {
    DocumentSourceSpan {
        byte_start: 0,
        byte_end: text.len(),
        line_start: 1,
        line_end: text.lines().count().max(1),
    }
}

fn candidate_run_id(documents: &[SourceDocument]) -> String {
    let mut seed = String::new();
    for document in documents {
        seed.push_str(&document.canonical_path);
        seed.push('\n');
        seed.push_str(&document.sha256);
        seed.push('\n');
    }
    format!("candidate-run:{}", &sha256_hex(seed.as_bytes())[..12])
}

fn normalize_status(value: &str) -> Result<String> {
    let status = required_trimmed("candidate status", value)?.to_ascii_lowercase();
    match status.as_str() {
        "proposed" | "accepted" | "rejected" => Ok(status),
        other => Err(CommandError::Validation(format!(
            "unsupported candidate status '{other}'; expected proposed, accepted, or rejected"
        ))
        .into()),
    }
}

fn normalize_list(field: &'static str, values: Vec<String>) -> Result<Vec<String>> {
    values
        .into_iter()
        .map(|value| required_trimmed(field, &value))
        .collect()
}

fn required_trimmed(field: &'static str, value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(one_line(value))
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| one_line(&value))
        .filter(|value| !value.trim().is_empty())
}

fn one_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_snippet(input: &str) -> String {
    let mut snippet = String::new();
    let mut char_count = 0;
    for token in input.split_whitespace() {
        if char_count >= 240 {
            break;
        }
        if !snippet.is_empty() {
            snippet.push(' ');
            char_count += 1;
        }
        for character in token.chars() {
            if char_count >= 240 {
                break;
            }
            snippet.push(character);
            char_count += 1;
        }
    }
    snippet
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn default_candidate_status() -> String {
    "proposed".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn response_file_candidates_carry_source_provenance_without_ledger_access() {
        let root = std::env::temp_dir().join(format!("hivemind-suggest-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("scratch dir");
        let document = root.join("memo.txt");
        fs::write(
            &document,
            "Architecture memo\nDecision: keep ingestion reviewed.\nBecause automatic writes would bypass provenance.\n",
        )
        .expect("write document");
        let response = root.join("response.json");
        fs::write(
            &response,
            serde_json::json!({
                "candidates": [{
                    "file_index": 0,
                    "source_span": {
                        "byte_start": 18,
                        "byte_end": 92,
                        "line_start": 2,
                        "line_end": 3
                    },
                    "title": "Keep ingestion reviewed",
                    "topic_keys": ["documents", "ingestion"],
                    "rationale": "Automatic writes would bypass provenance.",
                    "option_labels": ["review candidates", "auto write"],
                    "chosen_option_label": "review candidates",
                    "evidence": ["The memo says automatic writes would bypass provenance."],
                    "hypotheses": ["Review keeps provenance defensible."],
                    "explanation": "The lines contain a decision and rationale."
                }]
            })
            .to_string(),
        )
        .expect("write response");

        let report = propose_document_extraction_candidates(&DocumentCandidateRequest {
            paths: vec![document.clone()],
            format: DocumentImportFormat::Text,
            extractor: DocumentCandidateExtractor::ResponseFile(response),
        })
        .expect("candidate report");

        assert_eq!(report.summary.files_seen, 1);
        assert_eq!(report.summary.candidates_proposed, 1);
        assert!(report.review_required);
        assert_eq!(report.candidates[0].source.span.line_start, 2);
        assert!(report.candidates[0]
            .source
            .snippet
            .contains("keep ingestion reviewed"));
        assert!(report.candidates[0].source.path.ends_with("memo.txt"));

        let _ = fs::remove_dir_all(root);
    }
}
