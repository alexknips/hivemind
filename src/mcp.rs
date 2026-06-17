//! Model Context Protocol (MCP) server for HiveMind.
//!
//! Exposes the same write/query surface the CLI does, framed as MCP tools so
//! any MCP-aware client (Claude Desktop, Claude Code, Cursor, Codex with MCP
//! support, etc.) can capture and read decisions through a single transport.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdio (MCP stdio transport).
//! Each request line on stdin produces at most one response line on stdout.
//! Notifications (no `id`) get no response.
//!
//! Layer discipline:
//!  * Capture tools delegate to [`crate::commands::Commands`] (layer 1, write).
//!  * Query tools delegate to [`crate::queries`] (layer 2, read).
//!  * Summarization tools delegate to [`crate::summarize`] (layer 3, swappable).
//!  * No additional inference happens here — the server is a thin transport.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tracing::{debug, warn};

use crate::commands::{CommandContext, Commands};
use crate::error::{CliError, CommandError, HivemindError};
use crate::events::{EventProvenance, TenantId};
use crate::identity::{agent_actor_id, agent_session_from_env, default_agent_tool};
use crate::ledger::SqliteEventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::{
    derive_decision_status, get_compact_view, get_decision, get_relevant_decisions,
    get_supersession_chain, search_decisions_fts_with_context, DecisionStatus, QueryContext,
    SearchDecisionRequest,
};
use crate::summarize::{summarize_decisions, SummarizeMode, SummarizeRequest};
use crate::Result;

/// MCP protocol revision this server speaks. Aligns with the modelcontextprotocol.io
/// 2025-03-26 schema (tools/list + tools/call); kept in one place so version
/// negotiation in `initialize` and the spec link in docs stay in sync.
const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "hivemind";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const JSONRPC_PARSE_ERROR: i32 = -32700;
const JSONRPC_INVALID_REQUEST: i32 = -32600;
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
const JSONRPC_INVALID_PARAMS: i32 = -32602;
const JSONRPC_INTERNAL_ERROR: i32 = -32603;

/// Configuration assembled at server startup. Env vars are resolved by the
/// caller (the CLI `mcp` subcommand) so this module stays easy to drive from
/// tests with explicit values.
#[derive(Debug, Clone)]
pub struct McpConfig {
    pub hivemind_dir: PathBuf,
    pub tenant_id: TenantId,
    /// Tool name embedded in default actor ids for write tools.
    pub agent_tool: String,
    /// Session identifier used to build the default actor id for write tools.
    /// Tools that don't provide a per-call `actor_id` fall back to this label
    /// prefixed with the configured agent tool.
    pub session_id: String,
}

impl McpConfig {
    pub fn new(hivemind_dir: impl Into<PathBuf>) -> Self {
        let agent_tool = default_agent_tool();
        let session_id = agent_session_from_env(&agent_tool).unwrap_or_else(default_session_id);
        Self {
            hivemind_dir: hivemind_dir.into(),
            tenant_id: TenantId::local(),
            agent_tool,
            session_id,
        }
    }

    pub fn with_agent_tool(mut self, agent_tool: impl Into<String>) -> Self {
        self.agent_tool = agent_tool.into();
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    pub fn with_tenant(mut self, tenant_id: TenantId) -> Self {
        self.tenant_id = tenant_id;
        self
    }

    fn command_context(&self, provenance: EventProvenance) -> CommandContext {
        CommandContext::new(self.tenant_id.clone(), provenance)
    }

    fn query_context(&self) -> QueryContext {
        QueryContext::new(self.tenant_id.clone())
    }
}

fn default_session_id() -> String {
    format!("mcp-{}", uuid::Uuid::new_v4())
}

/// Run the MCP server with stdin/stdout as transport.
///
/// Blocks until stdin closes. Returns Ok(()) on clean shutdown; transport-level
/// errors (e.g. broken pipe on stdout) surface as `CliError::InvalidInput` so
/// they share the existing CLI exit-code path.
pub fn serve_stdio(config: &McpConfig) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serve(config, BufReader::new(stdin.lock()), &mut stdout)
}

