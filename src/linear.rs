//! Minimal Linear GraphQL client for creating issues from quality-scan results.
//!
//! All configuration (API key, team ID) comes from environment variables or
//! CLI arguments — no secrets are hard-coded or stored in the ledger.
//!
//! Linear's API uses a personal or workspace API key, passed via the
//! `Authorization` header (not `Bearer`).  The endpoint is always
//! `https://api.linear.app/graphql`.
//!
//! # The reference consumer of `get_suggestions`
//! The connector keeps no list of what it has filed. It asks `get_suggestions` for the findings
//! nobody has acknowledged, files one ticket for each, and acknowledges each finding
//! (`acknowledge_suggestion` semantics: channel `linear`, action `acted`), so the next run is
//! not shown it. A finding whose basis changes has a new `finding_id` and is filed again. This
//! is the pattern for any sink: "where" suggestions go is the consumer's own configuration, and
//! HiveMind records only that a finding was surfaced and what was done about it.
//!
//! # Layer discipline
//! This is an **output connector** used exclusively by the `quality-scan` CLI
//! command (layer-3 adjacent: it sends derived signals outward to a human
//! review queue).  It must not be called from layer 1 (ingest) or layer 2
//! (queries).  It reads through the layer-2 suggestions query and writes only
//! through the ordinary actor-attributed command path.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::commands::Commands;
use crate::error::CliError;
use crate::events::AckAction;
use crate::ledger::EventLedger;
use crate::projector::GraphView;
use crate::quality_profile::{
    dimension_markdown, get_suggestions_at, ScanFinding, ScanReport, ScanRequest,
    SuggestionsRequest,
};
use crate::queries::QueryResponse;
use crate::Result;

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

/// The `channel` the connector records its acknowledgements under: a label the consumer owns.
pub const ACK_CHANNEL: &str = "linear";

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

/// Format an attention finding as a Linear issue title: the kind of finding and the decision's
/// own title. Kept short (≤ 140 chars) because Linear truncates long titles.
pub fn format_issue_title(scanned: &ScanFinding) -> String {
    let kind = scanned.finding.kind.as_str();
    let title = &scanned.finding.decision_title;
    let truncated = if title.chars().count() > 80 {
        format!("{}…", title.chars().take(79).collect::<String>())
    } else {
        title.clone()
    };
    format!("[HiveMind] {kind}: {truncated}")
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
        "## Decision needs a look\n\n**Decision:** {}  \n**Decision ID:** `{}`  \n**Finding:** {}  \n**Finding ID:** `{}`\n\n",
        finding.decision_title,
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

// ---------------------------------------------------------------------------
// Filing what is new
// ---------------------------------------------------------------------------

/// The findings nobody has acknowledged yet, as of `now`: the one read the connector makes.
/// Because every filed finding is acknowledged, a graph that has not changed has none, and a
/// backlog longer than `request.limit` is worked through one page per run.
pub fn pending_findings(
    graph: &impl GraphView,
    request: &ScanRequest,
    now: DateTime<Utc>,
) -> Result<QueryResponse<ScanReport>> {
    get_suggestions_at(
        graph,
        &SuggestionsRequest {
            scan: request.clone(),
            exclude_acknowledged: true,
        },
        now,
    )
}

/// A finding the connector filed as a ticket and then acknowledged.
#[derive(Debug, Clone)]
pub struct FiledFinding {
    pub finding_id: String,
    pub decision_id: String,
    pub kind: &'static str,
    pub issue: CreatedIssue,
}

/// Files one ticket per finding through `create_issue(title, description)` and acknowledges the
/// finding (`acted`, channel [`ACK_CHANNEL`], by `actor_id`) right after its ticket exists, so a
/// run that stops part-way has recorded exactly the tickets it filed.
///
/// The first failure stops the run: later findings stay unacknowledged and are filed by the next
/// run. A ticket whose acknowledgement could not be recorded is named in the error, because the
/// next run would file that finding a second time.
pub fn file_findings<L: EventLedger>(
    findings: &[ScanFinding],
    commands: &Commands<'_, L>,
    actor_id: &str,
    hivemind_base_url: Option<&str>,
    mut create_issue: impl FnMut(&str, &str) -> Result<CreatedIssue>,
) -> Result<Vec<FiledFinding>> {
    let mut filed: Vec<FiledFinding> = Vec::with_capacity(findings.len());
    for scanned in findings {
        let finding = &scanned.finding;
        let issue = create_issue(
            &format_issue_title(scanned),
            &format_issue_description(scanned, hivemind_base_url),
        )
        .map_err(|error| CliError::InvalidInput(format!("{error}{}", filed_so_far(&filed))))?;

        commands
            .acknowledge_suggestion(
                actor_id,
                &finding.finding_id,
                &finding.decision_id,
                AckAction::Acted,
                ACK_CHANNEL,
            )
            .map_err(|error| {
                CliError::InvalidInput(format!(
                    "filed {} ({}) for finding {} but could not record that it was acted on: \
                     {error}. Acknowledge it (acknowledge_suggestion, channel `{ACK_CHANNEL}`, \
                     action `acted`) or the next run files it again{}",
                    issue.identifier,
                    issue.url,
                    finding.finding_id,
                    filed_so_far(&filed),
                ))
            })?;

        filed.push(FiledFinding {
            finding_id: finding.finding_id.clone(),
            decision_id: finding.decision_id.clone(),
            kind: finding.kind.as_str(),
            issue,
        });
    }
    Ok(filed)
}

/// What a failed run had already filed and acknowledged, for its error message.
fn filed_so_far(filed: &[FiledFinding]) -> String {
    if filed.is_empty() {
        return String::new();
    }
    let identifiers: Vec<&str> = filed
        .iter()
        .map(|done| done.issue.identifier.as_str())
        .collect();
    let list = identifiers.join(", "); // ubs:ignore: one-shot error-message join; no simpler idiom
    format!(" (filed and acknowledged earlier in this run: {list})")
}

#[cfg(test)]
mod tests;
