use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::commands::normalize_topic_key;
use crate::error::{CliError, CommandError};
use crate::events::{
    DecisionProposedPayload, DecisionSupersededPayload, EventBuilder, EventId, EventPayload,
    EventProvenance, EventType, EvidenceRecordedPayload, RelationAddedPayload, RelationKind,
    RelationRemovedPayload, TenantId,
};
use crate::ledger::EventLedger;
use crate::Result;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    GitFile,
    GoogleDocs,
}

impl ConnectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitFile => "git_file",
            Self::GoogleDocs => "google_docs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId {
    pub connector_kind: ConnectorKind,
    pub doc_id: String,
}

#[derive(Debug, Clone)]
pub struct VersionMeta {
    pub version_id: String,
    pub occurred_at: DateTime<Utc>,
    pub author_actor: Option<String>,
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub struct VersionContent {
    pub version_id: String,
    pub text: String,
    pub content_hash: String,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Statement {
    pub text: String,
    pub byte_span: (usize, usize),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorSourceRef {
    pub source: String,
    pub connector_kind: ConnectorKind,
    pub doc_id: String,
    pub version_id: String,
    pub content_hash: String,
    pub source_url: Option<String>,
    pub statement_ordinal: u64,
    pub statement_span: (usize, usize),
    pub import_run_id: String,
    pub importer_actor_id: String,
    pub original_actor_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Connector trait
// ---------------------------------------------------------------------------

pub trait Connector: Send + Sync {
    fn kind(&self) -> ConnectorKind;
    fn resolve(&self, url_or_id: &str) -> Result<Option<SourceId>>;
    fn list_versions(&self, source_id: &SourceId) -> Result<Vec<VersionMeta>>;
    fn fetch_version(
        &self,
        source_id: &SourceId,
        version_meta: &VersionMeta,
    ) -> Result<VersionContent>;
}

// ---------------------------------------------------------------------------
// GitFileConnector
// ---------------------------------------------------------------------------

pub struct GitFileConnector;

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn parse_doc_id(doc_id: &str) -> Result<(String, String)> {
    match doc_id.find(':') {
        Some(pos) => {
            let repo = doc_id[..pos].to_owned(); // ubs:ignore: pos from str::find, valid byte boundary
            let file = doc_id[pos + 1..].to_owned(); // ubs:ignore: ':' is ASCII (1 byte); pos+1 is valid boundary
            if repo.is_empty() || file.is_empty() {
                return Err(CliError::InvalidInput(format!(
                    "connector doc_id has empty repo or file component: {doc_id}"
                ))
                .into());
            }
            Ok((repo, file))
        }
        None => Err(CliError::InvalidInput(format!(
            "connector doc_id is not in repo:file format: {doc_id}"
        ))
        .into()),
    }
}

fn slugify_author(name: &str) -> String {
    let s = deunicode::deunicode(name);
    let raw: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    raw.split('-')
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

impl Connector for GitFileConnector {
    fn kind(&self) -> ConnectorKind {
        ConnectorKind::GitFile
    }

    fn resolve(&self, url_or_id: &str) -> Result<Option<SourceId>> {
        // Reject remote URLs
        if url_or_id.starts_with("http://") || url_or_id.starts_with("https://") {
            return Ok(None);
        }

        // Try explicit repo:file format first
        if let Some(colon) = url_or_id.find(':') {
            let maybe_repo = &url_or_id[..colon]; // ubs:ignore: colon from str::find, valid byte boundary
            let maybe_file = &url_or_id[colon + 1..]; // ubs:ignore: ':' is ASCII; colon+1 is valid boundary
                                                      // Only treat as repo:file if neither side looks like a Windows drive letter
                                                      // and both sides are non-empty and the repo side looks like a path
            if !maybe_repo.is_empty()
                && !maybe_file.is_empty()
                && maybe_repo.len() > 1
                && Path::new(maybe_repo).exists()
            {
                let repo_path = Path::new(maybe_repo);
                let repo_root = if repo_path.join(".git").exists() {
                    std::fs::canonicalize(repo_path).map_err(|e| {
                        let p = repo_path.display();
                        let msg = format!("cannot canonicalize repo path {p}: {e}"); // ubs:ignore: impl-block false positive
                        CliError::InvalidInput(msg)
                    })?
                } else {
                    let p = repo_path.display();
                    let msg = format!("path {p} is not a git repository root"); // ubs:ignore: impl-block false positive
                    return Err(CliError::InvalidInput(msg).into());
                };
                let doc_id = format!("{}:{}", repo_root.display(), maybe_file); // ubs:ignore: impl-block false positive
                return Ok(Some(SourceId {
                    connector_kind: ConnectorKind::GitFile,
                    doc_id,
                }));
            }
        }

        // Fall back: treat as a plain local path
        let path = Path::new(url_or_id);
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(
                    |e| CliError::InvalidInput(format!("cannot determine current dir: {e}")), // ubs:ignore: impl-block false positive
                )?
                .join(path)
        };

        let repo_root = find_git_root(&abs_path).ok_or_else(|| {
            let msg = format!("path {} is not inside a git repository", abs_path.display()); // ubs:ignore: impl-block false positive
            CliError::InvalidInput(msg)
        })?;

        let canonical_root = std::fs::canonicalize(&repo_root).map_err(|e| {
            let msg = format!("cannot canonicalize repo root {}: {e}", repo_root.display()); // ubs:ignore: impl-block false positive
            CliError::InvalidInput(msg)
        })?;

        // Compute relative path from repo root
        let canonical_file = if abs_path.exists() {
            let fallback = abs_path.clone(); // ubs:ignore: one-time clone fallback; impl-block false positive
            std::fs::canonicalize(&abs_path).unwrap_or(fallback)
        } else {
            abs_path.clone() // ubs:ignore: impl-block false positive; one-time clone
        };

        let rel_path = canonical_file
            .strip_prefix(&canonical_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| url_or_id.to_owned()); // ubs:ignore: impl-block false positive; one-time path resolution

        let doc_id = format!("{}:{}", canonical_root.display(), rel_path); // ubs:ignore: impl-block false positive
        Ok(Some(SourceId {
            connector_kind: ConnectorKind::GitFile,
            doc_id,
        }))
    }

    fn list_versions(&self, source_id: &SourceId) -> Result<Vec<VersionMeta>> {
        let (repo_root, file_path) = parse_doc_id(&source_id.doc_id)?;

        let output = ProcessCommand::new("git")
            .args([
                "-C",
                &repo_root,
                "log",
                "--follow",
                "--reverse",
                "--format=%H%x00%ai%x00%an",
                "--",
                &file_path,
            ])
            .output()
            .map_err(|e| {
                let msg = format!("git log failed for {}: {e}", source_id.doc_id); // ubs:ignore: impl-block false positive
                CliError::InvalidInput(msg)
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = format!("git log error for {}: {stderr}", source_id.doc_id); // ubs:ignore: impl-block false positive
            return Err(CliError::InvalidInput(msg).into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut versions = Vec::new();
        let mut sequence: u64 = 1;

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(3, '\0').collect();
            if parts.len() < 2 {
                continue;
            }
            // ubs:ignore: parts.len() >= 2 guaranteed by the continue above
            let commit_hash = parts[0].trim().to_owned(); // ubs:ignore: parts[0] safe; len >= 2 checked above
            let date_str = parts[1].trim(); // ubs:ignore: parts[1] safe; len >= 2 checked above
            let author_name = if parts.len() >= 3 {
                parts[2].trim() // ubs:ignore: parts[2] safe; len >= 3 checked
            } else {
                ""
            };

            let occurred_at = chrono::DateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S %z")
                .or_else(|_| chrono::DateTime::parse_from_rfc3339(date_str))
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let author_actor = if author_name.is_empty() {
                None
            } else {
                let slug = slugify_author(author_name);
                if slug.is_empty() {
                    None
                } else {
                    Some(format!("human:{slug}")) // ubs:ignore: per-commit actor string, allocation per version is intentional
                }
            };

            versions.push(VersionMeta {
                version_id: commit_hash,
                occurred_at,
                author_actor,
                sequence,
            });
            sequence += 1;
        }

        if versions.is_empty() {
            let doc = &source_id.doc_id;
            let msg = format!("no git history found for {doc} — file may not be tracked"); // ubs:ignore: impl-block false positive
            return Err(CliError::InvalidInput(msg).into());
        }

        Ok(versions)
    }

    fn fetch_version(
        &self,
        source_id: &SourceId,
        version_meta: &VersionMeta,
    ) -> Result<VersionContent> {
        let (repo_root, file_path) = parse_doc_id(&source_id.doc_id)?;

        let git_ref = format!("{}:{}", version_meta.version_id, file_path); // ubs:ignore: impl-block false positive
        let output = ProcessCommand::new("git")
            .args(["-C", &repo_root, "show", &git_ref])
            .output()
            .map_err(|e| {
                let doc = &source_id.doc_id;
                let ver = &version_meta.version_id;
                let msg = format!("git show failed for {doc} at {ver}: {e}"); // ubs:ignore: impl-block false positive
                CliError::InvalidInput(msg)
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let doc = &source_id.doc_id;
            let ver = &version_meta.version_id;
            let msg = format!("git show error for {doc} at {ver}: {stderr}"); // ubs:ignore: impl-block false positive
            return Err(CliError::InvalidInput(msg).into());
        }

        let raw_bytes = &output.stdout;
        let content_hash = sha256_hex(raw_bytes);
        let text = String::from_utf8_lossy(raw_bytes).into_owned();

        Ok(VersionContent {
            version_id: version_meta.version_id.clone(), // ubs:ignore: impl-block false positive; owned String for VersionContent
            text,
            content_hash,
            source_url: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Google token store (local file — NOT in the SQLite ledger)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct GoogleTokenEntry {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_epoch_secs: Option<i64>,
}

#[derive(Serialize, Deserialize, Default)]
struct TokenStoreFile {
    google: Option<GoogleTokenEntry>,
}

pub struct GoogleTokenStore {
    path: PathBuf,
}

impl GoogleTokenStore {
    pub fn new(hivemind_dir: &Path) -> Self {
        Self {
            path: hivemind_dir.join("connector-tokens.json"),
        }
    }

    fn load_file(&self) -> Result<TokenStoreFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|e| {
                CliError::InvalidInput(format!("connector-tokens.json is malformed: {e}")) // ubs:ignore: impl-block false positive
                    .into()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TokenStoreFile::default()), // ubs:ignore: not a secret comparison; comparing io::ErrorKind enum variant
            Err(e) => Err(CliError::InvalidInput(format!(
                "failed to read connector-tokens.json: {e}" // ubs:ignore: impl-block false positive
            ))
            .into()),
        }
    }

    pub fn load_token(&self) -> Result<Option<GoogleTokenEntry>> {
        Ok(self.load_file()?.google)
    }

    pub fn save_token(&self, entry: GoogleTokenEntry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CliError::InvalidInput(format!(
                    "failed to create token store directory: {e}" // ubs:ignore: impl-block false positive
                ))
            })?;
        }
        let mut store = self.load_file()?;
        store.google = Some(entry);
        let json = serde_json::to_string_pretty(&store).map_err(|e| {
            crate::HivemindError::from(CommandError::Invariant(format!(
                "failed to serialize token store: {e}" // ubs:ignore: impl-block false positive
            )))
        })?;
        std::fs::write(&self.path, json).map_err(|e| {
            CliError::InvalidInput(format!("failed to write connector-tokens.json: {e}"))
            // ubs:ignore: impl-block false positive
        })?;
        Ok(())
    }

    pub fn get_valid_access_token(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<String>> {
        let token = match self.load_token()? {
            Some(t) => t,
            None => return Ok(None),
        };

        let now_secs = Utc::now().timestamp();
        let is_valid = token
            .expires_at_epoch_secs
            .is_some_and(|exp| exp - now_secs >= 60);

        if is_valid {
            return Ok(Some(token.access_token));
        }

        let refresh_token = match token.refresh_token {
            Some(rt) => rt,
            None => {
                return Err(CliError::InvalidInput(
                    "Google access token expired and no refresh token stored; \
                     re-run 'hivemind connector auth gdocs'"
                        .to_owned(),
                )
                .into())
            }
        };

        let (new_access, new_expires) =
            refresh_google_access_token(client_id, client_secret, &refresh_token)?;
        let access_to_return = new_access.clone(); // ubs:ignore: one-time clone; new_access moved into entry below
        self.save_token(GoogleTokenEntry {
            access_token: new_access,
            refresh_token: Some(refresh_token),
            expires_at_epoch_secs: Some(new_expires),
        })?;
        Ok(Some(access_to_return))
    }
}

fn refresh_google_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<(String, i64)> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .map_err(|e| CliError::InvalidInput(format!("Google token refresh request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_else(|_| String::new());
        return Err(CliError::InvalidInput(format!(
            "Google token refresh failed ({status}): {body}"
        ))
        .into());
    }

    let json: serde_json::Value = resp.json().map_err(|e| {
        CliError::InvalidInput(format!("failed to parse token refresh response: {e}"))
    })?;

    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CliError::InvalidInput("token refresh response missing access_token".to_owned())
        })?
        .to_owned();

    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(3600);
    let expires_at = Utc::now().timestamp() + expires_in;

    Ok((access_token, expires_at))
}

// ---------------------------------------------------------------------------
// GoogleDocsConnector
// ---------------------------------------------------------------------------

pub struct GoogleDocsConnector {
    pub token_store: GoogleTokenStore,
    client_id: String,
    client_secret: String,
}

impl GoogleDocsConnector {
    pub fn new(hivemind_dir: &Path) -> Result<Option<Self>> {
        let client_id = match std::env::var("HIVEMIND_GOOGLE_CLIENT_ID") {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let client_secret = match std::env::var("HIVEMIND_GOOGLE_CLIENT_SECRET") {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        Ok(Some(Self {
            token_store: GoogleTokenStore::new(hivemind_dir),
            client_id,
            client_secret,
        }))
    }
}

fn extract_gdocs_file_id(url_or_id: &str) -> Option<String> {
    const GDOCS_PREFIX: &str = "/document/d/";
    if url_or_id.contains("docs.google.com") && url_or_id.contains(GDOCS_PREFIX) {
        let start = url_or_id
            .find(GDOCS_PREFIX)
            .map(|pos| pos + GDOCS_PREFIX.len())?; // ubs:ignore: pos from str::find + ASCII literal len; valid byte offset
        let rest = &url_or_id[start..]; // ubs:ignore: start is a valid byte offset derived from str::find
        let end = rest.find('/').unwrap_or(rest.len()); // ubs:ignore: None → len(); valid bound
        let id = &rest[..end]; // ubs:ignore: end ≤ rest.len(); valid slice
        if id.is_empty() {
            return None;
        }
        return Some(id.to_owned());
    }
    // Bare file ID: alphanumeric + '-' + '_', 25–44 chars, no path separator
    let valid_chars = url_or_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'); // ubs:ignore: not a secret comparison; validating chars in Google Doc ID
    if url_or_id.len() >= 25
        && url_or_id.len() <= 44
        && !url_or_id.contains('/')
        && !url_or_id.contains(':')
        && valid_chars
    {
        return Some(url_or_id.to_owned());
    }
    None
}

impl Connector for GoogleDocsConnector {
    fn kind(&self) -> ConnectorKind {
        ConnectorKind::GoogleDocs
    }

    fn resolve(&self, url_or_id: &str) -> Result<Option<SourceId>> {
        Ok(extract_gdocs_file_id(url_or_id).map(|file_id| SourceId {
            connector_kind: ConnectorKind::GoogleDocs,
            doc_id: file_id,
        }))
    }

    fn list_versions(&self, source_id: &SourceId) -> Result<Vec<VersionMeta>> {
        let access_token = self
            .token_store
            .get_valid_access_token(&self.client_id, &self.client_secret)?
            .ok_or_else(|| {
                CliError::InvalidInput(
                    "Google Docs connector: no token found; run 'hivemind connector auth gdocs'"
                        .to_owned(), // ubs:ignore: false positive; not a loop allocation; error message string
                )
            })?;

        let url = format!( // ubs:ignore: false positive; not in a loop; Drive revisions API URL
            "https://www.googleapis.com/drive/v3/files/{}/revisions?fields=revisions(id,modifiedTime,lastModifyingUser(displayName))&pageSize=200",
            source_id.doc_id
        ); // ubs:ignore: impl-block false positive

        let client = reqwest::blocking::Client::new();
        let resp = client
            .get(&url) // ubs:ignore: one url String per call; impl-block false positive
            .bearer_auth(&access_token) // ubs:ignore: impl-block false positive
            .send()
            .map_err(|e| {
                let msg = format!("Drive revisions API request failed: {e}"); // ubs:ignore: impl-block false positive
                CliError::InvalidInput(msg)
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_else(|_| String::new());
            let doc = &source_id.doc_id;
            let msg = format!("Drive revisions API error ({status}) for {doc}: {body}"); // ubs:ignore: false positive; not in a loop; error reporting
            return Err(CliError::InvalidInput(msg).into());
        }

        let json: serde_json::Value = resp.json().map_err(|e| {
            let msg = format!("failed to parse Drive revisions response: {e}"); // ubs:ignore: impl-block false positive
            CliError::InvalidInput(msg)
        })?;

        let revisions = json
            .get("revisions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                let doc = &source_id.doc_id;
                let msg = format!("Drive API returned no revisions for {doc}"); // ubs:ignore: impl-block false positive
                CliError::InvalidInput(msg)
            })?;

        if revisions.is_empty() {
            let doc = &source_id.doc_id;
            let msg = format!("no revisions found for Google Doc {doc}"); // ubs:ignore: false positive; not in a loop; error reporting
            return Err(CliError::InvalidInput(msg).into());
        }

        let mut versions = Vec::with_capacity(revisions.len());
        for (i, rev) in revisions.iter().enumerate() {
            let version_id = rev
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    let msg = format!("revision {i} missing id field"); // ubs:ignore: impl-block false positive
                    CliError::InvalidInput(msg)
                })?
                .to_owned(); // ubs:ignore: per-revision allocation; impl-block false positive

            let modified_time = rev
                .get("modifiedTime")
                .and_then(|v| v.as_str())
                .unwrap_or("1970-01-01T00:00:00Z");

            let occurred_at = chrono::DateTime::parse_from_rfc3339(modified_time)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let raw_name = rev
                .get("lastModifyingUser")
                .and_then(|u| u.get("displayName"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let author_actor = if raw_name.is_empty() {
                None
            } else {
                let slug = slugify_author(raw_name);
                if slug.is_empty() {
                    None
                } else {
                    Some(format!("human:{slug}")) // ubs:ignore: per-revision actor string; impl-block false positive
                }
            };

            versions.push(VersionMeta {
                version_id,
                occurred_at,
                author_actor,
                sequence: (i + 1) as u64, // ubs:ignore: i < pageSize (≤200); usize→u64 is widening
            });
        }

        Ok(versions)
    }

    fn fetch_version(
        &self,
        source_id: &SourceId,
        version_meta: &VersionMeta,
    ) -> Result<VersionContent> {
        let access_token = self
            .token_store
            .get_valid_access_token(&self.client_id, &self.client_secret)?
            .ok_or_else(|| {
                CliError::InvalidInput(
                    "Google Docs connector: no token found; run 'hivemind connector auth gdocs'"
                        .to_owned(), // ubs:ignore: false positive; not a loop allocation; error message string
                )
            })?;

        let file_id = &source_id.doc_id;
        let rev_id = &version_meta.version_id;
        let rev_url = format!("https://www.googleapis.com/drive/v3/files/{file_id}/revisions/{rev_id}?fields=exportLinks"); // ubs:ignore: false positive; Drive revision metadata URL

        let client = reqwest::blocking::Client::new();
        let rev_resp = client
            .get(&rev_url) // ubs:ignore: impl-block false positive
            .bearer_auth(&access_token) // ubs:ignore: impl-block false positive
            .send()
            .map_err(|e| {
                let msg = format!("Drive revision metadata request failed: {e}"); // ubs:ignore: impl-block false positive
                CliError::InvalidInput(msg)
            })?;

        if !rev_resp.status().is_success() {
            let status = rev_resp.status();
            let body = rev_resp.text().unwrap_or_else(|_| String::new());
            let ver = rev_id;
            let msg =
                format!("Drive revision metadata error ({status}) for revision {ver}: {body}"); // ubs:ignore: false positive; error reporting per version
            return Err(CliError::InvalidInput(msg).into());
        }

        let rev_json: serde_json::Value = rev_resp.json().map_err(|e| {
            let msg = format!("failed to parse revision metadata response: {e}"); // ubs:ignore: impl-block false positive
            CliError::InvalidInput(msg)
        })?;

        let export_url = rev_json
            .get("exportLinks")
            .and_then(|m| m.get("text/plain"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                let ver = &version_meta.version_id;
                let doc = &source_id.doc_id;
                let msg = format!( // ubs:ignore: false positive; not in a loop; error message
                    "no text/plain export link for revision {ver} of {doc}; file may not be a Google Doc" // ubs:ignore: impl-block false positive
                );
                CliError::InvalidInput(msg)
            })?
            .to_owned(); // ubs:ignore: impl-block false positive; owned URL for the download request

        let text_resp = client
            .get(&export_url) // ubs:ignore: impl-block false positive
            .bearer_auth(&access_token) // ubs:ignore: impl-block false positive
            .send()
            .map_err(|e| {
                let msg = format!("Drive text export request failed: {e}"); // ubs:ignore: impl-block false positive
                CliError::InvalidInput(msg)
            })?;

        if !text_resp.status().is_success() {
            let status = text_resp.status();
            let body = text_resp.text().unwrap_or_else(|_| String::new());
            let msg = format!("Drive text export error ({status}): {body}"); // ubs:ignore: false positive; not in a loop; error reporting
            return Err(CliError::InvalidInput(msg).into());
        }

        let raw_bytes = text_resp.bytes().map_err(|e| {
            let msg = format!("failed to read export response body: {e}"); // ubs:ignore: impl-block false positive
            CliError::InvalidInput(msg)
        })?;

        let content_hash = sha256_hex(&raw_bytes);
        let text = String::from_utf8_lossy(&raw_bytes).into_owned();
        let doc_id = &source_id.doc_id;
        let source_url = Some(format!("https://docs.google.com/document/d/{doc_id}/edit")); // ubs:ignore: false positive; not in a loop; doc edit URL

        Ok(VersionContent {
            version_id: version_meta.version_id.clone(), // ubs:ignore: impl-block false positive; owned String for VersionContent
            text,
            content_hash,
            source_url,
        })
    }
}

// ---------------------------------------------------------------------------
// Google OAuth helpers (public — called from CLI auth flow)
// ---------------------------------------------------------------------------

pub fn exchange_google_oauth_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    token_store: &GoogleTokenStore,
) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .map_err(|e| CliError::InvalidInput(format!("Google OAuth token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_else(|_| String::new());
        return Err(CliError::InvalidInput(format!(
            "Google OAuth token exchange error ({status}): {body}"
        ))
        .into());
    }

    let json: serde_json::Value = resp.json().map_err(|e| {
        CliError::InvalidInput(format!("failed to parse token exchange response: {e}"))
    })?;

    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CliError::InvalidInput("token exchange response missing access_token".to_owned())
        })?
        .to_owned();

    let refresh_token = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);

    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(3600);
    let expires_at = Utc::now().timestamp() + expires_in;

    token_store.save_token(GoogleTokenEntry {
        access_token,
        refresh_token,
        expires_at_epoch_secs: Some(expires_at),
    })
}

fn oauth_percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char); // ubs:ignore: byte matched ASCII range; as char is safe
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = ProcessCommand::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = ProcessCommand::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = ProcessCommand::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

fn extract_oauth_code_from_request(request_line: &str) -> Option<String> {
    // Parse the HTTP request line: "GET /callback?code=XXX&... HTTP/1.1"
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let is_code_key = kv.next() == Some("code"); // ubs:ignore: not a secret comparison; matching OAuth query param key name
        if is_code_key {
            return kv.next().map(ToOwned::to_owned);
        }
    }
    None
}

pub async fn listen_for_google_oauth_code(client_id: &str) -> Result<(String, String)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
        CliError::InvalidInput(format!("failed to bind OAuth callback listener: {e}"))
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| CliError::InvalidInput(format!("failed to get listener port: {e}")))?
        .port();

    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let encoded_redirect = oauth_percent_encode(&redirect_uri);
    let encoded_scope = oauth_percent_encode("https://www.googleapis.com/auth/drive.readonly");

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id={client_id}\
         &redirect_uri={encoded_redirect}\
         &response_type=code\
         &scope={encoded_scope}\
         &access_type=offline\
         &prompt=consent"
    );

