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

/// Format a quality-scan result as a Linear issue title.
/// Kept short (≤ 140 chars) because Linear truncates long titles.
pub fn format_issue_title(decision_id: &str, tier: &str, decision_title: Option<&str>) -> String {
    match decision_title.filter(|t| !t.is_empty()) {
        Some(title) => {
            let truncated = if title.chars().count() > 80 {
                format!("{}…", title.chars().take(79).collect::<String>())
            } else {
                title.to_owned()
            };
            format!("[HiveMind] {tier}: {truncated}")
        }
        None => format!("[HiveMind] {tier}: decision {decision_id}"),
    }
}

/// Format a quality-scan result as a Linear issue description (Markdown).
pub fn format_issue_description(
    decision_id: &str,
    score: f64,
    tier: &str,
    reasons: &[String],
    contributing_ids: &[String],
    hivemind_base_url: Option<&str>,
) -> String {
    let mut out = String::new();

    let _ = write!(out, "## Decision quality concern\n\n**Decision ID:** `{decision_id}`  \n**Score:** {score:.3}  \n**Tier:** {tier}\n\n");

    if let Some(base) = hivemind_base_url {
        let base = base.trim_end_matches('/');
        let _ = writeln!(out, "**Link:** {base}/decisions/{decision_id}\n");
    }

    if !reasons.is_empty() {
        out.push_str("### Reasons\n\n");
        for reason in reasons {
            let _ = writeln!(out, "- {reason}");
        }
        out.push('\n');
    }

    if !contributing_ids.is_empty() {
        out.push_str("### Contributing node IDs\n\n");
        for id in contributing_ids {
            let _ = writeln!(out, "- `{id}`");
        }
        out.push('\n');
    }

    out.push_str("---\n*Filed by HiveMind quality-scan. Human review required — HiveMind never auto-acts on decisions.*\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_title_with_title() {
        let t = format_issue_title("d-001", "high_concern", Some("Deploy to prod"));
        assert!(t.starts_with("[HiveMind]")); // ubs:ignore: test assertion
        assert!(t.contains("Deploy to prod")); // ubs:ignore: test assertion
        assert!(t.len() <= 140); // ubs:ignore: test assertion
    }

    #[test]
    fn format_title_without_title() {
        let t = format_issue_title("d-001", "high_concern", None);
        assert!(t.contains("d-001")); // ubs:ignore: test assertion
    }

    #[test]
    fn format_title_truncates_long_titles() {
        let long = "A".repeat(100);
        let t = format_issue_title("d-001", "high_concern", Some(&long));
        assert!(t.len() <= 140); // ubs:ignore: test assertion
        assert!(t.contains('…')); // ubs:ignore: test assertion
    }

    #[test]
    fn format_description_contains_required_fields() {
        let desc = format_issue_description(
            "d-001",
            0.3,
            "high_concern",
            &["reason A".to_owned()],
            &["h-001".to_owned()],
            Some("https://hivemind.example.com"),
        );
        assert!(desc.contains("d-001")); // ubs:ignore: test assertion
        assert!(desc.contains("0.300")); // ubs:ignore: test assertion
        assert!(desc.contains("reason A")); // ubs:ignore: test assertion
        assert!(desc.contains("h-001")); // ubs:ignore: test assertion
        assert!(desc.contains("hivemind.example.com")); // ubs:ignore: test assertion
        assert!(desc.contains("Human review required")); // ubs:ignore: test assertion
    }
}