pub(crate) fn serve<R: BufRead, W: Write>(
    config: &McpConfig,
    mut reader: R,
    writer: &mut W,
) -> Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| transport_error(format!("read stdin: {error}")))?;
        if read == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_message(trimmed, config) {
            writeln!(writer, "{response}")
                .map_err(|error| transport_error(format!("write stdout: {error}")))?;
            writer
                .flush()
                .map_err(|error| transport_error(format!("flush stdout: {error}")))?;
        }
    }
}

fn handle_message(line: &str, config: &McpConfig) -> Option<String> {
    let parsed: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            warn!(target: "hivemind::mcp", %error, "failed to parse JSON-RPC message");
            return Some(error_response(
                Value::Null,
                JSONRPC_PARSE_ERROR,
                format!("invalid JSON: {error}"),
            ));
        }
    };

    let request = match Request::from_value(parsed) {
        Ok(req) => req,
        Err(err) => return Some(err),
    };

    debug!(target: "hivemind::mcp", method = %request.method, "dispatch");

    // Notifications carry no id and never produce a response (e.g.
    // `notifications/initialized` from the MCP handshake).
    let id = request.id.clone()?;
    let result = dispatch(&request.method, request.params, config);
    match result {
        Ok(value) => Some(success_response(id, value)),
        Err(err) => Some(error_response(id, err.code, err.message)),
    }
}

struct Request {
    method: String,
    params: Value,
    id: Option<Value>,
}

impl Request {
    fn from_value(value: Value) -> std::result::Result<Self, String> {
        let mut obj = match value {
            Value::Object(map) => map,
            _ => {
                return Err(error_response(
                    Value::Null,
                    JSONRPC_INVALID_REQUEST,
                    "request must be a JSON object".to_owned(),
                ))
            }
        };
        let method = match obj.remove("method") {
            Some(Value::String(s)) => s,
            _ => {
                return Err(error_response(
                    Value::Null,
                    JSONRPC_INVALID_REQUEST,
                    "missing or non-string `method`".to_owned(),
                ))
            }
        };
        let params = obj.remove("params").unwrap_or(Value::Null);
        let id = obj.remove("id");
        Ok(Self { method, params, id })
    }
}

struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: JSONRPC_INVALID_PARAMS,
            message: msg.into(),
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: JSONRPC_INTERNAL_ERROR,
            message: msg.into(),
        }
    }
    fn method_not_found(method: &str) -> Self {
        Self {
            code: JSONRPC_METHOD_NOT_FOUND,
            message: format!("unknown method: {method}"),
        }
    }
}

impl From<HivemindError> for RpcError {
    fn from(error: HivemindError) -> Self {
        let code = match &error {
            HivemindError::Command(CommandError::Validation(_))
            | HivemindError::Cli(CliError::InvalidInput(_)) => JSONRPC_INVALID_PARAMS,
            _ => JSONRPC_INTERNAL_ERROR,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

fn dispatch(
    method: &str,
    params: Value,
    config: &McpConfig,
) -> std::result::Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => tools_call(params, config),
        other => Err(RpcError::method_not_found(other)),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
    })
}

fn tools_list_result() -> Value {
    json!({ "tools": tool_definitions() })
}

fn tools_call(params: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let mut obj = match params {
        Value::Object(map) => map,
        _ => return Err(RpcError::invalid_params("params must be an object")),
    };
    let name = match obj.remove("name") {
        Some(Value::String(s)) => s,
        _ => return Err(RpcError::invalid_params("missing `name`")),
    };
    let arguments = obj.remove("arguments").unwrap_or(Value::Object(Map::new()));

    let outcome = match name.as_str() {
        "capture_decision" => tool_capture_decision(arguments, config),
        "capture_evidence" => tool_capture_evidence(arguments, config),
        "capture_hypothesis" => tool_capture_hypothesis(arguments, config),
        "disagree_decision" => tool_disagree_decision(arguments, config),
        "supersede_decision" => tool_supersede_decision(arguments, config),
        "get_decision" => tool_get_decision(arguments, config),
        "get_relevant_decisions" => tool_get_relevant_decisions(arguments, config),
        "get_supersession_chain" => tool_get_supersession_chain(arguments, config),
        "search_decisions" => tool_search_decisions(arguments, config),
        "dump_graph" => tool_dump_graph(arguments, config),
        "hivemind_compact_view" => tool_compact_view(arguments, config),
        "summarize_decisions" => tool_summarize_decisions(arguments, config),
        other => return Err(RpcError::invalid_params(format!("unknown tool: {other}"))),
    };

    match outcome {
        Ok(content) => Ok(tool_success(content)),
        Err(rpc) => Ok(tool_error(rpc.message)),
    }
}