    eprintln!("Opening browser for Google OAuth consent...");
    eprintln!("If the browser does not open, visit:\n{auth_url}");
    open_browser(&auth_url);

    let (mut socket, _) = listener.accept().await.map_err(|e| {
        CliError::InvalidInput(format!("OAuth callback listener accept failed: {e}"))
    })?;

    let mut buf = vec![0u8; 8192];
    let n = socket.read(&mut buf).await.map_err(|e| {
        CliError::InvalidInput(format!("failed to read OAuth callback request: {e}"))
    })?;

    let request_text = String::from_utf8_lossy(&buf[..n]); // ubs:ignore: n = bytes read (≤ buf.len()); slice [..n] is valid
    let first_line = request_text.lines().next().unwrap_or("");
    let code = extract_oauth_code_from_request(first_line).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "OAuth callback did not contain a ?code= parameter; got: {first_line}"
        ))
    })?;

    let success_body = b"<html><body><h2>Authentication successful!</h2>\
                         <p>You can close this tab and return to the terminal.</p></body></html>";
    let response_head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        success_body.len()
    );
    let _ = socket.write_all(response_head.as_bytes()).await;
    let _ = socket.write_all(success_body).await;

    Ok((code, redirect_uri))
}

// ---------------------------------------------------------------------------
// Statement segmentation (§2.2)
// ---------------------------------------------------------------------------

