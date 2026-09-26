//! Minimal Linear GraphQL client for creating issues from quality-scan results.
//!
//! All configuration (API key, team ID) comes from environment variables or
//! CLI arguments — no secrets are hard-coded or stored in the ledger.
//!
//! Linear's API uses a personal or workspace API key, passed via the
//! `Authorization` header (not `Bearer`).  The endpoint is always
//! `https://api.linear.app/graphql`.
//!
//! # Layer discipline
//! This is an **output connector** used exclusively by the `quality-scan` CLI
//! command (layer-3 adjacent: it sends derived signals outward to a human
//! review queue).  It must not be called from layer 1 (ingest) or layer 2
//! (queries).

use std::fmt::Write as _;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::quality_profile::{dimension_markdown, ScanFinding};
use crate::Result;

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct LinearClient {
    client: Client,
    api_key: String,
}

impl LinearClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
        }
    }

    /// Create a Linear issue and return its identifier (e.g. `ENG-123`) and URL.
    pub fn create_issue(
        &self,
        team_id: &str,
        title: &str,
        description: &str,
    ) -> Result<CreatedIssue> {
        let body = GraphQLRequest {
            query: ISSUE_CREATE_MUTATION.to_owned(),
            variables: serde_json::json!({
                "teamId": team_id,
                "title": title,
                "description": description,
            }),
        };

        let resp = self
            .client
            .post(LINEAR_API_URL)
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| CliError::InvalidInput(format!("Linear API request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(
                CliError::InvalidInput(format!("Linear API returned {status}: {text}")).into(),
            );
        }

        let gql: GraphQLResponse = resp
            .json()
            .map_err(|e| CliError::InvalidInput(format!("Linear API response parse error: {e}")))?;

        if let Some(errors) = gql.errors {
            let msg = errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; "); // ubs:ignore: one-shot error-message join; no simpler idiom
            return Err(CliError::InvalidInput(format!("Linear API errors: {msg}")).into());
        }

        let data = gql
            .data
            .ok_or_else(|| CliError::InvalidInput("Linear API returned no data".to_owned()))?;

        if !data.issue_create.success {
            return Err(CliError::InvalidInput(
                "Linear issueCreate reported success=false".to_owned(),
            )
            .into());
        }

        data.issue_create.issue.ok_or_else(|| {
            CliError::InvalidInput("Linear issueCreate succeeded but returned no issue".to_owned())
                .into()
        })
    }
}

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreatedIssue {
    pub id: String,
    pub identifier: String,
    pub url: String,
}

// ---------------------------------------------------------------------------
// Internal serde types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GraphQLRequest {
    query: String,
    variables: serde_json::Value,
}

#[derive(Deserialize)]
struct GraphQLResponse {
    data: Option<IssueCreateResponseData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueCreateResponseData {
    issue_create: IssueCreatePayload,
}

#[derive(Deserialize)]
struct IssueCreatePayload {
    success: bool,
    issue: Option<CreatedIssue>,
}

#[derive(Deserialize)]
struct GraphQLError {
    message: String,
}

impl<'de> Deserialize<'de> for CreatedIssue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            id: String,
            identifier: String,
            url: String,
        }
        let r = Raw::deserialize(d)?;
        Ok(CreatedIssue {
            id: r.id,
            identifier: r.identifier,
            url: r.url,
        })
    }
}

// ---------------------------------------------------------------------------
// GraphQL mutation (constant — avoids string formatting at call site)
// ---------------------------------------------------------------------------

const ISSUE_CREATE_MUTATION: &str = r#"
mutation IssueCreate($teamId: String!, $title: String!, $description: String) {
  issueCreate(input: { teamId: $teamId, title: $title, description: $description }) {
    success
    issue {
      id
      identifier
      url
    }
  }
}
"#;

// ---------------------------------------------------------------------------
// Issue formatting helpers (public for tests and CLI)
// ---------------------------------------------------------------------------

/// Format an attention finding as a Linear issue title: the kind of finding and the decision.
/// Kept short (≤ 140 chars) because Linear truncates long titles.
pub fn format_issue_title(scanned: &ScanFinding, decision_title: Option<&str>) -> String {
    let kind = scanned.finding.kind.as_str();
    match decision_title.filter(|t| !t.is_empty()) {
        Some(title) => {
            let truncated = if title.chars().count() > 80 {
                format!("{}…", title.chars().take(79).collect::<String>())
            } else {
                title.to_owned()
            };
            format!("[HiveMind] {kind}: {truncated}")
        }
        None => format!(
            "[HiveMind] {kind}: decision {}",
            scanned.finding.decision_id
        ),
    }
}

/// Format an attention finding as a Linear issue description (Markdown): the decision and the
/// finding with its id, why it needs a look, the nodes it rests on, and the dimensions of the
/// decision it bears on, each with its level, reasons and ids (or why it was not assessed).
/// Nothing in it grades the decision.
pub fn format_issue_description(scanned: &ScanFinding, hivemind_base_url: Option<&str>) -> String {
    let finding = &scanned.finding;
    let mut out = String::new();

    let _ = write!(
        out,
        "## Decision needs a look\n\n**Decision ID:** `{}`  \n**Finding:** {}  \n**Finding ID:** `{}`\n\n",
        finding.decision_id,
        finding.kind.as_str(),
        finding.finding_id,
    );

    if let Some(base) = hivemind_base_url {
        let base = base.trim_end_matches('/');
        let _ = writeln!(out, "**Link:** {base}/decisions/{}\n", finding.decision_id);
    }

    let _ = write!(out, "### Why it needs a look\n\n{}\n\n", finding.reason);

    if !finding.node_ids.is_empty() {
        out.push_str("### Node IDs\n\n");
        for id in &finding.node_ids {
            let _ = writeln!(out, "- `{id}`");
        }
        out.push('\n');
    }

    if !scanned.dimensions.is_empty() {
        out.push_str("### Dimensions it bears on\n\n");
        for line in &scanned.dimensions {
            out.push_str(&dimension_markdown(line.dimension, &line.assessment));
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("---\n*Filed by HiveMind quality-scan. Human review required — HiveMind never auto-acts on decisions.*\n");

    out
}

#[cfg(test)]
mod tests;