// ---------------------------------------------------------------------------
// Tool descriptors
// ---------------------------------------------------------------------------

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "capture_decision",
            "description": "Record a proposed decision with rationale, topic keys, and at least one option. Defaults actor_id to agent:<tool>:<session> and writes source=agent.",
            "inputSchema": {
                "type": "object",
                "required": ["title", "rationale", "topic_keys", "options"],
                "properties": {
                    "actor_id": { "type": "string", "description": "Optional capturing actor override. Defaults to `agent:<tool>:<session>`." },
                    "title": { "type": "string" },
                    "rationale": { "type": "string" },
                    "topic_keys": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                    "options": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "required": ["label"],
                            "properties": {
                                "label": { "type": "string" },
                                "description": { "type": "string" }
                            }
                        }
                    },
                    "chosen_option_label": { "type": "string", "description": "Label of the option that was accepted; must match one of `options[].label`." },
                    "hypothesis_ids": { "type": "array", "items": { "type": "string" } },
                    "evidence_ids": { "type": "array", "items": { "type": "string" } }
                }
            }
        }),
        json!({
            "name": "capture_evidence",
            "description": "Record an evidence item that can be attached to decisions or hypotheses. Defaults actor_id to agent:<tool>:<session> and writes source=agent.",
            "inputSchema": {
                "type": "object",
                "required": ["content"],
                "properties": {
                    "actor_id": { "type": "string", "description": "Optional capturing actor override. Defaults to `agent:<tool>:<session>`." },
                    "content": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "capture_hypothesis",
            "description": "Record a hypothesis. Defaults actor_id to agent:<tool>:<session> and writes source=agent.",
            "inputSchema": {
                "type": "object",
                "required": ["statement"],
                "properties": {
                    "actor_id": { "type": "string", "description": "Optional capturing actor override. Defaults to `agent:<tool>:<session>`." },
                    "statement": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "disagree_decision",
            "description": "Record an actor disagreement with a decision and return the resulting derived status. Wraps `hivemind disagree`.",
            "inputSchema": {
                "type": "object",
                "required": ["decision_id", "reason"],
                "properties": {
                    "actor_id": { "type": "string", "description": "Disagreeing actor. Defaults to `agent:codex:<session>` when omitted." },
                    "decision_id": { "type": "string" },
                    "reason": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "supersede_decision",
            "description": "Propose a replacement decision and mark it as superseding an old decision. Wraps `hivemind supersede`.",
            "inputSchema": {
                "type": "object",
                "required": ["old_decision_id", "title", "rationale"],
                "properties": {
                    "actor_id": { "type": "string", "description": "Superseding actor. Defaults to `agent:codex:<session>` when omitted." },
                    "old_decision_id": { "type": "string" },
                    "title": { "type": "string" },
                    "rationale": { "type": "string" },
                    "topic_keys": { "type": "array", "items": { "type": "string" } },
                    "options": {
                        "type": "array",
                        "items": {
                            "anyOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "required": ["label"],
                                    "properties": {
                                        "label": { "type": "string" }
                                    }
                                }
                            ]
                        }
                    },
                    "chosen_option_label": { "type": "string" },
                    "hypothesis_ids": { "type": "array", "items": { "type": "string" } },
                    "evidence_ids": { "type": "array", "items": { "type": "string" } }
                }
            }
        }),
        json!({
            "name": "get_decision",
            "description": "Fetch a single decision by id. Returns null when absent.",
            "inputSchema": {
                "type": "object",
                "required": ["decision_id"],
                "properties": {
                    "decision_id": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "get_relevant_decisions",
            "description": "List decisions whose topic_keys contain the given topic. Optional status filter.",
            "inputSchema": {
                "type": "object",
                "required": ["topic"],
                "properties": {
                    "topic": { "type": "string" },
                    "status": { "type": "string", "enum": ["proposed", "accepted", "rejected", "contested", "superseded"] }
                }
            }
        }),
        json!({
            "name": "get_supersession_chain",
            "description": "Return the linear supersession chain a decision sits in, oldest first.",
            "inputSchema": {
                "type": "object",
                "required": ["decision_id"],
                "properties": {
                    "decision_id": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "search_decisions",
            "description": "Full-text search over decisions. Equivalent to `hivemind query search`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Full-text query." },
                    "topic": { "type": "array", "items": { "type": "string" } },
                    "status": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["proposed", "accepted", "rejected", "contested", "superseded"] }
                    },
                    "actor_id": { "type": "array", "items": { "type": "string" } },
                    "source": { "type": "array", "items": { "type": "string" } },
                    "since": { "type": "string", "description": "RFC3339 lower bound for decision proposal time." },
                    "until": { "type": "string", "description": "RFC3339 upper bound for decision proposal time." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                    "cursor": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "dump_graph",
            "description": "Render the current decision graph as Graphviz DOT.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "hivemind_compact_view",
            "description": "Layer-3 compact view of a decision subgraph. Applies signal/noise semantics: terminal decision is fully preserved; superseded predecessors, unchosen options, and resolved blockers are elided and counted. Contested decisions are never compacted. Returns null when the decision_id is not found.",
            "inputSchema": {
                "type": "object",
                "required": ["decision_id"],
                "properties": {
                    "decision_id": { "type": "string", "description": "The decision to compact. If mid-chain, the terminal (newest) decision in the supersession chain is used as the focal node." }
                }
            }
        }),
        json!({
            "name": "summarize_decisions",
            "description": "Layer-3: produce a concise text summary of one or more decisions. All content is sourced from decision record fields — no invented content. Every decision that contributed to the summary is listed in cited_decision_ids. Modes: single (one decision), cluster (multi-decision synthesis), chain (follows the supersession chain from the given decision_id).",
            "inputSchema": {
                "type": "object",
                "required": ["decision_ids"],
                "properties": {
                    "decision_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": 10,
                        "description": "IDs of decisions to summarize (1–10)."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["single", "cluster", "chain"],
                        "description": "single = one decision digest; cluster = multi-decision synthesis; chain = supersession chain evolution. Defaults to single when one ID is given, cluster when multiple."
                    }
                }
            }
        }),
    ]
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn tool_capture_decision(args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let actor_id = actor_id_or_default(&args, config)?;
    let title = require_string(&args, "title")?;
    let rationale = require_string(&args, "rationale")?;
    let topic_keys = require_string_array(&args, "topic_keys")?;
    if topic_keys.is_empty() {
        return Err(RpcError::invalid_params("topic_keys must not be empty"));
    }

    let options_value = args
        .get("options")
        .cloned()
        .ok_or_else(|| RpcError::invalid_params("missing `options`"))?;
    let options = match options_value {
        Value::Array(items) => items,
        _ => return Err(RpcError::invalid_params("`options` must be an array")),
    };
    if options.is_empty() {
        return Err(RpcError::invalid_params("options must not be empty"));
    }

    let chosen_label = optional_string(&args, "chosen_option_label")?;
    let hypothesis_ids = optional_string_array(&args, "hypothesis_ids")?;
    let evidence_ids = optional_string_array(&args, "evidence_ids")?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        config.command_context(EventProvenance::agent(actor_id.clone())),
    );

    let mut option_ids: Vec<String> = Vec::with_capacity(options.len());
    let mut chosen_option_id: Option<String> = None;
    for (index, option) in options.into_iter().enumerate() {
        let option_obj = match option {
            Value::Object(map) => map,
            _ => {
                return Err(RpcError::invalid_params(format!(
                    "options[{index}] must be an object"
                )))
            }
        };
        let label = option_obj
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                RpcError::invalid_params(format!(
                    "options[{index}].label must be a non-empty string"
                ))
            })?
            .to_owned();
        let description = option_obj
            .get("description")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_owned())
            .unwrap_or_else(|| {
                let mut description =
                    String::with_capacity("Option generated from MCP value ''".len() + label.len());
                let _ = write!(description, "Option generated from MCP value '{label}'");
                description
            });

        let option_id = commands.record_option(&actor_id, &label, &description)?;
        if chosen_label.as_deref() == Some(label.as_str()) {
            chosen_option_id = Some(option_id.clone());
        }
        option_ids.push(option_id);
    }

    if chosen_label.is_some() && chosen_option_id.is_none() {
        return Err(RpcError::invalid_params(
            "chosen_option_label must match one of the supplied option labels",
        ));
    }

    let decision_id = commands.propose_decision(
        &actor_id,
        &title,
        &rationale,
        &topic_keys,
        &option_ids,
        chosen_option_id.as_deref(),
        &hypothesis_ids,
        &evidence_ids,
    )?;

    Ok(json!({
        "decision_id": decision_id,
        "option_ids": option_ids,
        "chosen_option_id": chosen_option_id,
    }))
}