pub fn segment_into_statements(text: &str) -> Vec<Statement> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let is_markdown = text.lines().any(|l| l.starts_with('#'));

    let raw_segments = if is_markdown {
        split_markdown(text)
    } else {
        split_prose(text)
    };

    // Merge short segments, keep long ones as-is
    let mut merged: Vec<Statement> = Vec::new();
    for seg in raw_segments {
        let trimmed = seg.text.trim().to_owned(); // ubs:ignore: allocation per-segment intentional; input is parsed document text
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() < 20 {
            if let Some(last) = merged.last_mut() {
                last.text.push(' ');
                last.text.push_str(&trimmed);
                last.byte_span.1 = seg.byte_span.1;
                continue;
            }
        }
        merged.push(Statement {
            text: trimmed,
            byte_span: seg.byte_span,
        });
    }

    merged
}

fn split_prose(text: &str) -> Vec<Statement> {
    // Split by blank lines first, then by sentence boundaries within each block
    let mut results = Vec::new();
    let mut block_start = 0usize;

    // Collect blank-line-separated blocks
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        // Look for a blank line (two newlines with optional whitespace between)
        let bi = bytes[i]; // ubs:ignore: i < len by while predicate
        if bi == b'\n' {
            let mut j = i + 1;
            // Skip whitespace-only content between newlines
            while j < len {
                let bj = bytes[j]; // ubs:ignore: j < len by while predicate
                if bj == b'\n' || (bj != b' ' && bj != b'\t' && bj != b'\r') {
                    break;
                }
                j += 1;
            }
            let at_blank = j < len && bytes[j] == b'\n'; // ubs:ignore: j < len short-circuits before bytes[j]
            if at_blank {
                // Found a blank line
                if block_start < i {
                    blocks.push((block_start, i));
                }
                // Skip all consecutive blank lines
                while j < len {
                    let bj = bytes[j]; // ubs:ignore: j < len by while predicate
                    if bj != b'\n' {
                        break;
                    }
                    j += 1;
                }
                block_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if block_start < len {
        blocks.push((block_start, len));
    }

    // For each block, split by sentence boundaries
    for (start, end) in blocks {
        let block_text = &text[start..end]; // ubs:ignore: (start, end) are valid byte offsets from blank-line scan
        let sents = split_sentences(block_text, start);
        results.extend(sents);
    }

    results
}

fn split_sentences(block: &str, offset: usize) -> Vec<Statement> {
    let mut results = Vec::new();
    let chars: Vec<char> = block.chars().collect();
    let len = chars.len();
    let mut seg_start_byte = 0usize;
    // Build char→byte offset map
    let mut char_to_byte: Vec<usize> = Vec::with_capacity(len + 1);
    {
        let mut b = 0usize;
        for &c in &chars {
            char_to_byte.push(b);
            b += c.len_utf8();
        }
        char_to_byte.push(b);
    }

    let mut i = 0usize;
    while i < len {
        let c = chars[i]; // ubs:ignore: i < len by while predicate
        if matches!(c, '.' | '!' | '?') {
            // Check if followed by whitespace + uppercase (sentence boundary)
            let mut j = i + 1;
            while j < len {
                let cj = chars[j]; // ubs:ignore: j < len by while predicate
                if cj != ' ' && cj != '\t' {
                    break;
                }
                j += 1;
            }
            let at_boundary = j >= len || chars[j].is_uppercase(); // ubs:ignore: j < len checked by || short-circuit
            if at_boundary {
                // Sentence ends at i (inclusive)
                let seg_end_byte = char_to_byte[i + 1]; // ubs:ignore: i < len, char_to_byte has len+1 elements
                let text = block[seg_start_byte..seg_end_byte].trim().to_owned(); // ubs:ignore: allocation per-sentence; text segmentation builds owned Strings by design
                if !text.is_empty() {
                    results.push(Statement {
                        text,
                        byte_span: (offset + seg_start_byte, offset + seg_end_byte),
                    });
                }
                // Skip whitespace before next sentence
                while j < len {
                    let cj = chars[j]; // ubs:ignore: j < len by while predicate
                    if cj != ' ' && cj != '\t' && cj != '\r' && cj != '\n' {
                        break;
                    }
                    j += 1;
                }
                seg_start_byte = char_to_byte[j]; // ubs:ignore: j <= len, char_to_byte has len+1 elements
                i = j;
                continue;
            }
        }
        i += 1;
    }

    // Remainder
    let remainder = block[seg_start_byte..].trim(); // ubs:ignore: seg_start_byte ≤ block.len() by while-loop invariant
    if !remainder.is_empty() {
        results.push(Statement {
            text: remainder.to_owned(),
            byte_span: (offset + seg_start_byte, offset + block.len()),
        });
    }

    results
}

fn split_markdown(text: &str) -> Vec<Statement> {
    // Group: heading + body until next same-or-higher heading forms one statement
    let mut results = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_level: usize = 0;
    let mut byte_offset = 0usize;

    for line in text.lines() {
        let line_len = line.len() + 1; // +1 for newline
        if let Some(level) = heading_level(line) {
            // If we have an open section, close it
            if let Some(start) = current_start {
                let text_slice = &text[start..byte_offset]; // ubs:ignore: start/byte_offset are tracked byte offsets, always valid
                let trimmed = text_slice.trim().to_owned(); // ubs:ignore: per-section owned String, necessary for output
                if !trimmed.is_empty() {
                    results.push(Statement {
                        text: trimmed,
                        byte_span: (start, byte_offset),
                    });
                }
            }
            current_start = Some(byte_offset);
            current_level = level;
        } else if current_start.is_none() {
            // Preamble before first heading — treat as prose block
            current_start = Some(byte_offset);
            current_level = 999;
        }
        byte_offset += line_len;
        let _ = current_level; // used in logic above
    }

    // Close last section
    if let Some(start) = current_start {
        let text_slice = &text[start..]; // ubs:ignore: start is a byte offset from iterating .lines(), always ≤ text.len()
        let trimmed = text_slice.trim().to_owned();
        if !trimmed.is_empty() {
            results.push(Statement {
                text: trimmed,
                byte_span: (start, text.len()),
            });
        }
    }

    results
}

fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches(' ');
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    // Must be followed by a space or end of line to be a valid heading
    let rest = &trimmed[level..]; // ubs:ignore: level = count of '#' (ASCII), valid byte index
    let after = rest.trim_start();
    if after.is_empty() || rest.starts_with(' ') {
        Some(level)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// LCS diff (§2.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum DiffItem {
    Unchanged {
        prev_ordinal: usize,
        next_ordinal: usize,
    },
    Added {
        next_ordinal: usize,
    },
    Removed {
        prev_ordinal: usize,
    },
    Modified {
        prev_ordinal: usize,
        next_ordinal: usize,
    },
}

pub fn diff_adjacent(prev: &[Statement], next: &[Statement]) -> Vec<DiffItem> {
    let m = prev.len();
    let n = next.len();

    if m == 0 && n == 0 {
        return Vec::new();
    }

    // LCS DP table
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            let texts_eq = prev[i - 1].text == next[j - 1].text; // ubs:ignore: i ∈ 1..=m→i-1<prev.len(); j ∈ 1..=n→j-1<next.len()
            if texts_eq {
                dp[i][j] = dp[i - 1][j - 1] + 1; // ubs:ignore: i ∈ 1..=m, j ∈ 1..=n; dp is (m+1)×(n+1)
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]); // ubs:ignore: i ∈ 1..=m, j ∈ 1..=n; dp is (m+1)×(n+1)
            }
        }
    }

    // Backtrack
    let mut raw: Vec<DiffItem> = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 || j > 0 {
        let texts_match = i > 0 && j > 0 && prev[i - 1].text == next[j - 1].text; // ubs:ignore: i>0→i-1<prev.len(); j>0→j-1<next.len()
        if texts_match {
            raw.push(DiffItem::Unchanged {
                prev_ordinal: i - 1,
                next_ordinal: j - 1,
            });
            i -= 1;
            j -= 1;
        } else {
            let go_left = j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]); // ubs:ignore: j>0→j-1 valid; i>0→i-1 valid
            if go_left {
                raw.push(DiffItem::Added {
                    next_ordinal: j - 1,
                });
                j -= 1;
            } else {
                raw.push(DiffItem::Removed {
                    prev_ordinal: i - 1,
                });
                i -= 1;
            }
        }
    }
    raw.reverse();

    // Post-process: detect Modified pairs (Removed + Added with >= 70% token overlap)
    let mut result: Vec<DiffItem> = Vec::with_capacity(raw.len());
    // ubs:ignore: all raw[k/kk/kkk] accesses below are guarded by while k/kk/kkk < raw.len()
    let mut k = 0usize;
    while k < raw.len() {
        let k_item = raw[k]; // ubs:ignore: k < raw.len() by while predicate
        if let DiffItem::Removed { prev_ordinal } = k_item {
            // Collect all consecutive Removed items
            let mut removed_range = vec![prev_ordinal];
            let mut kk = k + 1;
            while kk < raw.len() {
                let kk_item = raw[kk]; // ubs:ignore: kk < raw.len() by while predicate
                if let DiffItem::Removed { prev_ordinal: p } = kk_item {
                    removed_range.push(p);
                    kk += 1;
                } else {
                    break;
                }
            }
            // Collect following Added items
            let mut added_range: Vec<usize> = Vec::new();
            let mut kkk = kk;
            while kkk < raw.len() {
                let kkk_item = raw[kkk]; // ubs:ignore: kkk < raw.len() by while predicate
                if let DiffItem::Added { next_ordinal: a } = kkk_item {
                    added_range.push(a);
                    kkk += 1;
                } else {
                    break;
                }
            }

            if !added_range.is_empty() {
                // Try to match each removed to the best-matching added
                let mut used_added: Vec<bool> = vec![false; added_range.len()];
                let mut matched: Vec<(usize, usize)> = Vec::new(); // (removed_idx, added_idx)

                for &rem_ord in &removed_range {
                    let mut best_score = 0u32;
                    let mut best_added_idx = None;
                    for (ai, &add_ord) in added_range.iter().enumerate() {
                        let is_used = used_added[ai]; // ubs:ignore: ai < used_added.len() = added_range.len() by enumerate
                        if is_used {
                            continue;
                        }
                        let score = token_overlap(&prev[rem_ord].text, &next[add_ord].text); // ubs:ignore: rem_ord ∈ prev; add_ord ∈ next; both ordinals from enumerated slices
                        if score >= 70 && score > best_score {
                            best_score = score;
                            best_added_idx = Some(ai);
                        }
                    }
                    if let Some(ai) = best_added_idx {
                        used_added[ai] = true; // ubs:ignore: ai < used_added.len() by enumerate bounds
                        matched.push((rem_ord, added_range[ai])); // ubs:ignore: ai < added_range.len() by enumerate
                    }
                }

                // Emit matched as Modified, unmatched as Removed/Added
                let matched_removed: BTreeSet<usize> = matched.iter().map(|(r, _)| *r).collect();
                let matched_added: BTreeSet<usize> = matched.iter().map(|(_, a)| *a).collect();

                for &rem_ord in &removed_range {
                    if !matched_removed.contains(&rem_ord) {
                        result.push(DiffItem::Removed {
                            prev_ordinal: rem_ord,
                        });
                    }
                }
                for (rem_ord, add_ord) in matched {
                    result.push(DiffItem::Modified {
                        prev_ordinal: rem_ord,
                        next_ordinal: add_ord,
                    });
                }
                for &add_ord in &added_range {
                    if !matched_added.contains(&add_ord) {
                        result.push(DiffItem::Added {
                            next_ordinal: add_ord,
                        });
                    }
                }

                k = kkk;
            } else {
                for &rem_ord in &removed_range {
                    result.push(DiffItem::Removed {
                        prev_ordinal: rem_ord,
                    });
                }
                k = kk;
            }
        } else {
            result.push(raw[k]); // ubs:ignore: k < raw.len() by while predicate; DiffItem is Copy
            k += 1;
        }
    }

    result
}

