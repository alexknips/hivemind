//! Layer-3 background classifier: reads ingest.batch_received events and
//! annotates them with ingest.batch_classified events via Haiku 4.5.
//!
//! The worker is entirely optional: if ANTHROPIC_API_KEY is absent it exits
//! immediately and the rest of the system stays fully correct without it.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::commands::{CommandContext, Commands};
use crate::events::{CaptureItem, EventProvenance, EventType, TenantId};
use crate::ledger::{EventLedger, SqliteEventLedger};

const CLASSIFIER_MODEL: &str = "claude-haiku-4-5-20251001";
const SCHEMA_VERSION: &str = "2";
const ACTOR_ID: &str = "agent:hivemind:classifier";
const POLL_INTERVAL: Duration = Duration::from_secs(10);
const HAIKU_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TOKENS: u32 = 1200;
const HAIKU_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

// Classifier prompt from CAPTURE_CLASSIFIER.md
const CLASSIFIER_PROMPT: &str = r#"You are the HiveMind capture classifier.

HiveMind stores organizational decision memory: durable decisions, evidence,
hypotheses, blockers, decision requests, and notifications with provenance. It
does not store chat history, task tracking, private scratch notes, or raw tool
logs.

Read the batch of recent agent activity. Return only JSON matching the capture
schema. Most batches should return {"captures":[]}.

Capture a decision only when the text shows a chosen path among plausible
alternatives and gives or implies a reason. If a choice is requested but not yet
made, use decision-request instead.

Capture evidence only when there is an observation with a referent that could
support or refute a later decision, such as a test result, production symptom,
verified external fact, measured latency, or explicit user research finding.

Capture a hypothesis only when the text states a proposition being tested or a
claim that may later be supported or refuted.

Capture a blocker only when progress is materially stopped by a dependency,
permission issue, unavailable service, missing artifact, failing gate, or
unresolved external decision.

Capture a notification only when another actor would need the announcement
after restart, such as handoff state, merge readiness with proof, rejection
state, or completed verification.

Do not capture synthetic test data, fixture/demo content, gc/br plumbing,
branch-name mechanics, routine gate chatter, raw command output, stack traces,
file diffs, TODO lists, status narration, or generic plans. If the material is
borderline, omit it.

Never invent evidence ids. Use only ids present in the input. Keep titles short.
Use 1 to 5 lowercase topic keys. Confidence is your self-estimate for offline
tuning, not authoritative truth.

RELATIONAL FIELDS — populate only from explicit text, never infer:

expressed_confidence: For decisions only. Extract the decider's own words about
confidence level: "low" (tentative, provisional, lean, unsure), "medium"
(reasonably confident but caveated), "high" (firm, definite, committed). Use
null if the decider expresses no explicit confidence level.

actor_id: The specific actor who proposed/made/reported this item, IF named in
the input. Use their ID if present, otherwise null. Never infer a proposer from
context; only record them when explicitly stated.

accepted_by / rejected_by: For decisions. The specific actor who accepted or
rejected it, IF explicitly named. null otherwise.

supersedes_id: For decisions that explicitly replace a prior decision. Only use
an ID that appears verbatim in the input text. null if none.

assumes_ids: For decisions. Hypothesis IDs this decision explicitly assumes,
from the input text only. Empty array if none.

supports_ids / refutes_ids: For evidence. Hypothesis IDs this evidence
explicitly supports or refutes, from the input text only. Empty array if none.

blocked_actor_id: For blockers. The actor being blocked, IF named in the input.
decision_id: For blockers. The decision being blocked, IF its ID appears in
the input. Both null if not explicitly stated."#;