fn tool_capture_evidence(args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let actor_id = actor_id_or_default(&args, config)?;
    let content = require_string(&args, "content")?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        config.command_context(EventProvenance::agent(actor_id.clone())),
    );
    let evidence_id = commands.record_evidence(&actor_id, &content)?;
    Ok(json!({ "evidence_id": evidence_id }))
}

fn tool_capture_hypothesis(
    args: Value,
    config: &McpConfig,
) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let actor_id = actor_id_or_default(&args, config)?;
    let statement = require_string(&args, "statement")?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        config.command_context(EventProvenance::agent(actor_id.clone())),
    );
    let hypothesis_id = commands.record_hypothesis(&actor_id, &statement)?;
    Ok(json!({ "hypothesis_id": hypothesis_id }))
}

fn tool_disagree_decision(args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let actor_id = mcp_actor_id(&args, config)?;
    let decision_id = require_string(&args, "decision_id")?;
    let reason = require_string(&args, "reason")?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        config.command_context(EventProvenance::agent(config.session_id.clone())),
    );
    let event_id = commands.disagree(&actor_id, &decision_id, &reason)?;
    let graph = open_memory_graph(config)?;
    let decision_status = derive_decision_status(&graph, &decision_id)?;

    Ok(json!({
        "decision_id": decision_id,
        "event_id": event_id,
        "decision_status": decision_status,
    }))
}