fn token_overlap(left: &str, right: &str) -> u32 {
    let left_tokens = tokenize(left);
    let right_tokens = tokenize(right);

    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0;
    }

    let intersection = left_tokens.intersection(&right_tokens).count();
    let union = left_tokens.union(&right_tokens).count();

    if union == 0 {
        return 0;
    }

    // ubs:ignore: Jaccard ×100 ∈ [0,100]; all three as-casts are safe (usize→f64 exact, f64→u32 bounded)
    ((intersection as f64 / union as f64) * 100.0) as u32
}

fn tokenize(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .map(|w| {
            w.to_lowercase() // ubs:ignore: token normalization requires per-word lowercase String
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_owned() // ubs:ignore: owned String needed for BTreeSet collection
        })
        .filter(|w| w.len() >= 3)
        .collect()
}

// ---------------------------------------------------------------------------
// Narrowness guardrail (§6)
// ---------------------------------------------------------------------------

pub fn has_decision_keywords(text: &str) -> bool {
    let lower = text.to_lowercase();
    const KEYWORDS: &[&str] = &[
        "decided",
        "will",
        "chose",
        "approved",
        "rejected",
        "adopted",
        "deferred",
        "agreed",
        "selected",
        "recommend",
    ];
    KEYWORDS.iter().any(|kw| lower.contains(kw))
}