// JSON Schema for structured output
fn capture_schema() -> serde_json::Value {
    let nullable_string = serde_json::json!({
        "oneOf": [{ "type": "string" }, { "type": "null" }]
    });
    let string_array = serde_json::json!({
        "type": "array",
        "items": { "type": "string" }
    });
    serde_json::json!({
        "type": "object",
        "properties": {
            "captures": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["decision", "evidence", "hypothesis", "blocker", "decision-request", "notification"]
                        },
                        "title": { "type": "string" },
                        "rationale": { "type": "string" },
                        "topic_keys": string_array.clone(),
                        "evidence_ids": string_array.clone(),
                        "options": {
                            "oneOf": [
                                { "type": "array", "items": { "type": "string" } },
                                { "type": "null" }
                            ]
                        },
                        "chosen_option": nullable_string.clone(),
                        "extraction_confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "expressed_confidence": {
                            "oneOf": [
                                { "type": "string", "enum": ["low", "medium", "high"] },
                                { "type": "null" }
                            ]
                        },
                        "supersedes_id": nullable_string.clone(),
                        "assumes_ids": string_array.clone(),
                        "supports_ids": string_array.clone(),
                        "refutes_ids": string_array.clone(),
                        "actor_id": nullable_string.clone(),
                        "accepted_by": nullable_string.clone(),
                        "rejected_by": nullable_string.clone(),
                        "blocked_actor_id": nullable_string.clone(),
                        "decision_id": nullable_string.clone()
                    },
                    "required": [
                        "kind", "title", "rationale", "topic_keys", "evidence_ids",
                        "options", "chosen_option", "extraction_confidence",
                        "expressed_confidence", "supersedes_id",
                        "assumes_ids", "supports_ids", "refutes_ids",
                        "actor_id", "accepted_by", "rejected_by",
                        "blocked_actor_id", "decision_id"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": ["captures"],
        "additionalProperties": false
    })
}

#[derive(Debug, Serialize)]
struct HaikuRequest {
    model: &'static str,
    max_tokens: u32,
    output_config: HaikuOutputConfig,
    messages: Vec<HaikuMessage>,
}

#[derive(Debug, Serialize)]
struct HaikuOutputConfig {
    format: HaikuFormat,
}

#[derive(Debug, Serialize)]
struct HaikuFormat {
    #[serde(rename = "type")]
    format_type: &'static str,
    schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct HaikuMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct HaikuResponse {
    content: Vec<HaikuContentBlock>,
}

#[derive(Debug, Deserialize)]
struct HaikuContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClassifierOutput {
    captures: Vec<CaptureItemRaw>,
}

#[derive(Debug, Deserialize)]
struct CaptureItemRaw {
    kind: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    evidence_ids: Vec<String>,
    options: Option<Vec<String>>,
    chosen_option: Option<String>,
    extraction_confidence: f64,
    expressed_confidence: Option<String>,
    supersedes_id: Option<String>,
    #[serde(default)]
    assumes_ids: Vec<String>,
    #[serde(default)]
    supports_ids: Vec<String>,
    #[serde(default)]
    refutes_ids: Vec<String>,
    actor_id: Option<String>,
    accepted_by: Option<String>,
    rejected_by: Option<String>,
    blocked_actor_id: Option<String>,
    decision_id: Option<String>,
}

/// Spawn the background classifier task. Returns immediately; the worker runs
/// in the background until the process exits.
pub fn spawn_classifier(hivemind_dir: Arc<PathBuf>, tenant_id: TenantId, api_key: String) {
    tokio::spawn(async move {
        run_classifier_loop(hivemind_dir, tenant_id, api_key).await;
    });
}

async fn run_classifier_loop(hivemind_dir: Arc<PathBuf>, tenant_id: TenantId, api_key: String) {
    info!(target: "hivemind::classifier", "classifier worker started");
    let client = match reqwest::Client::builder().timeout(HAIKU_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            warn!(target: "hivemind::classifier", "failed to build http client: {e}");
            return;
        }
    };