fn tool_supersede_decision(
    args: Value,
    config: &McpConfig,
) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let actor_id = mcp_actor_id(&args, config)?;
    let old_decision_id = require_string(&args, "old_decision_id")?;
    let title = require_string(&args, "title")?;
    let rationale = require_string(&args, "rationale")?;
    let topic_keys = optional_string_array(&args, "topic_keys")?;
    let option_labels = optional_option_labels(&args, "options")?;
    let chosen_option_label = optional_string(&args, "chosen_option_label")?;
    let hypothesis_ids = optional_string_array(&args, "hypothesis_ids")?;
    let evidence_ids = optional_string_array(&args, "evidence_ids")?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        config.command_context(EventProvenance::agent(config.session_id.clone())),
    );
    let outcome = commands.supersede(
        &actor_id,
        &old_decision_id,
        &title,
        &rationale,
        &topic_keys,
        &option_labels,
        chosen_option_label.as_deref(),
        &hypothesis_ids,
        &evidence_ids,
    )?;
    let graph = open_memory_graph(config)?;
    let old_decision_status = derive_decision_status(&graph, &old_decision_id)?;
    let new_decision_status = derive_decision_status(&graph, &outcome.new_decision_id)?;

    Ok(json!({
        "old_decision_id": old_decision_id,
        "new_decision_id": outcome.new_decision_id,
        "proposal_event_id": outcome.proposal_event_id,
        "relation_event_ids": outcome.relation_event_ids,
        "superseded_event_id": outcome.superseded_event_id,
        "old_decision_status": old_decision_status,
        "new_decision_status": new_decision_status,
    }))
}

fn tool_get_decision(args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let decision_id = require_string(&args, "decision_id")?;
    let graph = open_memory_graph(config)?;
    let response = get_decision(&graph, &decision_id)?;
    Ok(serde_json::to_value(QueryEnvelope::from(response))?)
}

fn tool_get_relevant_decisions(
    args: Value,
    config: &McpConfig,
) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let topic = require_string(&args, "topic")?;
    let status = optional_string(&args, "status")?;
    let status_filter = match status.as_deref() {
        None => None,
        Some("proposed") => Some(DecisionStatus::Proposed),
        Some("accepted") => Some(DecisionStatus::Accepted),
        Some("rejected") => Some(DecisionStatus::Rejected),
        Some("contested") => Some(DecisionStatus::Contested),
        Some("superseded") => Some(DecisionStatus::Superseded),
        Some(other) => {
            return Err(RpcError::invalid_params(format!(
                "unknown status `{other}`"
            )))
        }
    };
    let graph = open_memory_graph(config)?;
    let response = get_relevant_decisions(&graph, &topic, status_filter)?;
    Ok(serde_json::to_value(QueryEnvelope::from(response))?)
}