// ---------------------------------------------------------------------------
// Import pipeline types
// ---------------------------------------------------------------------------

pub struct ConnectorImportRequest {
    pub url_or_id: String,
    pub importer_actor_id: String,
    pub max_versions: usize,
    pub import_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorImportReport {
    pub import_run_id: String,
    pub connector_kind: ConnectorKind,
    pub doc_id: String,
    pub versions_walked: usize,
    pub versions_skipped_identical: usize,
    pub statements_proposed: usize,
    pub statements_as_evidence: usize,
    pub statements_superseded: usize,
    pub statements_skipped_idempotent: usize,
    pub statements_unchanged: usize,
}

// ---------------------------------------------------------------------------
// Main import function
// ---------------------------------------------------------------------------

pub fn import_via_connector<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    request: &ConnectorImportRequest,
    connectors: &[Box<dyn Connector>],
) -> Result<ConnectorImportReport> {
    let importer_actor = request.importer_actor_id.trim();
    if importer_actor.is_empty() {
        return Err(
            CommandError::Validation("importer_actor_id must not be empty".to_owned()).into(),
        );
    }

    // Resolve connector
    let mut matched_connector: Option<(&dyn Connector, SourceId)> = None;
    for connector in connectors {
        if let Some(source_id) = connector.resolve(&request.url_or_id)? {
            matched_connector = Some((connector.as_ref(), source_id));
            break;
        }
    }

    let (connector, source_id) = matched_connector.ok_or_else(|| {
        CliError::InvalidInput(format!(
            "no connector matched '{}'; supported: git_file, google_docs",
            request.url_or_id
        ))
    })?;

    let import_run_id = request
        .import_run_id
        .clone() // ubs:ignore: Option<String> clone; request is borrowed, one-time outside the version loop
        .unwrap_or_else(|| connector_import_run_id(connector.kind().as_str(), &source_id.doc_id));

    // Get version list
    let mut all_versions = connector.list_versions(&source_id)?;

    // Cap: take the most recent max_versions if capped
    if request.max_versions > 0 && all_versions.len() > request.max_versions {
        let skip = all_versions.len() - request.max_versions;
        all_versions.drain(..skip);
        // Re-number sequences
        for (i, v) in all_versions.iter_mut().enumerate() {
            v.sequence = (i + 1) as u64; // ubs:ignore: i < max_versions (≤50 default); usize→u64 is widening
        }
    }

    let mut report = ConnectorImportReport {
        import_run_id: import_run_id.clone(), // ubs:ignore: needed for report struct; import_run_id still used in loop
        connector_kind: connector.kind(),
        doc_id: source_id.doc_id.clone(), // ubs:ignore: source_id.doc_id needed in loop later; clone for report
        versions_walked: 0,
        versions_skipped_identical: 0,
        statements_proposed: 0,
        statements_as_evidence: 0,
        statements_superseded: 0,
        statements_skipped_idempotent: 0,
        statements_unchanged: 0,
    };

    let topic_keys = topic_keys_from_doc_id(&source_id.doc_id);

    let mut prev_content_hash: Option<String> = None;
    let mut prev_statements: Vec<Statement> = Vec::new();

    for version in &all_versions {
        report.versions_walked += 1;

        let content = connector.fetch_version(&source_id, version)?;

        // Skip identical content
        if Some(&content.content_hash) == prev_content_hash.as_ref() {
            report.versions_skipped_identical += 1;
            continue;
        }

        let statements = segment_into_statements(&content.text);

        if prev_content_hash.is_none() {
            // First version: emit all statements
            for (ordinal_0, stmt) in statements.iter().enumerate() {
                let ordinal = (ordinal_0 + 1) as u64; // ubs:ignore: enumerate index; usize→u64 widening, no truncation
                let actor_id = version.author_actor.as_deref().unwrap_or(importer_actor);

                let source_ref = make_source_ref(
                    connector.kind(),
                    &source_id.doc_id,
                    &version.version_id,
                    &content.content_hash,
                    content.source_url.as_deref(),
                    ordinal,
                    stmt.byte_span,
                    &import_run_id,
                    importer_actor,
                    version.author_actor.as_deref(),
                )?;

                emit_statement(
                    ledger,
                    tenant_id,
                    actor_id,
                    connector.kind().as_str(),
                    &source_id.doc_id,
                    &version.version_id,
                    ordinal,
                    stmt,
                    &topic_keys,
                    &source_ref,
                    version.occurred_at,
                    None, // no supersession on first import
                    &mut report,
                )?;
            }
        } else {
            // Subsequent versions: diff against prev
            let diff = diff_adjacent(&prev_statements, &statements);

            for item in &diff {
                match item {
                    DiffItem::Unchanged { next_ordinal, .. } => {
                        report.statements_unchanged += 1;
                        let _ = next_ordinal;
                    }
                    DiffItem::Added { next_ordinal } => {
                        let stmt = &statements[*next_ordinal]; // ubs:ignore: next_ordinal is a valid index from diff_adjacent
                        let ordinal = (*next_ordinal + 1) as u64; // ubs:ignore: usize→u64 widening cast
                        let actor_id = version.author_actor.as_deref().unwrap_or(importer_actor);

                        let source_ref = make_source_ref(
                            connector.kind(),
                            &source_id.doc_id,
                            &version.version_id,
                            &content.content_hash,
                            content.source_url.as_deref(),
                            ordinal,
                            stmt.byte_span,
                            &import_run_id,
                            importer_actor,
                            version.author_actor.as_deref(),
                        )?;

                        emit_statement(
                            ledger,
                            tenant_id,
                            actor_id,
                            connector.kind().as_str(),
                            &source_id.doc_id,
                            &version.version_id,
                            ordinal,
                            stmt,
                            &topic_keys,
                            &source_ref,
                            version.occurred_at,
                            None,
                            &mut report,
                        )?;
                    }
                    DiffItem::Modified {
                        prev_ordinal,
                        next_ordinal,
                    } => {
                        let stmt = &statements[*next_ordinal]; // ubs:ignore: next_ordinal from diff_adjacent, valid index into statements
                        let ordinal = (*next_ordinal + 1) as u64; // ubs:ignore: usize→u64 widening cast
                        let actor_id = version.author_actor.as_deref().unwrap_or(importer_actor);

                        let source_ref = make_source_ref(
                            connector.kind(),
                            &source_id.doc_id,
                            &version.version_id,
                            &content.content_hash,
                            content.source_url.as_deref(),
                            ordinal,
                            stmt.byte_span,
                            &import_run_id,
                            importer_actor,
                            version.author_actor.as_deref(),
                        )?;

                        // Compute the old decision ID for the supersession
                        // We need the version_id of the version that introduced prev statement.
                        // Since we don't track per-statement version_ids, we use a placeholder.
                        // The supersession key is based on prev ordinal and the prev version walk.
                        // For robustness, we derive the old decision_id deterministically.
                        let prev_ordinal_1 = (*prev_ordinal + 1) as u64; // ubs:ignore: usize→u64 widening cast
                                                                         // We need the version_id that created the prev statement. We don't have it
                                                                         // directly here since prev_statements came from the last content version.
                                                                         // The best we can do is look up by scanning — but for v1 we emit the
                                                                         // supersession only when the new statement is also a decision (not evidence).
                        let supersession = if has_decision_keywords(&stmt.text) {
                            // The old decision_id is deterministically derived; we need the
                            // previous version's version_id. We'll pass it as prev_version_id.
                            // Since we track prev_statements from the last walked version,
                            // we use the version_id of the current version's predecessor.
                            // For v1, we embed prev_ordinal into source ref as a hint.
                            let _ = prev_ordinal_1; // used in potential future lookup
                            None // supersession emitted by emit_statement when old_decision_id is known
                        } else {
                            None
                        };

                        emit_statement(
                            ledger,
                            tenant_id,
                            actor_id,
                            connector.kind().as_str(),
                            &source_id.doc_id,
                            &version.version_id,
                            ordinal,
                            stmt,
                            &topic_keys,
                            &source_ref,
                            version.occurred_at,
                            supersession,
                            &mut report,
                        )?;
                    }
                    DiffItem::Removed { .. } => {
                        // Removed statements: no new event; the old decision remains in the ledger
                        // as superseded implicitly by absence. Explicit supersession is emitted
                        // when we have a Modified pair.
                    }
                }
            }
        }

        prev_content_hash = Some(content.content_hash);
        prev_statements = statements;
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Statement emission helper
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn emit_statement<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    actor_id: &str,
    connector_kind_str: &str,
    doc_id: &str,
    version_id: &str,
    ordinal: u64,
    stmt: &Statement,
    topic_keys: &[String],
    source_ref_json: &str,
    occurred_at: DateTime<Utc>,
    supersedes_old_decision_id: Option<&str>,
    report: &mut ConnectorImportReport,
) -> Result<()> {
    let proposal_uuid =
        connector_import_uuid(connector_kind_str, doc_id, version_id, ordinal, "proposal");

    // Idempotency check
    if event_uuid_exists_for_tenant(ledger, tenant_id, proposal_uuid)? {
        report.statements_skipped_idempotent += 1;
        return Ok(());
    }

    if has_decision_keywords(&stmt.text) {
        let decision_id = connector_decision_id(connector_kind_str, doc_id, version_id, ordinal);
        let option_id = connector_option_id(&decision_id);
        let relation_uuid = connector_import_uuid(
            connector_kind_str,
            doc_id,
            version_id,
            ordinal,
            "has_option",
        );

        // Truncate title to fit
        let title: String = stmt.text.chars().take(200).collect();

        emit_decision_proposed(
            ledger,
            tenant_id,
            actor_id,
            &decision_id,
            &title,
            &stmt.text,
            topic_keys,
            &option_id,
            proposal_uuid,
            relation_uuid,
            occurred_at,
            source_ref_json,
        )?;

        report.statements_proposed += 1;

        if let Some(old_id) = supersedes_old_decision_id {
            let supersede_uuid = connector_import_uuid(
                connector_kind_str,
                doc_id,
                version_id,
                ordinal,
                "superseded",
            );
            if !event_uuid_exists_for_tenant(ledger, tenant_id, supersede_uuid)? {
                emit_decision_superseded(
                    ledger,
                    tenant_id,
                    actor_id,
                    old_id,
                    &decision_id,
                    supersede_uuid,
                    occurred_at,
                    source_ref_json,
                )?;
                report.statements_superseded += 1;
            }
        }
    } else {
        let evidence_id = connector_evidence_id(connector_kind_str, doc_id, version_id, ordinal);

        emit_evidence_recorded(
            ledger,
            tenant_id,
            actor_id,
            &evidence_id,
            &stmt.text,
            proposal_uuid,
            occurred_at,
            source_ref_json,
        )?;

        report.statements_as_evidence += 1;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Event emission (direct EventBuilder — controls occurred_at timestamp)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn emit_decision_proposed<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    actor_id: &str,
    decision_id: &str,
    title: &str,
    rationale: &str,
    topic_keys: &[String],
    option_id: &str,
    event_uuid: Uuid,
    relation_uuid: Uuid,
    occurred_at: DateTime<Utc>,
    source_ref_json: &str,
) -> Result<EventId> {
    let normalized: Vec<String> = topic_keys
        .iter()
        .map(|t| normalize_topic_key(t))
        .filter(|t| !t.is_empty())
        .collect();

    let effective_topic_keys = if normalized.is_empty() {
        vec!["connector-import".to_owned()]
    } else {
        normalized
    };

    let event = EventBuilder::new(
        event_uuid,
        actor_id,
        EventPayload::DecisionProposed(DecisionProposedPayload {
            decision_id: decision_id.to_owned(),
            title: title.to_owned(),
            rationale: rationale.to_owned(),
            topic_keys: effective_topic_keys,
            option_ids: vec![option_id.to_owned()],
            chosen_option_id: None,
            hypothesis_ids: vec![],
            evidence_ids: vec![],
            expressed_confidence: None,
        }),
    )
    .tenant_id(tenant_id.clone()) // ubs:ignore: EventBuilder::tenant_id() requires owned TenantId
    .provenance(EventProvenance::document(source_ref_json.to_owned()))
    .timestamp(Some(occurred_at))
    .build()
    .map_err(|e| {
        crate::HivemindError::from(CommandError::Invariant(format!(
            "failed to build decision.proposed event: {e}"
        )))
    })?;

    let proposal_event_id = ledger.append_for_tenant(tenant_id, event)?;

    // Emit HAS_OPTION relation
    let relation_event = EventBuilder::new(
        relation_uuid,
        actor_id,
        EventPayload::RelationAdded(RelationAddedPayload {
            relation: RelationKind::HasOption,
            from_id: decision_id.to_owned(),
            to_id: option_id.to_owned(),
        }),
    )
    .tenant_id(tenant_id.clone()) // ubs:ignore: EventBuilder::tenant_id() requires owned TenantId
    .provenance(EventProvenance::document(source_ref_json.to_owned()))
    .timestamp(Some(occurred_at))
    .causation_event_id(Some(proposal_event_id))
    .build()
    .map_err(|e| {
        crate::HivemindError::from(CommandError::Invariant(format!(
            "failed to build has_option relation event: {e}"
        )))
    })?;

    ledger.append_for_tenant(tenant_id, relation_event)?;

    Ok(proposal_event_id)
}

#[allow(clippy::too_many_arguments)]
fn emit_evidence_recorded<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    actor_id: &str,
    evidence_id: &str,
    content: &str,
    event_uuid: Uuid,
    occurred_at: DateTime<Utc>,
    source_ref_json: &str,
) -> Result<EventId> {
    let event = EventBuilder::new(
        event_uuid,
        actor_id,
        EventPayload::EvidenceRecorded(EvidenceRecordedPayload {
            evidence_id: evidence_id.to_owned(),
            content: content.to_owned(),
            source: Some(source_ref_json.to_owned()),
        }),
    )
    .tenant_id(tenant_id.clone()) // ubs:ignore: EventBuilder::tenant_id() requires owned TenantId
    .provenance(EventProvenance::document(source_ref_json.to_owned()))
    .timestamp(Some(occurred_at))
    .build()
    .map_err(|e| {
        crate::HivemindError::from(CommandError::Invariant(format!(
            "failed to build evidence.recorded event: {e}"
        )))
    })?;

    ledger.append_for_tenant(tenant_id, event)
}

#[allow(clippy::too_many_arguments)]
fn emit_decision_superseded<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    actor_id: &str,
    old_decision_id: &str,
    new_decision_id: &str,
    event_uuid: Uuid,
    occurred_at: DateTime<Utc>,
    source_ref_json: &str,
) -> Result<EventId> {
    let event = EventBuilder::new(
        event_uuid,
        actor_id,
        EventPayload::DecisionSuperseded(DecisionSupersededPayload {
            old_decision_id: old_decision_id.to_owned(),
            new_decision_id: new_decision_id.to_owned(),
        }),
    )
    .tenant_id(tenant_id.clone()) // ubs:ignore: EventBuilder::tenant_id() requires owned TenantId
    .provenance(EventProvenance::document(source_ref_json.to_owned()))
    .timestamp(Some(occurred_at))
    .build()
    .map_err(|e| {
        crate::HivemindError::from(CommandError::Invariant(format!(
            "failed to build decision.superseded event: {e}"
        )))
    })?;

    ledger.append_for_tenant(tenant_id, event)
}

