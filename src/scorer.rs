//! Layer-3 background scorer: reads ingest.batch_classified events and
//! annotates decision captures with decision.scored events via Haiku 4.5.
//!
//! Two-axis model per docs/DECISION_SCORING.md:
//!   Quality [0,1] — 7 dimensions, weighted composite, score stored per-dim
//!   Importance (unbounded) — Stakes × Irreversibility × Actionability
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
use crate::events::{
    DecisionScoredPayload, EventProvenance, EventType, ImportanceFactors, QualityDim, QualityDims,
    TenantId,
};
use crate::ledger::{EventLedger, SqliteEventLedger};

const SCORER_MODEL: &str = "claude-haiku-4-5-20251001";
const WEIGHT_VERSION: &str = "v1";
const ACTOR_ID: &str = "agent:hivemind:scorer";
const POLL_INTERVAL: Duration = Duration::from_secs(30);
const API_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TOKENS: u32 = 2000;
const HAIKU_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

const SCORER_PROMPT: &str = r#"You are the HiveMind decision scorer.

HiveMind stores organizational decision memory. Your job is to score a captured
decision on two independent axes, assessed EX ANTE — only from what was knowable
at decision time. Never penalise or reward a decision for its outcomes.

AXIS 1 — Quality [0.0,1.0]: How well-made was the decision?
Score each of the 7 dimensions from 0.0 (absent/poor) to 1.0 (excellent):
  framing        — Was the right problem/question framed?
  alternatives   — Were genuine alternatives generated and considered?
  information    — Was relevant information gathered and used?
  reasoning      — Is the inference from information to choice sound?
  values_tradeoffs — Were values and tradeoffs made explicit and weighed?
  bias_exposure  — Exposure to cognitive distortions (anchoring, confirmation,
                   sunk-cost, framing, motivated reasoning). 1.0=low bias.
  calibration    — Does expressed confidence match the evidence? 1.0=well-calibrated.

AXIS 2 — Importance (unbounded magnitude):
  stakes         — Unbounded positive float, log-scaled. Small decisions: ~1.
                   Department-level: ~10. Company-level: ~100. Industry-level: ~1000.
                   Computed as severity × reach.
  irreversibility — [0.0,1.0]. 0=fully reversible (two-way door), 1=irreversible.
  actionability  — [0.0,1.0]. 0=not actionable (pure observation), 1=fully actionable.

For each score, give a short (1-2 sentence) explanation grounded in the decision text.
If a dimension cannot be assessed from the available text, score it 0.5 and explain why.

Return only JSON matching the schema. Be honest and calibrated."#;

