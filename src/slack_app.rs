use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{CliError, CommandError};
use crate::events::EventType;
use crate::ingest::{import_slack_thread, SlackDecisionDraft, SlackIngestOutcome};
use crate::ledger::EventLedger;
use crate::projector::{GraphView, RelationKind};
use crate::queries::{
    get_decision, get_decision_neighborhood, search_decisions, DecisionSearchResults,
    DecisionStatus, NeighborhoodRequest, SearchDecisionRequest,
};
use crate::Result;

const APP_DIR: &str = "slack-app";
const INSTALLS_FILE: &str = "installations.json";
const QUEUE_FILE: &str = "queue.jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackWorkspaceInstall {
    pub team_id: String,
    pub team_name: String,
    pub bot_token: String,
    pub signing_secret: String,
    pub hivemind_url: String,
    pub reaction_emoji: String,
    #[serde(default)]
    pub actor_mappings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackInstallSummary {
    pub team_id: String,
    pub team_name: String,
    pub hivemind_url: String,
    pub reaction_emoji: String,
    pub bot_token_stored: bool,
    pub signing_secret_stored: bool,
}

#[derive(Debug, Clone)]
pub struct SlackAppStore {
    hivemind_dir: PathBuf,
}

impl SlackAppStore {
    pub fn new(hivemind_dir: impl Into<PathBuf>) -> Self {
        Self {
            hivemind_dir: hivemind_dir.into(),
        }
    }

    pub fn app_dir(&self) -> PathBuf {
        self.hivemind_dir.join(APP_DIR)
    }

    fn installs_path(&self) -> PathBuf {
        self.app_dir().join(INSTALLS_FILE)
    }

    fn queue_path(&self) -> PathBuf {
        self.app_dir().join(QUEUE_FILE)
    }

    pub fn install_workspace(&self, install: SlackWorkspaceInstall) -> Result<SlackInstallSummary> {
        validate_install(&install)?;
        let mut installs = self.load_installs()?;
        installs.retain(|existing| existing.team_id != install.team_id);
        installs.push(install.clone());
        installs.sort_by(|left, right| left.team_id.cmp(&right.team_id));
        self.save_installs(&installs)?;

        Ok(SlackInstallSummary {
            team_id: install.team_id,
            team_name: install.team_name,
            hivemind_url: install.hivemind_url,
            reaction_emoji: install.reaction_emoji,
            bot_token_stored: true,
            signing_secret_stored: true,
        })
    }