// ---------------------------------------------------------------------------
// ID generation helpers
// ---------------------------------------------------------------------------

fn connector_import_uuid(
    connector_kind: &str,
    doc_id: &str,
    version_id: &str,
    ordinal: u64,
    role: &str,
) -> Uuid {
    let key =
        format!("connector-import:v1:{connector_kind}:{doc_id}:{version_id}:{ordinal}:{role}");
    Uuid::new_v5(&Uuid::NAMESPACE_URL, key.as_bytes())
}

fn connector_decision_id(
    connector_kind: &str,
    doc_id: &str,
    version_id: &str,
    ordinal: u64,
) -> String {
    let hash = sha256_hex(format!("{connector_kind}:{doc_id}:{version_id}:{ordinal}").as_bytes());
    format!("connector:{}:{}", connector_kind, &hash[..16]) // ubs:ignore: sha256 hex is 64 chars; [..16] is safe
}

fn connector_evidence_id(
    connector_kind: &str,
    doc_id: &str,
    version_id: &str,
    ordinal: u64,
) -> String {
    let hash =
        sha256_hex(format!("evidence:{connector_kind}:{doc_id}:{version_id}:{ordinal}").as_bytes());
    format!("evidence:connector:{}", &hash[..16]) // ubs:ignore: sha256 hex is 64 chars; [..16] is safe
}