    loop {
        classify_pending_batches(&client, &hivemind_dir, &tenant_id, &api_key).await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

struct BatchInfo {
    event_id: u64,
    batch_id: String,
    batch_text: String,
    /// actor_id from the IngestBatchReceived event (the batch submitter).
    actor_id: String,
    agent_tool: String,
    session_id: String,
}

async fn classify_pending_batches(
    client: &reqwest::Client,
    hivemind_dir: &PathBuf,
    tenant_id: &TenantId,
    api_key: &str,
) {
    let batches = match find_unclassified_batches(hivemind_dir, tenant_id) {
        Ok(b) => b,
        Err(e) => {
            warn!(target: "hivemind::classifier", "ledger scan failed: {e}");
            return;
        }
    };

    for batch in batches {
        debug!(target: "hivemind::classifier", batch_id = %batch.batch_id, "classifying batch");

        let session_initiator: Option<String> = if batch.actor_id.is_empty() {
            None
        } else {
            Some(batch.actor_id.clone())
        };

        // Build the agent actor ID from the session; empty session_id means no agent session.
        let agent_actor_id: Option<String> = if batch.session_id.is_empty() {
            None
        } else {
            Some(format!("agent:{}:{}", batch.agent_tool, batch.session_id))
        };

        match call_haiku(client, api_key, &batch.batch_text).await {
            Ok(output) => {
                let captures: Vec<CaptureItem> = output
                    .captures
                    .into_iter()
                    .map(|r| {
                        let mut participants: Vec<String> = Vec::new();
                        if let Some(ref initiator) = session_initiator {
                            participants.push(initiator.clone());
                        }
                        if let Some(ref agent) = agent_actor_id {
                            if !participants.contains(agent) {
                                participants.push(agent.clone());
                            }
                        }
                        CaptureItem {
                            kind: r.kind,
                            title: r.title,
                            rationale: r.rationale,
                            topic_keys: r.topic_keys,
                            evidence_ids: r.evidence_ids,
                            options: r.options,
                            chosen_option: r.chosen_option,
                            extraction_confidence: r.extraction_confidence,
                            expressed_confidence: r.expressed_confidence,
                            supersedes_id: r.supersedes_id,
                            assumes_ids: r.assumes_ids,
                            supports_ids: r.supports_ids,
                            refutes_ids: r.refutes_ids,
                            actor_id: r.actor_id,
                            accepted_by: r.accepted_by,
                            rejected_by: r.rejected_by,
                            blocked_actor_id: r.blocked_actor_id,
                            decision_id: r.decision_id,
                            participants,
                            session_initiator: session_initiator.clone(),
                        }
                    })
                    .collect();

                if let Err(e) = write_classification(
                    hivemind_dir,
                    tenant_id,
                    &batch.batch_id,
                    captures,
                    Some(batch.event_id),
                ) {
                    warn!(target: "hivemind::classifier", batch_id = %batch.batch_id, "write failed: {e}");
                }
            }
            Err(e) => {
                warn!(target: "hivemind::classifier", batch_id = %batch.batch_id, "haiku call failed: {}", e);
            }
        }
    }
}

fn find_unclassified_batches(
    hivemind_dir: &PathBuf,
    tenant_id: &TenantId,
) -> crate::Result<Vec<BatchInfo>> {
    let ledger = SqliteEventLedger::open(hivemind_dir)?;
    let mut offset = 0u64;
    const PAGE: usize = 256;

    let mut received: Vec<BatchInfo> = Vec::new();
    let mut classified_batch_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    loop {
        let events = ledger.read_for_tenant(tenant_id, offset, PAGE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            match event.event_type {
                EventType::IngestBatchReceived => {
                    if let Some(event_id) = event.event_id {
                        if let Some(batch_id) =
                            event.payload.get("batch_id").and_then(|v| v.as_str())
                        {
                            let batch_text = render_batch_text(event);
                            let agent_tool = event
                                .payload
                                .get("agent_tool")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_owned();
                            let session_id = event
                                .payload
                                .get("session_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_owned();
                            received.push(BatchInfo {
                                event_id,
                                batch_id: batch_id.to_owned(),
                                batch_text,
                                actor_id: event.actor_id.clone(),
                                agent_tool,
                                session_id,
                            });
                        }
                    }
                }
                EventType::IngestBatchClassified => {
                    if let Some(batch_id) = event.payload.get("batch_id").and_then(|v| v.as_str()) {
                        classified_batch_ids.insert(batch_id.to_owned());
                    }
                }
                _ => {}
            }
        }

        if let Some(last) = events.last().and_then(|e| e.event_id) {
            offset = last;
        } else {
            break;
        }
    }

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified_batch_ids.contains(&b.batch_id))
        .collect();

    Ok(pending)
}

fn render_batch_text(event: &crate::events::Event) -> String {
    let turns = event
        .payload
        .get("turns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = String::new();
    for turn in &turns {
        let role = turn
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let text = turn.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let truncated = turn
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if truncated {
            let _ = writeln!(out, "[{role}] {text} [TRUNCATED]");
        } else {
            let _ = writeln!(out, "[{role}] {text}");
        }
    }
    out
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn call_haiku(
    client: &reqwest::Client,
    api_key: &str,
    batch_text: &str,
) -> Result<ClassifierOutput, BoxError> {
    let user_content = format!("{CLASSIFIER_PROMPT}\n\n---BATCH---\n{batch_text}");

    let request = HaikuRequest {
        model: CLASSIFIER_MODEL,
        max_tokens: MAX_TOKENS,
        output_config: HaikuOutputConfig {
            format: HaikuFormat {
                format_type: "json_schema",
                schema: capture_schema(),
            },
        },
        messages: vec![HaikuMessage {
            role: "user",
            content: user_content,
        }],
    };

    let response = client
        .post(HAIKU_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("haiku returned {status}: {body}").into());
    }

    let haiku_resp: HaikuResponse = response.json().await?;
    let text = haiku_resp
        .content
        .into_iter()
        .find(|b| b.block_type == "text")
        .and_then(|b| b.text)
        .ok_or("no text block in haiku response")?;

    let output: ClassifierOutput = serde_json::from_str(&text)?;
    Ok(output)
}

fn write_classification(
    hivemind_dir: &PathBuf,
    tenant_id: &TenantId,
    batch_id: &str,
    captures: Vec<CaptureItem>,
    causation_event_id: Option<u64>,
) -> crate::Result<()> {
    let ledger = SqliteEventLedger::open(hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::agent(ACTOR_ID)),
    );
    commands.record_ingest_batch_classified(
        ACTOR_ID,
        batch_id,
        CLASSIFIER_MODEL,
        SCHEMA_VERSION,
        captures,
        causation_event_id,
    )?;
    Ok(())
}

/// Classify a single free-text input and return the raw capture items without
/// touching the ledger. Used by the fidelity evaluator to test the classifier
/// against the hand-authored gold corpus.
pub async fn classify_text(
    client: &reqwest::Client,
    api_key: &str,
    input: &str,
) -> Result<Vec<crate::events::CaptureItem>, Box<dyn std::error::Error + Send + Sync>> {
    let output = call_haiku(client, api_key, input).await?;
    let captures = output
        .captures
        .into_iter()
        .map(|r| crate::events::CaptureItem {
            kind: r.kind,
            title: r.title,
            rationale: r.rationale,
            topic_keys: r.topic_keys,
            evidence_ids: r.evidence_ids,
            options: r.options,
            chosen_option: r.chosen_option,
            extraction_confidence: r.extraction_confidence,
            expressed_confidence: r.expressed_confidence,
            supersedes_id: r.supersedes_id,
            assumes_ids: r.assumes_ids,
            supports_ids: r.supports_ids,
            refutes_ids: r.refutes_ids,
            actor_id: r.actor_id,
            accepted_by: r.accepted_by,
            rejected_by: r.rejected_by,
            blocked_actor_id: r.blocked_actor_id,
            decision_id: r.decision_id,
            participants: vec![],
            session_initiator: None,
        })
        .collect();
    Ok(captures)
}

/// Try to read the API key and spawn the worker. Logs a warning and returns
/// `None` if the key is absent.
pub fn try_spawn(hivemind_dir: Arc<PathBuf>, tenant_id: TenantId) -> Option<()> {
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            spawn_classifier(hivemind_dir, tenant_id, key);
            Some(())
        }
        _ => {
            warn!(
                target: "hivemind::classifier",
                "ANTHROPIC_API_KEY not set — Layer-3 classifier disabled"
            );
            None
        }
    }
}