fn tool_get_supersession_chain(
    args: Value,
    config: &McpConfig,
) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let decision_id = require_string(&args, "decision_id")?;
    let graph = open_memory_graph(config)?;
    let response = get_supersession_chain(&graph, &decision_id)?;
    Ok(serde_json::to_value(QueryEnvelope::from(response))?)
}

fn tool_search_decisions(args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let query = optional_string(&args, "q")?;
    let statuses = optional_string_array(&args, "status")?
        .into_iter()
        .map(|status| parse_decision_status(&status))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let limit = optional_usize(&args, "limit")?.unwrap_or(25);
    let request = SearchDecisionRequest {
        query,
        topic_keys: optional_string_array(&args, "topic")?,
        statuses,
        actor_ids: optional_string_array(&args, "actor_id")?,
        sources: optional_string_array(&args, "source")?,
        since: optional_datetime(&args, "since")?,
        until: optional_datetime(&args, "until")?,
        limit,
        cursor: optional_string(&args, "cursor")?,
    };
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &config.tenant_id, &graph)?;
    let response =
        search_decisions_fts_with_context(&config.query_context(), &ledger, &graph, &request)?;
    Ok(serde_json::to_value(QueryEnvelope::from(response))?)
}

fn tool_summarize_decisions(
    args: Value,
    config: &McpConfig,
) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let decision_ids = require_string_array(&args, "decision_ids")?;
    if decision_ids.is_empty() {
        return Err(RpcError::invalid_params("decision_ids must not be empty"));
    }
    let mode_str = optional_string(&args, "mode")?;
    let mode = match mode_str.as_deref() {
        None if decision_ids.len() == 1 => SummarizeMode::Single,
        None => SummarizeMode::Cluster,
        Some("single") if decision_ids.len() != 1 => {
            return Err(RpcError::invalid_params(
                "mode=single requires exactly one decision_id",
            ))
        }
        Some("single") => SummarizeMode::Single,
        Some("cluster") => SummarizeMode::Cluster,
        Some("chain") if decision_ids.len() != 1 => {
            return Err(RpcError::invalid_params(
                "mode=chain requires exactly one decision_id",
            ))
        }
        Some("chain") => SummarizeMode::Chain,
        Some(other) => {
            return Err(RpcError::invalid_params(format!(
                "unknown mode `{other}`; must be single, cluster, or chain"
            )))
        }
    };
    let graph = open_memory_graph(config)?;
    let request = SummarizeRequest { decision_ids, mode };
    let response = summarize_decisions(&graph, &request).map_err(RpcError::from)?;
    Ok(serde_json::to_value(QueryEnvelope::from(response))?)
}

fn tool_dump_graph(_args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let graph = open_memory_graph(config)?;
    let dot = crate::cli::render_decision_dot(&graph)?;
    Ok(json!({ "format": "dot", "content": dot }))
}

fn tool_compact_view(args: Value, config: &McpConfig) -> std::result::Result<Value, RpcError> {
    let args = args.as_object().cloned().unwrap_or_default();
    let decision_id = require_string(&args, "decision_id")?;
    let graph = open_memory_graph(config)?;
    let response = get_compact_view(&graph, &decision_id)?;
    serde_json::to_value(&response).map_err(|e| RpcError::internal(e.to_string()))
}

fn open_memory_graph(config: &McpConfig) -> Result<MemoryGraph> {
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &config.tenant_id, &graph)?;
    Ok(graph)
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn actor_id_or_default(
    args: &Map<String, Value>,
    config: &McpConfig,
) -> std::result::Result<String, RpcError> {
    optional_string(args, "actor_id").map(|actor_id| {
        actor_id.unwrap_or_else(|| agent_actor_id(&config.agent_tool, &config.session_id))
    })
}