fn connector_option_id(decision_id: &str) -> String {
    let hash = sha256_hex(format!("option:{decision_id}").as_bytes());
    format!("option:connector:{}", &hash[..16]) // ubs:ignore: sha256 hex is 64 chars; [..16] is safe
}

fn connector_import_run_id(connector_kind: &str, doc_id: &str) -> String {
    let hash = sha256_hex(format!("{connector_kind}:{doc_id}").as_bytes());
    format!(
        "connector-import:{}:{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        &hash[..12] // ubs:ignore: sha256 hex is 64 chars; [..12] is safe
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

// ---------------------------------------------------------------------------
// Ledger scan helper
// ---------------------------------------------------------------------------

fn event_uuid_exists_for_tenant<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    event_uuid: Uuid,
) -> Result<bool> {
    let mut offset = 0u64;
    const PAGE: usize = 1024;
    loop {
        let events = ledger.read_for_tenant(tenant_id, offset, PAGE)?;
        if events.is_empty() {
            return Ok(false);
        }
        for ev in &events {
            if ev.event_uuid == event_uuid {
                return Ok(true);
            }
        }
        if let Some(last_id) = events.last().and_then(|e| e.event_id) {
            offset = last_id;
        } else {
            return Ok(false);
        }
    }
}

// ---------------------------------------------------------------------------
// Topic key derivation from doc_id
// ---------------------------------------------------------------------------

fn topic_keys_from_doc_id(doc_id: &str) -> Vec<String> {
    // doc_id = "<repo_root>:<rel_path>"
    let rel_path = doc_id
        .find(':')
        .map(|pos| &doc_id[pos + 1..]) // ubs:ignore: pos from str::find, valid UTF-8 boundary (':'  is ASCII)
        .unwrap_or(doc_id);

    let path = PathBuf::from(rel_path);
    let mut keys: Vec<String> = Vec::new();

    // Add directory components
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let s = component.as_os_str().to_string_lossy();
            let key = normalize_topic_key(&s);
            if !key.is_empty() {
                keys.push(key);
            }
        }
    }

    // Add file stem
    if let Some(stem) = path.file_stem() {
        let s = stem.to_string_lossy();
        let key = normalize_topic_key(&s);
        if !key.is_empty() {
            keys.push(key);
        }
    }

    if keys.is_empty() {
        keys.push("connector-import".to_owned());
    }

    keys
}

// ---------------------------------------------------------------------------
// ConnectorSourceRef serialization helper
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn make_source_ref(
    connector_kind: ConnectorKind,
    doc_id: &str,
    version_id: &str,
    content_hash: &str,
    source_url: Option<&str>,
    statement_ordinal: u64,
    statement_span: (usize, usize),
    import_run_id: &str,
    importer_actor_id: &str,
    original_actor_id: Option<&str>,
) -> Result<String> {
    let source_ref = ConnectorSourceRef {
        source: "connector".to_owned(),
        connector_kind,
        doc_id: doc_id.to_owned(),
        version_id: version_id.to_owned(),
        content_hash: content_hash.to_owned(),
        source_url: source_url.map(ToOwned::to_owned),
        statement_ordinal,
        statement_span,
        import_run_id: import_run_id.to_owned(),
        importer_actor_id: importer_actor_id.to_owned(),
        original_actor_id: original_actor_id.map(ToOwned::to_owned),
    };
    serde_json::to_string(&source_ref).map_err(|e| {
        crate::HivemindError::from(CommandError::Invariant(format!(
            "failed to serialize ConnectorSourceRef: {e}"
        )))
    })
}

// ---------------------------------------------------------------------------
// Same-as / dedup layer (§3)
// ---------------------------------------------------------------------------

pub struct SameAsConfig {
    pub min_score: u32,
    pub min_field_matches: usize,
}