    pub fn load_installs(&self) -> Result<Vec<SlackWorkspaceInstall>> {
        let path = self.installs_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let input = fs::read_to_string(&path).map_err(|error| {
            CliError::InvalidInput(format!(
                "could not read Slack installations {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_str(&input).map_err(|error| {
            CliError::InvalidInput(format!(
                "invalid Slack installations file {}: {error}",
                path.display()
            ))
            .into()
        })
    }

    fn save_installs(&self, installs: &[SlackWorkspaceInstall]) -> Result<()> {
        fs::create_dir_all(self.app_dir()).map_err(|error| {
            CliError::InvalidInput(format!(
                "could not create Slack app directory {}: {error}",
                self.app_dir().display()
            ))
        })?;
        let output = serde_json::to_string_pretty(installs).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}"))
        })?;
        write_secret_file(&self.installs_path(), output.as_bytes())
    }

    pub fn installation(&self, team_id: &str) -> Result<SlackWorkspaceInstall> {
        let team_id = non_empty("team_id", team_id)?;
        self.load_installs()?
            .into_iter()
            .find(|install| install.team_id == team_id)
            .ok_or_else(|| {
                CommandError::Validation(format!(
                    "Slack workspace {team_id} is not installed; run slack-app install first"
                ))
                .into()
            })
    }

    pub fn enqueue_capture(&self, capture: SlackCaptureRequest) -> Result<QueuedSlackEvent> {
        validate_capture(&capture)?;
        self.installation(&capture.team_id)?;

        fs::create_dir_all(self.app_dir()).map_err(|error| {
            CliError::InvalidInput(format!(
                "could not create Slack app directory {}: {error}",
                self.app_dir().display()
            ))
        })?;
        let event = QueuedSlackEvent {
            id: Uuid::new_v4().to_string(),
            enqueued_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            attempts: 0,
            last_error: None,
            capture,
        };
        let line = serde_json::to_string(&event).map_err(|error| {
            CliError::InvalidInput(format!("json serialization failed: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.queue_path())
            .map_err(|error| {
                CliError::InvalidInput(format!(
                    "could not open Slack queue {}: {error}",
                    self.queue_path().display()
                ))
            })?;
        writeln!(file, "{line}").map_err(|error| {
            CliError::InvalidInput(format!(
                "could not append Slack queue {}: {error}",
                self.queue_path().display()
            ))
        })?;
        Ok(event)
    }

    pub fn drain_queue<L: EventLedger>(&self, ledger: &L) -> Result<SlackDrainReport> {
        let mut events = self.load_queue()?;
        let total = events.len();
        let mut remaining = Vec::new();
        let mut processed = Vec::new();
        let mut failed = Vec::new();

        for mut event in events.drain(..) {
            match process_capture(ledger, self, &event.capture) {
                Ok(outcome) => {
                    processed.push(SlackProcessedEvent {
                        queue_id: event.id,
                        decision_id: outcome.decision_id().to_owned(),
                        already_imported: matches!(
                            outcome,
                            SlackIngestOutcome::AlreadyImported { .. }
                        ),
                    });
                }
                Err(error) => {
                    event.attempts = event.attempts.saturating_add(1);
                    event.last_error = Some(error.to_string());
                    failed.push(SlackFailedEvent {
                        queue_id: event.id.clone(),
                        attempts: event.attempts,
                        error: error.to_string(),
                    });
                    remaining.push(event);
                }
            }
        }

        self.save_queue(&remaining)?;
        Ok(SlackDrainReport {
            queued_before: total,
            processed_count: processed.len(),
            failed_count: failed.len(),
            queued_after: remaining.len(),
            processed,
            failed,
        })
    }

    fn load_queue(&self) -> Result<Vec<QueuedSlackEvent>> {
        let path = self.queue_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let input = fs::read_to_string(&path).map_err(|error| {
            CliError::InvalidInput(format!(
                "could not read Slack queue {}: {error}",
                path.display()
            ))
        })?;
        input
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).map_err(|error| {
                    CliError::InvalidInput(format!(
                        "invalid Slack queue item in {}: {error}",
                        path.display()
                    ))
                    .into()
                })
            })
            .collect()
    }

    fn save_queue(&self, events: &[QueuedSlackEvent]) -> Result<()> {
        fs::create_dir_all(self.app_dir()).map_err(|error| {
            CliError::InvalidInput(format!(
                "could not create Slack app directory {}: {error}",
                self.app_dir().display()
            ))
        })?;
        let mut output = String::new();
        for event in events {
            let line = serde_json::to_string(event).map_err(|error| {
                CliError::InvalidInput(format!("json serialization failed: {error}"))
            })?;
            output.push_str(&line);
            output.push('\n');
        }
        write_regular_file(&self.queue_path(), output.as_bytes())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackCaptureSurface {
    SlashCommand,
    MessageAction,
    Reaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackCaptureRequest {
    pub team_id: String,
    pub user_id: String,
    pub channel_id: String,
    pub message_ts: String,
    pub thread_ts: String,
    pub permalink: String,
    pub surface: SlackCaptureSurface,
    #[serde(default)]
    pub reaction_emoji: Option<String>,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub option_labels: Vec<String>,
    pub chosen_option_label: Option<String>,
    pub thread_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedSlackEvent {
    pub id: String,
    pub enqueued_at: String,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub capture: SlackCaptureRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackDrainReport {
    pub queued_before: usize,
    pub processed_count: usize,
    pub failed_count: usize,
    pub queued_after: usize,
    pub processed: Vec<SlackProcessedEvent>,
    pub failed: Vec<SlackFailedEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackProcessedEvent {
    pub queue_id: String,
    pub decision_id: String,
    pub already_imported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackFailedEvent {
    pub queue_id: String,
    pub attempts: u32,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackCommandRequest {
    pub team_id: String,
    pub user_id: String,
    pub text: String,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlackAppResponse {
    pub response_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modal: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlackCitation {
    pub decision_id: String,
    pub event_id: u64,
    pub actor_id: String,
    pub source: String,
    pub source_ref: Option<String>,
}

pub fn slack_app_manifest(
    request_url: &str,
    event_request_url: Option<&str>,
    redirect_url: Option<&str>,
) -> Result<Value> {
    let request_url = non_empty("request_url", request_url)?;
    let event_request_url = event_request_url.unwrap_or(request_url).trim();
    if event_request_url.is_empty() {
        return Err(
            CommandError::Validation("event_request_url must not be empty".to_owned()).into(),
        );
    }
    let redirect_urls = redirect_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_owned()])
        .unwrap_or_default();

    Ok(json!({
        "display_information": {
            "name": "HiveMind",
            "description": "Capture and query organizational decision memory",
            "background_color": "#1f2933"
        },
        "features": {
            "bot_user": {
                "display_name": "HiveMind",
                "always_online": false
            },
            "slash_commands": [{
                "command": "/hivemind",
                "url": request_url,
                "description": "Capture or query HiveMind decisions",
                "usage_hint": "capture | query <topic> | show <decision-id>",
                "should_escape": false
            }],
            "shortcuts": [{
                "name": "Capture this thread as a decision",
                "type": "message",
                "callback_id": "hivemind_capture_thread",
                "description": "Preserve a Slack thread permalink as HiveMind evidence"
            }]
        },
        "oauth_config": {
            "redirect_urls": redirect_urls,
            "scopes": {
                "bot": [
                    "commands",
                    "chat:write",
                    "reactions:read",
                    "channels:history",
                    "groups:history",
                    "links:read"
                ]
            }
        },
        "settings": {
            "interactivity": {
                "is_enabled": true,
                "request_url": request_url
            },
            "event_subscriptions": {
                "request_url": event_request_url,
                "bot_events": ["reaction_added"]
            },
            "org_deploy_enabled": false,
            "socket_mode_enabled": false,
            "token_rotation_enabled": false
        }
    }))
}

pub fn slack_oauth_install_url(client_id: &str, redirect_uri: &str, state: &str) -> Result<String> {
    let client_id = non_empty("client_id", client_id)?;
    let redirect_uri = non_empty("redirect_uri", redirect_uri)?;
    let state = non_empty("state", state)?;
    let scopes = "commands,chat:write,reactions:read,channels:history,groups:history,links:read";
    Ok(format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope={}&redirect_uri={}&state={}",
        percent_encode(client_id),
        percent_encode(scopes),
        percent_encode(redirect_uri),
        percent_encode(state)
    ))
}

pub fn process_capture<L: EventLedger>(
    ledger: &L,
    store: &SlackAppStore,
    capture: &SlackCaptureRequest,
) -> Result<SlackIngestOutcome> {
    validate_capture(capture)?;
    let install = store.installation(&capture.team_id)?;
    validate_reaction_trigger(&install, capture)?;
    let draft = capture_to_draft(&install, capture);
    import_slack_thread(ledger, &draft)
}

pub fn handle_slack_command<L: EventLedger, G: GraphView>(
    ledger: &L,
    graph: &G,
    store: &SlackAppStore,
    request: &SlackCommandRequest,
) -> Result<SlackAppResponse> {
    let install = store.installation(&request.team_id)?;
    non_empty("user_id", &request.user_id)?;
    let text = request.text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("capture") {
        return Ok(capture_modal_response(&install, &request.user_id));
    }

    let Some((command, rest)) = text.split_once(char::is_whitespace) else {
        return unknown_command_response(text);
    };
    let rest = rest.trim();
    match command.to_ascii_lowercase().as_str() {
        "query" => render_query_response(ledger, graph, rest, request.limit),
        "show" => render_show_response(ledger, graph, rest),
        _ => unknown_command_response(text),
    }
}

fn render_query_response<L: EventLedger, G: GraphView>(
    ledger: &L,
    graph: &G,
    query: &str,
    limit: usize,
) -> Result<SlackAppResponse> {
    let query = non_empty("query", query)?;
    let response = search_decisions(
        graph,
        &SearchDecisionRequest {
            query: Some(query.to_owned()),
            limit: limit.clamp(1, 10),
            ..SearchDecisionRequest::default()
        },
    )?;
    let citations = citations_for_search(ledger, &response)?;
    let mut blocks = Vec::new();
    for item in &response.data.items {
        let decision = &item.decision;
        let citation = citations.get(&decision.id);
        blocks.push(json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!(
                    "*{}* `{}`\nstatus={} citation={}",
                    decision.title,
                    decision.id,
                    status_name(decision.status),
                    citation_label(citation)
                )
            }
        }));
    }

    let text = if response.data.items.is_empty() {
        format!("No HiveMind decisions matched `{query}`.")
    } else {
        format!(
            "HiveMind found {} decision(s) for `{query}`.",
            response.data.items.len()
        )
    };

    Ok(SlackAppResponse {
        response_type: "ephemeral".to_owned(),
        text,
        blocks,
        action: None,
        modal: None,
    })
}

fn render_show_response<L: EventLedger, G: GraphView>(
    ledger: &L,
    graph: &G,
    decision_id: &str,
) -> Result<SlackAppResponse> {
    let decision_id = non_empty("decision_id", decision_id)?;
    let decision = get_decision(graph, decision_id)?
        .data
        .ok_or_else(|| CommandError::Validation(format!("decision not found: {decision_id}")))?;
    let neighborhood = get_decision_neighborhood(
        graph,
        decision_id,
        &NeighborhoodRequest::with_relations([
            RelationKind::BasedOn,
            RelationKind::HasOption,
            RelationKind::Chose,
            RelationKind::Assumes,
        ]),
    )?
    .data;
    let citations = decision_citations(ledger, [decision.id.clone()].into_iter().collect())?;
    let citation = citations.get(&decision.id);
    let related = neighborhood
        .nodes
        .iter()
        .filter(|node| node.id != decision.id)
        .map(|node| format!("{}:{:?}", node.id, node.kind))
        .collect::<Vec<_>>();
    let related = if related.is_empty() {
        "none".to_owned()
    } else {
        related.join(", ")
    };

    Ok(SlackAppResponse {
        response_type: "ephemeral".to_owned(),
        text: format!("HiveMind decision `{}`", decision.id),
        blocks: vec![json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!(
                    "*{}* `{}`\nstatus={}\nrationale={}\noptions={}\nevidence={}\nrelated={}\ncitation={}",
                    decision.title,
                    decision.id,
                    status_name(decision.status),
                    decision.rationale,
                    display_list(&decision.option_ids),
                    display_list(&decision.evidence_ids),
                    related,
                    citation_label(citation)
                )
            }
        })],
        action: None,
        modal: None,
    })
}

fn capture_modal_response(install: &SlackWorkspaceInstall, user_id: &str) -> SlackAppResponse {
    SlackAppResponse {
        response_type: "ephemeral".to_owned(),
        text: "Opening HiveMind capture modal.".to_owned(),
        blocks: Vec::new(),
        action: Some("open_modal".to_owned()),
        modal: Some(json!({
            "type": "modal",
            "callback_id": "hivemind_capture_decision",
            "title": {"type": "plain_text", "text": "HiveMind"},
            "submit": {"type": "plain_text", "text": "Capture"},
            "close": {"type": "plain_text", "text": "Cancel"},
            "private_metadata": {
                "team_id": install.team_id,
                "actor_id": slack_actor_id(install, user_id)
            },
            "blocks": [
                input_block("title", "Decision", "plain_text_input"),
                input_block("rationale", "Rationale", "plain_text_input"),
                input_block("topics", "Topics", "plain_text_input"),
                input_block("options", "Options", "plain_text_input")
            ]
        })),
    }
}

fn unknown_command_response(text: &str) -> Result<SlackAppResponse> {
    Ok(SlackAppResponse {
        response_type: "ephemeral".to_owned(),
        text: format!(
            "Unknown HiveMind command `{text}`. Use `capture`, `query <topic>`, or `show <id>`."
        ),
        blocks: Vec::new(),
        action: None,
        modal: None,
    })
}

fn input_block(block_id: &str, label: &str, element_type: &str) -> Value {
    json!({
        "type": "input",
        "block_id": block_id,
        "label": {"type": "plain_text", "text": label},
        "element": {"type": element_type, "action_id": "value"}
    })
}

fn capture_to_draft(
    install: &SlackWorkspaceInstall,
    capture: &SlackCaptureRequest,
) -> SlackDecisionDraft {
    let actor_id = slack_actor_id(install, &capture.user_id);
    SlackDecisionDraft {
        actor_id,
        source_ref: capture.permalink.clone(),
        title: capture.title.clone(),
        rationale: capture.rationale.clone(),
        topic_keys: capture.topic_keys.clone(),
        option_labels: capture.option_labels.clone(),
        chosen_option_label: capture.chosen_option_label.clone(),
        thread_context: render_capture_evidence(capture),
    }
}

fn render_capture_evidence(capture: &SlackCaptureRequest) -> String {
    let reaction = capture
        .reaction_emoji
        .as_deref()
        .map(|emoji| format!("\nreaction_emoji: {emoji}"))
        .unwrap_or_default();
    format!(
        "Slack {} capture\npermalink: {}\nteam: {}\nchannel: {}\nmessage_ts: {}\nthread_ts: {}{}\ntext:\n{}",
        surface_name(capture.surface),
        capture.permalink,
        capture.team_id,
        capture.channel_id,
        capture.message_ts,
        capture.thread_ts,
        reaction,
        capture.thread_text.trim()
    )
}

fn citations_for_search<L: EventLedger>(
    ledger: &L,
    response: &crate::queries::QueryResponse<DecisionSearchResults>,
) -> Result<BTreeMap<String, SlackCitation>> {
    let ids = response
        .data
        .items
        .iter()
        .map(|item| item.decision.id.clone())
        .collect();
    decision_citations(ledger, ids)
}

fn decision_citations<L: EventLedger>(
    ledger: &L,
    decision_ids: BTreeSet<String>,
) -> Result<BTreeMap<String, SlackCitation>> {
    let mut citations = BTreeMap::new();
    if decision_ids.is_empty() {
        return Ok(citations);
    }

    let mut offset = 0;
    loop {
        let events = ledger.read(offset, 1024)?;
        if events.is_empty() {
            return Ok(citations);
        }
        for event in &events {
            if event.event_type != EventType::DecisionProposed {
                continue;
            }
            let Some(decision_id) = event
                .payload
                .get("decision_id")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            if !decision_ids.contains(decision_id) {
                continue;
            }
            if let Some(event_id) = event.event_id {
                citations.insert(
                    decision_id.to_owned(),
                    SlackCitation {
                        decision_id: decision_id.to_owned(),
                        event_id,
                        actor_id: event.actor_id.clone(),
                        source: event.source.as_str().to_owned(),
                        source_ref: event.source_ref.clone(),
                    },
                );
            }
        }
        match events.last().and_then(|event| event.event_id) {
            Some(last) => offset = last,
            None => return Ok(citations),
        }
    }
}

fn citation_label(citation: Option<&SlackCitation>) -> String {
    citation
        .map(|citation| {
            citation
                .source_ref
                .as_ref()
                .map(|source_ref| format!("event:{} {}", citation.event_id, source_ref))
                .unwrap_or_else(|| {
                    format!("event:{} source={}", citation.event_id, citation.source)
                })
        })
        .unwrap_or_else(|| "missing".to_owned())
}

fn slack_actor_id(install: &SlackWorkspaceInstall, user_id: &str) -> String {
    install
        .actor_mappings
        .get(user_id)
        .cloned()
        .unwrap_or_else(|| format!("slack:{}:{user_id}", install.team_id))
}

fn validate_install(install: &SlackWorkspaceInstall) -> Result<()> {
    non_empty("team_id", &install.team_id)?;
    non_empty("team_name", &install.team_name)?;
    non_empty("bot_token", &install.bot_token)?;
    non_empty("signing_secret", &install.signing_secret)?;
    non_empty("hivemind_url", &install.hivemind_url)?;
    non_empty("reaction_emoji", &install.reaction_emoji)?;
    for (slack_user, actor_id) in &install.actor_mappings {
        non_empty("actor mapping Slack user", slack_user)?;
        non_empty("actor mapping actor_id", actor_id)?;
    }
    Ok(())
}

fn validate_capture(capture: &SlackCaptureRequest) -> Result<()> {
    non_empty("team_id", &capture.team_id)?;
    non_empty("user_id", &capture.user_id)?;
    non_empty("channel_id", &capture.channel_id)?;
    non_empty("message_ts", &capture.message_ts)?;
    non_empty("thread_ts", &capture.thread_ts)?;
    non_empty("permalink", &capture.permalink)?;
    non_empty("title", &capture.title)?;
    non_empty("rationale", &capture.rationale)?;
    if capture
        .topic_keys
        .iter()
        .all(|topic| topic.trim().is_empty())
    {
        return Err(CommandError::Validation("topic_keys must not be empty".to_owned()).into());
    }
    if capture
        .option_labels
        .iter()
        .all(|option| option.trim().is_empty())
    {
        return Err(CommandError::Validation("option_labels must not be empty".to_owned()).into());
    }
    if let Some(chosen) = capture.chosen_option_label.as_deref() {
        non_empty("chosen_option_label", chosen)?;
        if !capture.option_labels.iter().any(|option| option == chosen) {
            return Err(CommandError::Validation(
                "chosen_option_label must match one option label".to_owned(),
            )
            .into());
        }
    }
    Ok(())
}

fn validate_reaction_trigger(
    install: &SlackWorkspaceInstall,
    capture: &SlackCaptureRequest,
) -> Result<()> {
    if capture.surface != SlackCaptureSurface::Reaction {
        return Ok(());
    }
    let reaction = capture
        .reaction_emoji
        .as_deref()
        .ok_or_else(|| {
            CommandError::Validation("reaction captures must include reaction_emoji".to_owned())
        })?
        .trim();
    if reaction != install.reaction_emoji {
        return Err(CommandError::Validation(format!(
            "reaction '{reaction}' does not match configured trigger '{}'",
            install.reaction_emoji
        ))
        .into());
    }
    Ok(())
}

fn non_empty<'a>(field: &'static str, value: &'a str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(value)
    }
}