fn scorer_schema() -> serde_json::Value {
    let dim_obj = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["score", "explanation"],
        "properties": {
            "score": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
            "explanation": { "type": "string", "minLength": 1 }
        }
    });
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["quality_dims", "importance"],
        "properties": {
            "quality_dims": {
                "type": "object",
                "additionalProperties": false,
                "required": ["framing","alternatives","information","reasoning","values_tradeoffs","bias_exposure","calibration"],
                "properties": {
                    "framing":          dim_obj.clone(),
                    "alternatives":     dim_obj.clone(),
                    "information":      dim_obj.clone(),
                    "reasoning":        dim_obj.clone(),
                    "values_tradeoffs": dim_obj.clone(),
                    "bias_exposure":    dim_obj.clone(),
                    "calibration":      dim_obj.clone()
                }
            },
            "importance": {
                "type": "object",
                "additionalProperties": false,
                "required": ["stakes","stakes_explanation","irreversibility","irreversibility_explanation","actionability","actionability_explanation"],
                "properties": {
                    "stakes":                    { "type": "number" },
                    "stakes_explanation":         { "type": "string", "minLength": 1 },
                    "irreversibility":           { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "irreversibility_explanation":{ "type": "string", "minLength": 1 },
                    "actionability":             { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "actionability_explanation":  { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}

#[derive(Debug, Serialize)]
struct ApiRequest {
    model: &'static str,
    max_tokens: u32,
    output_config: ApiOutputConfig,
    messages: Vec<ApiMessage>,
}

#[derive(Debug, Serialize)]
struct ApiOutputConfig {
    format: ApiFormat,
}

#[derive(Debug, Serialize)]
struct ApiFormat {
    #[serde(rename = "type")]
    format_type: &'static str,
    schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ApiContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ApiContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScorerOutput {
    quality_dims: RawQualityDims,
    importance: RawImportance,
}

#[derive(Debug, Deserialize)]
struct RawDim {
    score: f64,
    explanation: String,
}

#[derive(Debug, Deserialize)]
struct RawQualityDims {
    framing: RawDim,
    alternatives: RawDim,
    information: RawDim,
    reasoning: RawDim,
    values_tradeoffs: RawDim,
    bias_exposure: RawDim,
    calibration: RawDim,
}

#[derive(Debug, Deserialize)]
struct RawImportance {
    stakes: f64,
    stakes_explanation: String,
    irreversibility: f64,
    irreversibility_explanation: String,
    actionability: f64,
    actionability_explanation: String,
}

/// Information about a decision capture that needs scoring.
struct CaptureToScore {
    /// The canonical capture node ID: `capture:{event_id}:{idx}`
    node_id: String,
    /// Batch classified event ID (for causation linkage).
    event_id: u64,
    /// Text description to send to the model.
    decision_text: String,
}

/// Spawn the background scorer task. Returns immediately; the worker runs
/// in the background until the process exits.
pub fn spawn_scorer(hivemind_dir: Arc<PathBuf>, tenant_id: TenantId, api_key: String) {
    tokio::spawn(async move {
        run_scorer_loop(hivemind_dir, tenant_id, api_key).await;
    });
}

async fn run_scorer_loop(hivemind_dir: Arc<PathBuf>, tenant_id: TenantId, api_key: String) {
    info!(target: "hivemind::scorer", "scorer worker started");
    let client = match reqwest::Client::builder().timeout(API_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            warn!(target: "hivemind::scorer", "failed to build http client: {e}");
            return;
        }
    };

    loop {
        score_pending_captures(&client, &hivemind_dir, &tenant_id, &api_key).await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn score_pending_captures(
    client: &reqwest::Client,
    hivemind_dir: &PathBuf,
    tenant_id: &TenantId,
    api_key: &str,
) {
    let captures = match find_unscored_decisions(hivemind_dir, tenant_id) {
        Ok(c) => c,
        Err(e) => {
            warn!(target: "hivemind::scorer", "ledger scan failed: {e}");
            return;
        }
    };

    for capture in captures {
        debug!(target: "hivemind::scorer", node_id = %capture.node_id, "scoring decision capture");

        match call_scorer(client, api_key, &capture.decision_text).await {
            Ok(output) => {
                if let Err(e) = write_score(
                    hivemind_dir,
                    tenant_id,
                    &capture.node_id,
                    output,
                    Some(capture.event_id),
                ) {
                    warn!(target: "hivemind::scorer", node_id = %capture.node_id, "write failed: {e}");
                }
            }
            Err(e) => {
                warn!(target: "hivemind::scorer", node_id = %capture.node_id, "api call failed: {e}");
            }
        }
    }
}

fn find_unscored_decisions(
    hivemind_dir: &PathBuf,
    tenant_id: &TenantId,
) -> crate::Result<Vec<CaptureToScore>> {
    let ledger = SqliteEventLedger::open(hivemind_dir)?;
    let mut offset = 0u64;
    const PAGE: usize = 256;

    let mut pending: Vec<CaptureToScore> = Vec::new();
    let mut scored_node_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        let events = ledger.read_for_tenant(tenant_id, offset, PAGE)?;
        if events.is_empty() {
            break;
        }

        for event in &events {
            match event.event_type {
                EventType::IngestBatchClassified => {
                    if let Some(event_id) = event.event_id {
                        if let Some(captures) =
                            event.payload.get("captures").and_then(|v| v.as_array())
                        {
                            for (idx, capture) in captures.iter().enumerate() {
                                let kind =
                                    capture.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                                if kind == "decision" {
                                    let node_id = format!("capture:{event_id}:{idx}");
                                    let text = render_decision_text(capture);
                                    pending.push(CaptureToScore {
                                        node_id,
                                        event_id,
                                        decision_text: text,
                                    });
                                }
                            }
                        }
                    }
                }
                EventType::DecisionScored => {
                    if let Some(node_id) = event
                        .payload
                        .get("capture_node_id")
                        .and_then(|v| v.as_str())
                    {
                        scored_node_ids.insert(node_id.to_owned());
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

    let unscored: Vec<_> = pending
        .into_iter()
        .filter(|c| !scored_node_ids.contains(&c.node_id))
        .collect();

    Ok(unscored)
}

/// Build a compact text description of a decision capture for the scoring prompt.
fn render_decision_text(capture: &serde_json::Value) -> String {
    let mut out = String::new();

    if let Some(title) = capture.get("title").and_then(|v| v.as_str()) {
        let _ = writeln!(out, "Title: {title}");
    }
    if let Some(rationale) = capture.get("rationale").and_then(|v| v.as_str()) {
        let _ = writeln!(out, "Rationale: {rationale}");
    }
    if let Some(options) = capture.get("options").and_then(|v| v.as_array()) {
        let opts: Vec<&str> = options.iter().filter_map(|o| o.as_str()).collect();
        if !opts.is_empty() {
            let _ = writeln!(out, "Options considered: {}", opts.join(", "));
        }
    }
    if let Some(chosen) = capture.get("chosen_option").and_then(|v| v.as_str()) {
        let _ = writeln!(out, "Chosen option: {chosen}");
    }
    if let Some(expressed) = capture.get("expressed_confidence").and_then(|v| v.as_str()) {
        let _ = writeln!(out, "Expressed confidence: {expressed}");
    }
    if let Some(keys) = capture.get("topic_keys").and_then(|v| v.as_array()) {
        let ks: Vec<&str> = keys.iter().filter_map(|k| k.as_str()).collect();
        if !ks.is_empty() {
            let _ = writeln!(out, "Topic keys: {}", ks.join(", "));
        }
    }

    out
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn call_scorer(
    client: &reqwest::Client,
    api_key: &str,
    decision_text: &str,
) -> Result<ScorerOutput, BoxError> {
    let user_content = format!("{SCORER_PROMPT}\n\n---DECISION---\n{decision_text}");

    let request = ApiRequest {
        model: SCORER_MODEL,
        max_tokens: MAX_TOKENS,
        output_config: ApiOutputConfig {
            format: ApiFormat {
                format_type: "json_schema",
                schema: scorer_schema(),
            },
        },
        messages: vec![ApiMessage {
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
        return Err(format!("scorer API returned {status}: {body}").into());
    }

    let api_resp: ApiResponse = response.json().await?;
    let text = api_resp
        .content
        .into_iter()
        .find(|b| b.block_type == "text")
        .and_then(|b| b.text)
        .ok_or("no text block in scorer API response")?;

    let output: ScorerOutput = serde_json::from_str(&text)?;
    Ok(output)
}

fn write_score(
    hivemind_dir: &PathBuf,
    tenant_id: &TenantId,
    capture_node_id: &str,
    output: ScorerOutput,
    causation_event_id: Option<u64>,
) -> crate::Result<()> {
    let quality_dims = QualityDims {
        framing: QualityDim {
            score: output.quality_dims.framing.score,
            explanation: output.quality_dims.framing.explanation,
        },
        alternatives: QualityDim {
            score: output.quality_dims.alternatives.score,
            explanation: output.quality_dims.alternatives.explanation,
        },
        information: QualityDim {
            score: output.quality_dims.information.score,
            explanation: output.quality_dims.information.explanation,
        },
        reasoning: QualityDim {
            score: output.quality_dims.reasoning.score,
            explanation: output.quality_dims.reasoning.explanation,
        },
        values_tradeoffs: QualityDim {
            score: output.quality_dims.values_tradeoffs.score,
            explanation: output.quality_dims.values_tradeoffs.explanation,
        },
        bias_exposure: QualityDim {
            score: output.quality_dims.bias_exposure.score,
            explanation: output.quality_dims.bias_exposure.explanation,
        },
        calibration: QualityDim {
            score: output.quality_dims.calibration.score,
            explanation: output.quality_dims.calibration.explanation,
        },
    };

    let importance = ImportanceFactors {
        stakes: output.importance.stakes,
        stakes_explanation: output.importance.stakes_explanation,
        irreversibility: output.importance.irreversibility,
        irreversibility_explanation: output.importance.irreversibility_explanation,
        actionability: output.importance.actionability,
        actionability_explanation: output.importance.actionability_explanation,
    };

    let payload = DecisionScoredPayload {
        capture_node_id: capture_node_id.to_owned(),
        scorer_model: SCORER_MODEL.to_owned(),
        weight_version: WEIGHT_VERSION.to_owned(),
        supersedes_score_id: None,
        quality_dims,
        importance,
    };

    let ledger = SqliteEventLedger::open(hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::agent(ACTOR_ID)),
    );
    commands.record_decision_scored(ACTOR_ID, payload, causation_event_id)?;
    Ok(())
}

/// Try to read the API key and spawn the worker. Logs a warning and returns
/// `None` if the key is absent.
pub fn try_spawn(hivemind_dir: Arc<PathBuf>, tenant_id: TenantId) -> Option<()> {
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            spawn_scorer(hivemind_dir, tenant_id, key);
            Some(())
        }
        _ => {
            warn!(
                target: "hivemind::scorer",
                "ANTHROPIC_API_KEY not set — Layer-3 scorer disabled"
            );
            None
        }
    }
}