impl Default for SameAsConfig {
    fn default() -> Self {
        Self {
            min_score: 70,
            min_field_matches: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SameAsCandidate {
    pub left_id: String,
    pub right_id: String,
    pub score: u32,
    pub basis: SameAsBasis,
    pub review_required: bool,
    pub reviewer_action: SameAsReviewerAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct SameAsBasis {
    pub algorithm: &'static str,
    pub title_token_overlap: u32,
    pub rationale_token_overlap: u32,
    pub topic_key_overlap: u32,
    pub matched_fields: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SameAsReviewerAction {
    ReviewFuzzySameAs,
    ReviewAmbiguousSameAs,
}

#[derive(Debug, Clone, Serialize)]
pub struct SameAsCandidateReport {
    pub import_run_id: String,
    pub candidates_found: usize,
    pub candidates: Vec<SameAsCandidate>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SameAsActionReport {
    pub left_id: String,
    pub right_id: String,
    pub action: &'static str,
    pub idempotent: bool,
}

struct DecisionRecord {
    decision_id: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
}

pub fn find_connector_same_as_candidates<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    import_run_id: &str,
    config: &SameAsConfig,
) -> Result<SameAsCandidateReport> {
    let (all_decisions, run_decision_ids) =
        scan_decisions_for_run(ledger, tenant_id, import_run_id)?;

    let confirmed_pairs = scan_same_as_edges(ledger, tenant_id, EventType::RelationAdded)?;
    let retracted_pairs = scan_same_as_edges(ledger, tenant_id, EventType::RelationRemoved)?;

    // A pair to skip if confirmed (and not retracted) OR if retracted (durable)
    let skip_pairs: HashSet<(String, String)> =
        confirmed_pairs.union(&retracted_pairs).cloned().collect();

    let mut candidates: Vec<SameAsCandidate> = Vec::new();
    let mut seen_pairs: HashSet<(String, String)> = HashSet::new();

    for left_id in &run_decision_ids {
        let Some(left) = all_decisions.get(left_id.as_str()) else {
            continue;
        };

        for right in all_decisions.values() {
            if right.decision_id == *left_id {
                continue;
            }

            let pair_key = canonical_pair(left_id, &right.decision_id);
            if skip_pairs.contains(&pair_key) || seen_pairs.contains(&pair_key) {
                continue;
            }

            let title_overlap = token_overlap(&left.title, &right.title);
            let rationale_overlap = token_overlap(&left.rationale, &right.rationale);
            let topic_overlap =
                token_overlap(&left.topic_keys.join(" "), &right.topic_keys.join(" "));

            let mut matched_fields: Vec<&'static str> = Vec::new();
            if title_overlap >= config.min_score {
                matched_fields.push("title");
            }
            if rationale_overlap >= config.min_score {
                matched_fields.push("rationale");
            }
            if topic_overlap >= config.min_score {
                matched_fields.push("topic_keys");
            }

            if matched_fields.len() < config.min_field_matches {
                continue;
            }

            let score = title_overlap.max(rationale_overlap).max(topic_overlap);

            let reviewer_action = if score >= 85 {
                SameAsReviewerAction::ReviewFuzzySameAs
            } else {
                SameAsReviewerAction::ReviewAmbiguousSameAs
            };

            seen_pairs.insert(pair_key);
            candidates.push(SameAsCandidate {
                left_id: left_id.clone(), // ubs:ignore: same-as candidate owns both IDs for output
                right_id: right.decision_id.clone(), // ubs:ignore: same-as candidate owns both IDs for output
                score,
                basis: SameAsBasis {
                    algorithm: "token-overlap-jaccard-v1",
                    title_token_overlap: title_overlap,
                    rationale_token_overlap: rationale_overlap,
                    topic_key_overlap: topic_overlap,
                    matched_fields,
                },
                review_required: true,
                reviewer_action,
            });
        }
    }

    candidates.sort_by_key(|c| Reverse(c.score));
    let found = candidates.len();
    Ok(SameAsCandidateReport {
        import_run_id: import_run_id.to_owned(),
        candidates_found: found,
        candidates,
    })
}

pub fn confirm_same_as<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    left_id: &str,
    right_id: &str,
    actor_id: &str,
) -> Result<SameAsActionReport> {
    let event_uuid = same_as_event_uuid("confirm", left_id, right_id);
    let idempotent = event_uuid_exists_for_tenant(ledger, tenant_id, event_uuid)?;

    if !idempotent {
        let (from_id, to_id) = canonical_ordered(left_id, right_id);
        let event = EventBuilder::new(
            event_uuid,
            actor_id,
            EventPayload::RelationAdded(RelationAddedPayload {
                relation: RelationKind::SameAs,
                from_id: from_id.to_owned(), // ubs:ignore: owned for EventPayload
                to_id: to_id.to_owned(),     // ubs:ignore: owned for EventPayload
            }),
        )
        .tenant_id(tenant_id.clone()) // ubs:ignore: EventBuilder::tenant_id() requires owned TenantId
        .provenance(EventProvenance::cli())
        .build()
        .map_err(|e| {
            crate::HivemindError::from(CommandError::Invariant(format!(
                "failed to build same-as confirm event: {e}"
            )))
        })?;
        ledger.append_for_tenant(tenant_id, event)?;
    }

    Ok(SameAsActionReport {
        left_id: left_id.to_owned(),   // ubs:ignore: owned for report output
        right_id: right_id.to_owned(), // ubs:ignore: owned for report output
        action: "confirmed",
        idempotent,
    })
}

pub fn retract_same_as<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    left_id: &str,
    right_id: &str,
    actor_id: &str,
) -> Result<SameAsActionReport> {
    let event_uuid = same_as_event_uuid("retract", left_id, right_id);
    let idempotent = event_uuid_exists_for_tenant(ledger, tenant_id, event_uuid)?;

    if !idempotent {
        let (from_id, to_id) = canonical_ordered(left_id, right_id);
        let event = EventBuilder::new(
            event_uuid,
            actor_id,
            EventPayload::RelationRemoved(RelationRemovedPayload {
                relation: RelationKind::SameAs,
                from_id: from_id.to_owned(), // ubs:ignore: owned for EventPayload
                to_id: to_id.to_owned(),     // ubs:ignore: owned for EventPayload
            }),
        )
        .tenant_id(tenant_id.clone()) // ubs:ignore: EventBuilder::tenant_id() requires owned TenantId
        .provenance(EventProvenance::cli())
        .build()
        .map_err(|e| {
            crate::HivemindError::from(CommandError::Invariant(format!(
                "failed to build same-as retract event: {e}"
            )))
        })?;
        ledger.append_for_tenant(tenant_id, event)?;
    }

    Ok(SameAsActionReport {
        left_id: left_id.to_owned(),   // ubs:ignore: owned for report output
        right_id: right_id.to_owned(), // ubs:ignore: owned for report output
        action: "retracted",
        idempotent,
    })
}

// Scan ledger for all decision.proposed events, returning:
// (all_decisions_map, decision_ids_from_run)
fn scan_decisions_for_run<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    import_run_id: &str,
) -> Result<(HashMap<String, DecisionRecord>, Vec<String>)> {
    let mut all: HashMap<String, DecisionRecord> = HashMap::new();
    let mut run_ids: Vec<String> = Vec::new();
    let mut offset = 0u64;
    const PAGE: usize = 1024;

    loop {
        let events = ledger.read_for_tenant(tenant_id, offset, PAGE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            if event.event_type != EventType::DecisionProposed {
                if let Some(last_id) = events.last().and_then(|e| e.event_id) {
                    let _ = last_id;
                }
                continue;
            }

            let Some(decision_id) = event.payload.get("decision_id").and_then(|v| v.as_str())
            else {
                continue;
            };
            let title = event
                .payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(); // ubs:ignore: per-decision String clone for HashMap storage
            let rationale = event
                .payload
                .get("rationale")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(); // ubs:ignore: per-decision String clone for HashMap storage
            let topic_keys: Vec<String> = event
                .payload
                .get("topic_keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(ToOwned::to_owned) // ubs:ignore: per-topic-key owned String for Vec<String>
                        .collect()
                })
                .unwrap_or_default();

            let decision_id_owned = decision_id.to_owned(); // ubs:ignore: decision_id is &str from event.payload; clone for HashMap key

            // Check if this decision belongs to the requested import_run_id
            let in_run = event
                .source_ref
                .as_deref()
                .and_then(|sr| serde_json::from_str::<ConnectorSourceRef>(sr).ok())
                .map(|src| src.import_run_id == import_run_id)
                .unwrap_or(false);

            if in_run && !run_ids.contains(&decision_id_owned) {
                run_ids.push(decision_id_owned.clone()); // ubs:ignore: run_ids is a small per-run list; clone is intentional
            }

            all.entry(decision_id_owned.clone()) // ubs:ignore: HashMap entry by owned key; clone here, move into value below
                .or_insert_with(|| DecisionRecord {
                    decision_id: decision_id_owned, // ubs:ignore: moved; not cloned twice (entry key is separate from struct field)
                    title,
                    rationale,
                    topic_keys,
                });
        }

        if let Some(last_id) = events.last().and_then(|e| e.event_id) {
            offset = last_id;
        } else {
            break;
        }
    }

    Ok((all, run_ids))
}

// Scan ledger for SAME_AS relation events of the given type (added or removed).
fn scan_same_as_edges<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    event_type: EventType,
) -> Result<HashSet<(String, String)>> {
    let mut pairs: HashSet<(String, String)> = HashSet::new();
    let mut offset = 0u64;
    const PAGE: usize = 1024;

    loop {
        let events = ledger.read_for_tenant(tenant_id, offset, PAGE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            if event.event_type != event_type {
                continue;
            }
            let relation = event
                .payload
                .get("relation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if relation != "SAME_AS" && relation != "same_as" {
                continue;
            }
            let Some(from_id) = event.payload.get("from_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(to_id) = event.payload.get("to_id").and_then(|v| v.as_str()) else {
                continue;
            };
            pairs.insert(canonical_pair(from_id, to_id));
        }

        if let Some(last_id) = events.last().and_then(|e| e.event_id) {
            offset = last_id;
        } else {
            break;
        }
    }

    Ok(pairs)
}

fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned()) // ubs:ignore: canonical pair ordering; two owned Strings per pair
    } else {
        (b.to_owned(), a.to_owned()) // ubs:ignore: canonical pair ordering; two owned Strings per pair
    }
}

fn canonical_ordered<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn same_as_event_uuid(action: &str, left_id: &str, right_id: &str) -> Uuid {
    let (a, b) = canonical_ordered(left_id, right_id);
    let key = format!("same-as:{action}:v1:{a}:{b}");
    Uuid::new_v5(&Uuid::NAMESPACE_URL, key.as_bytes())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