fn display_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

fn status_name(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn surface_name(surface: SlackCaptureSurface) -> &'static str {
    match surface {
        SlackCaptureSurface::SlashCommand => "slash_command",
        SlackCaptureSurface::MessageAction => "message_action",
        SlackCaptureSurface::Reaction => "reaction",
    }
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn write_regular_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|error| {
        CliError::InvalidInput(format!("could not write {}: {error}", tmp.display()))
    })?;
    fs::rename(&tmp, path).map_err(|error| {
        CliError::InvalidInput(format!(
            "could not replace {} with {}: {error}",
            path.display(),
            tmp.display()
        ))
        .into()
    })
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|error| {
        CliError::InvalidInput(format!("could not open {}: {error}", tmp.display()))
    })?;
    file.write_all(bytes).map_err(|error| {
        CliError::InvalidInput(format!("could not write {}: {error}", tmp.display()))
    })?;
    fs::rename(&tmp, path).map_err(|error| {
        CliError::InvalidInput(format!(
            "could not replace {} with {}: {error}",
            path.display(),
            tmp.display()
        ))
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSource;
    use crate::ledger::InMemoryEventLedger;

    #[test]
    fn oauth_url_escapes_query_values() {
        let url = slack_oauth_install_url("123.abc", "https://local/callback", "state with space")
            .expect("url builds");

        assert!(url.contains("client_id=123.abc"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Flocal%2Fcallback"));
        assert!(url.contains("state=state%20with%20space"));
    }

    #[test]
    fn capture_processing_uses_mapping_when_present() {
        let scratch = std::env::temp_dir().join(format!("hivemind-slack-app-{}", Uuid::new_v4()));
        let store = SlackAppStore::new(&scratch);
        store
            .install_workspace(SlackWorkspaceInstall {
                team_id: "T123".to_owned(),
                team_name: "Example".to_owned(),
                bot_token: generated_test_secret("bot"),
                signing_secret: generated_test_secret("signing"),
                hivemind_url: "http://127.0.0.1:8787".to_owned(),
                reaction_emoji: "hivemind".to_owned(),
                actor_mappings: BTreeMap::from([("U111".to_owned(), "actor:alice".to_owned())]),
            })
            .expect("install succeeds");

        let ledger = InMemoryEventLedger::default();
        let outcome = process_capture(&ledger, &store, &capture()).expect("capture succeeds");
        assert!(matches!(outcome, SlackIngestOutcome::Imported { .. }));
        let events = ledger.read(0, 100).expect("events read");
        let proposal = events
            .iter()
            .find(|event| event.event_type == EventType::DecisionProposed)
            .expect("proposal exists");
        assert_eq!(proposal.actor_id, "actor:alice");
        assert_eq!(proposal.source, EventSource::Slack);
        assert_eq!(
            proposal.source_ref.as_deref(),
            Some("https://example.slack.com/archives/C456/p1715970800000100")
        );

        let _ = fs::remove_dir_all(scratch);
    }

    #[test]
    fn reaction_capture_requires_configured_emoji() {
        let scratch = std::env::temp_dir().join(format!("hivemind-slack-app-{}", Uuid::new_v4()));
        let store = SlackAppStore::new(&scratch);
        store
            .install_workspace(SlackWorkspaceInstall {
                team_id: "T123".to_owned(),
                team_name: "Example".to_owned(),
                bot_token: generated_test_secret("bot"),
                signing_secret: generated_test_secret("signing"),
                hivemind_url: "http://127.0.0.1:8787".to_owned(),
                reaction_emoji: "hivemind".to_owned(),
                actor_mappings: BTreeMap::new(),
            })
            .expect("install succeeds");

        let ledger = InMemoryEventLedger::default();
        let mut capture = capture();
        capture.surface = SlackCaptureSurface::Reaction;
        capture.reaction_emoji = Some("eyes".to_owned());

        let error =
            process_capture(&ledger, &store, &capture).expect_err("wrong emoji should be rejected");
        assert!(error.to_string().contains("configured trigger"));

        capture.reaction_emoji = Some("hivemind".to_owned());
        process_capture(&ledger, &store, &capture).expect("configured emoji captures");

        let _ = fs::remove_dir_all(scratch);
    }

    fn capture() -> SlackCaptureRequest {
        SlackCaptureRequest {
            team_id: "T123".to_owned(),
            user_id: "U111".to_owned(),
            channel_id: "C456".to_owned(),
            message_ts: "1715970800.000100".to_owned(),
            thread_ts: "1715970800.000100".to_owned(),
            permalink: "https://example.slack.com/archives/C456/p1715970800000100".to_owned(),
            surface: SlackCaptureSurface::MessageAction,
            reaction_emoji: None,
            title: "Use the local Slack app".to_owned(),
            rationale: "It preserves reviewed human decisions before hosted service work"
                .to_owned(),
            topic_keys: vec!["slack".to_owned(), "integrations".to_owned()],
            option_labels: vec!["local-first".to_owned(), "hosted-first".to_owned()],
            chosen_option_label: Some("local-first".to_owned()),
            thread_text: "Decision reviewed in Slack".to_owned(),
        }
    }

    fn generated_test_secret(prefix: &str) -> String {
        format!("{prefix}-{}", Uuid::new_v4())
    }
}