fn require_string(args: &Map<String, Value>, field: &str) -> std::result::Result<String, RpcError> {
    match args.get(field) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(RpcError::invalid_params(format!(
            "`{field}` must be a non-empty string"
        ))),
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{field}` must be a string"
        ))),
        None => Err(RpcError::invalid_params(format!("missing `{field}`"))),
    }
}

fn optional_string(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<String>, RpcError> {
    match args.get(field) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) | None | Some(Value::Null) => Ok(None),
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{field}` must be a string"
        ))),
    }
}

fn require_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, RpcError> {
    match args.get(field) {
        Some(Value::Array(items)) => collect_strings(items, field),
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{field}` must be an array of strings"
        ))),
        None => Err(RpcError::invalid_params(format!("missing `{field}`"))),
    }
}

fn optional_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, RpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => collect_strings(items, field),
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{field}` must be an array of strings"
        ))),
    }
}

fn optional_datetime(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<DateTime<Utc>>, RpcError> {
    let Some(value) = optional_string(args, field)? else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(&value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|error| RpcError::invalid_params(format!("`{field}` must be RFC3339: {error}")))
}

fn optional_usize(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<usize>, RpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let Some(value) = number.as_u64() else {
                return Err(RpcError::invalid_params(format!(
                    "`{field}` must be a non-negative integer"
                )));
            };
            usize::try_from(value).map(Some).map_err(|error| {
                RpcError::invalid_params(format!("`{field}` is too large: {error}"))
            })
        }
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{field}` must be an integer"
        ))),
    }
}

fn optional_option_labels(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, RpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| match item {
                Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
                Value::Object(map) => map
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        RpcError::invalid_params(format!(
                            "`{field}[{index}].label` must be a non-empty string"
                        ))
                    }),
                _ => Err(RpcError::invalid_params(format!(
                    "`{field}[{index}]` must be a non-empty string or an object with a non-empty label"
                ))),
            })
            .collect(),
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{field}` must be an array"
        ))),
    }
}

fn parse_decision_status(value: &str) -> std::result::Result<DecisionStatus, RpcError> {
    match value {
        "proposed" => Ok(DecisionStatus::Proposed),
        "accepted" => Ok(DecisionStatus::Accepted),
        "rejected" => Ok(DecisionStatus::Rejected),
        "contested" => Ok(DecisionStatus::Contested),
        "superseded" => Ok(DecisionStatus::Superseded),
        other => Err(RpcError::invalid_params(format!(
            "unknown status `{other}`"
        ))),
    }
}

fn mcp_actor_id(
    args: &Map<String, Value>,
    config: &McpConfig,
) -> std::result::Result<String, RpcError> {
    if let Some(actor_id) = optional_string(args, "actor_id")? {
        return Ok(actor_id);
    }
    if config.session_id.starts_with("agent:") {
        Ok(config.session_id.clone())
    } else {
        Ok(format!("agent:codex:{}", config.session_id))
    }
}

fn collect_strings(items: &[Value], field: &str) -> std::result::Result<Vec<String>, RpcError> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
            _ => Err(RpcError::invalid_params(format!(
                "`{field}[{index}]` must be a non-empty string"
            ))),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Envelope shaping
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct QueryEnvelope<T: Serialize> {
    result_count: usize,
    truncated: bool,
    latency_ms: u128,
    data: T,
}

impl<T: Serialize> From<crate::queries::QueryResponse<T>> for QueryEnvelope<T> {
    fn from(response: crate::queries::QueryResponse<T>) -> Self {
        Self {
            result_count: response.result_count,
            truncated: response.truncated,
            latency_ms: response.latency_ms,
            data: response.data,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC envelope rendering
// ---------------------------------------------------------------------------

fn success_response(id: Value, result: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
    .to_string()
}

fn error_response(id: Value, code: i32, message: String) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

fn tool_success(payload: Value) -> Value {
    let body = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned());
    json!({
        "content": [{ "type": "text", "text": body }],
        "isError": false,
        "structuredContent": payload,
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

fn transport_error(message: String) -> HivemindError {
    HivemindError::Cli(CliError::InvalidInput(message))
}

impl From<serde_json::Error> for RpcError {
    fn from(error: serde_json::Error) -> Self {
        RpcError::internal(format!("serialization failed: {error}"))
    }
}

#[cfg(test)]
mod tests;
