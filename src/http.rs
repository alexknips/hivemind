//! HTTP JSON-RPC transport for HiveMind.
//!
//! This module is intentionally a transport wrapper. It resolves request
//! context from HTTP headers, then delegates writes to `commands` and reads to
//! `queries`. It does not infer, rank, deduplicate, or call layer-3 code.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tracing::info;

use crate::commands::{CommandContext, Commands};
use crate::error::{CliError, CommandError, HivemindError};
use crate::events::{BlockerPriority, EventId, EventProvenance, RelationKind, TenantId};
use crate::ledger::{SqliteEventLedger, TenantScopedLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::{
    derive_decision_status, export_read_only_summary, get_active_decision_blockers,
    get_blocker_notification_candidates, get_decision, get_decision_neighborhood,
    get_decisions_added_since, get_decisions_changed_since, get_recent_activity,
    get_recent_decisions, get_relevant_decisions, get_supersession_chain,
    search_decisions_fts_with_context, ActiveDecisionBlockersRequest,
    BlockerNotificationCandidatesRequest, ChangedSinceRequest, DecisionBlockerFilters,
    DecisionStatus, DecisionsAddedSinceFilterRequest, DecisionsAddedSinceRequest,
    HistoryFilterRequest, NeighborhoodRequest, QueryContext, ReadOnlyExportFormat,
    ReadOnlyExportQuery, ReadOnlyExportRequest, RecentActivityRequest, RecentDecisionFilterRequest,
    RecentDecisionsRequest, SearchDecisionRequest,
};
use crate::Result;

const JSONRPC_INVALID_REQUEST: i32 = -32600;
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
const JSONRPC_INVALID_PARAMS: i32 = -32602;
const JSONRPC_INTERNAL_ERROR: i32 = -32603;
const JSONRPC_UNAUTHORIZED: i32 = -32001;

const TENANT_HEADER: &str = "X-HiveMind-Tenant";
const ACTOR_HEADER: &str = "X-HiveMind-Actor";
const SOURCE_REF_HEADER: &str = "X-HiveMind-Source-Ref";

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub hivemind_dir: PathBuf,
    pub addr: SocketAddr,
}

impl HttpConfig {
    pub fn new(hivemind_dir: impl Into<PathBuf>, addr: SocketAddr) -> Self {
        Self {
            hivemind_dir: hivemind_dir.into(),
            addr,
        }
    }
}

#[derive(Debug, Clone)]
struct HttpState {
    config: Arc<HttpConfig>,
}

pub fn router(config: HttpConfig) -> Router {
    let state = HttpState {
        config: Arc::new(config),
    };
    Router::new()
        .route("/health", get(health))
        .route("/v1/rpc", post(rpc))
        .with_state(state)
}

pub fn serve_blocking(config: HttpConfig) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| transport_error(format!("build HTTP runtime: {error}")))?;
    runtime.block_on(serve(config))
}

pub async fn serve(config: HttpConfig) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .map_err(|error| transport_error(format!("bind HTTP listener {}: {error}", config.addr)))?;
    info!(
        target: "hivemind::http",
        addr = %config.addr,
        "hivemind HTTP API listening"
    );
    axum::serve(listener, router(config))
        .await
        .map_err(|error| transport_error(format!("serve HTTP API: {error}")))
}

async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "hivemind",
        "version": env!("CARGO_PKG_VERSION"),
        "rpc_path": "/v1/rpc",
    }))
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn rpc(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<RpcRequest>,
) -> impl IntoResponse {
    let id = request.id.clone().unwrap_or(Value::Null);
    if request.jsonrpc.as_deref() != Some("2.0") {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_response(
                id,
                JSONRPC_INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
            )),
        );
    }

    let context = match HttpRequestContext::from_headers(&headers) {
        Ok(context) => context,
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(error_response(id, error.code, error.message)),
            )
        }
    };

    match dispatch(
        &request.method,
        request.params,
        state.config.as_ref(),
        &context,
    ) {
        Ok(result) => (StatusCode::OK, Json(success_response(id, result))),
        Err(error) => (
            StatusCode::OK,
            Json(error_response(id, error.code, error.message)),
        ),
    }
}

#[derive(Debug, Clone)]
struct HttpRequestContext {
    tenant_id: TenantId,
    actor_id: String,
    source_ref: Option<String>,
}

impl HttpRequestContext {
    fn from_headers(headers: &HeaderMap) -> std::result::Result<Self, HttpRpcError> {
        let tenant = required_header(headers, TENANT_HEADER)?;
        let tenant_id = TenantId::new(tenant.clone())
            .map_err(|error| HttpRpcError::unauthorized(format!("{TENANT_HEADER}: {error}")))?;
        let actor_id = required_header(headers, ACTOR_HEADER)?;
        let source_ref =
            optional_header(headers, SOURCE_REF_HEADER)?.or_else(|| Some(actor_id.clone()));
        Ok(Self {
            tenant_id,
            actor_id,
            source_ref,
        })
    }

    fn command_context(&self) -> CommandContext {
        CommandContext::new(
            self.tenant_id.clone(),
            EventProvenance::api(self.source_ref.clone()),
        )
    }

    fn query_context(&self) -> QueryContext {
        QueryContext::new(self.tenant_id.clone())
    }
}

struct HttpRpcError {
    code: i32,
    message: String,
}

impl HttpRpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: JSONRPC_INVALID_PARAMS,
            message: message.into(),
        }
    }

    fn method_not_found(method: &str) -> Self {
        Self {
            code: JSONRPC_METHOD_NOT_FOUND,
            message: format!("unknown method: {method}"),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: JSONRPC_INTERNAL_ERROR,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            code: JSONRPC_UNAUTHORIZED,
            message: message.into(),
        }
    }
}

impl From<HivemindError> for HttpRpcError {
    fn from(error: HivemindError) -> Self {
        let code = match &error {
            HivemindError::Command(CommandError::Validation(_))
            | HivemindError::Cli(CliError::InvalidInput(_)) => JSONRPC_INVALID_PARAMS,
            _ => JSONRPC_INTERNAL_ERROR,
        };
        let mut message = String::new();
        let _ = write!(&mut message, "{error}");
        Self { code, message }
    }
}

impl From<serde_json::Error> for HttpRpcError {
    fn from(error: serde_json::Error) -> Self {
        let mut message = String::with_capacity("json serialization failed: ".len() + 64);
        message.push_str("json serialization failed: ");
        let _ = write!(&mut message, "{error}");
        HttpRpcError::internal(message)
    }
}

fn dispatch(
    method: &str,
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    match method {
        "emit.decision.capture" => capture_decision(params, config, context, CaptureShape::Cli),
        "capture_decision" => capture_decision(params, config, context, CaptureShape::Detailed),
        "emit.decision.proposed" => capture_decision(params, config, context, CaptureShape::Cli),
        "emit.decision.accepted" => emit_decision_accepted(params, config, context),
        "emit.decision.rejected" => emit_decision_rejected(params, config, context),
        "emit.decision.superseded" => emit_decision_superseded(params, config, context),
        "emit.evidence.recorded" => record_evidence(params, config, context, CaptureShape::Cli),
        "capture_evidence" => record_evidence(params, config, context, CaptureShape::Detailed),
        "emit.hypothesis.recorded" => record_hypothesis(params, config, context, CaptureShape::Cli),
        "capture_hypothesis" => record_hypothesis(params, config, context, CaptureShape::Detailed),
        "emit.option.recorded" => record_option(params, config, context),
        "emit.relation.added" => emit_relation_added(params, config, context),
        "emit.relation.attach_evidence" => emit_attach_evidence(params, config, context),
        "disagree" | "disagree_decision" => disagree_decision(params, config, context),
        "supersede" | "supersede_decision" => supersede_decision(params, config, context),
        "query.get_decision" | "get_decision" => query_get_decision(params, config, context),
        "query.get_relevant_decisions" | "get_relevant_decisions" => {
            query_get_relevant_decisions(params, config, context)
        }
        "query.get_supersession_chain" | "get_supersession_chain" => {
            query_get_supersession_chain(params, config, context)
        }
        "query.get_decision_neighborhood" => {
            query_get_decision_neighborhood(params, config, context)
        }
        "query.search" | "query.search_decisions" | "search_decisions" => {
            query_search_decisions(params, config, context)
        }
        "query.get_active_decision_blockers" => {
            query_get_active_decision_blockers(params, config, context)
        }
        "query.get_blocker_notification_candidates" => {
            query_get_blocker_notification_candidates(params, config, context)
        }
        "query.recent" => query_recent_decisions(params, config, context),
        "query.get_recent_activity" => query_recent_activity(params, config, context),
        "query.get_decisions_changed_since" => {
            query_get_decisions_changed_since(params, config, context)
        }
        "query.get_decisions_added_since" => {
            query_get_decisions_added_since(params, config, context)
        }
        "query.export_read_only_summary" => query_export_read_only_summary(params, config, context),
        "dump" | "dump_graph" => dump_graph(params, config, context),
        other => Err(HttpRpcError::method_not_found(other)),
    }
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum CaptureShape {
    Cli,
    Detailed,
}

#[derive(Debug, Serialize)]
struct OutputEnvelope {
    subcommand: &'static str,
    kind: &'static str,
    value: String,
}

#[derive(Debug, Serialize)]
struct CapturedDecision {
    decision_id: String,
    option_ids: Vec<String>,
    chosen_option_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct DisagreeOutput {
    decision_id: String,
    event_id: EventId,
    decision_status: DecisionStatus,
}

#[derive(Debug, Serialize)]
struct SupersedeOutput {
    old_decision_id: String,
    new_decision_id: String,
    proposal_event_id: EventId,
    relation_event_ids: Vec<EventId>,
    superseded_event_id: EventId,
    old_decision_status: DecisionStatus,
    new_decision_status: DecisionStatus,
}

#[derive(Debug)]
struct OptionSpec {
    label: String,
    description: Option<String>,
}

fn capture_decision(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
    shape: CaptureShape,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let title = required_string(&args, "title")?;
    let rationale = required_string(&args, "rationale")?;
    let topic_keys = required_string_array(&args, "topic_keys")?;
    let options = required_options(&args)?;
    let chosen_option_label = optional_string_any(&args, &["chosen_option_label", "chose"])?;
    let hypothesis_ids = optional_string_array_any(&args, &["hypothesis_ids", "hypotheses"])?;
    let evidence_ids = optional_string_array_any(&args, &["evidence_ids", "evidence"])?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let mut option_ids = Vec::with_capacity(options.len());
    let mut chosen_option_index = None;

    for option in options {
        let is_chosen = chosen_option_label.as_deref() == Some(option.label.as_str());
        let description = option.description.unwrap_or_else(|| {
            let mut description = String::with_capacity(
                "Option generated from HTTP value ''".len() + option.label.len(),
            );
            let _ = write!(
                description,
                "Option generated from HTTP value '{}'",
                option.label
            );
            description
        });
        let option_id = commands.record_option(&actor_id, &option.label, &description)?;
        if is_chosen {
            chosen_option_index = Some(option_ids.len());
        }
        option_ids.push(option_id);
    }

    let chosen_option_id = chosen_option_index.and_then(|index| option_ids.get(index).cloned());

    if chosen_option_label.is_some() && chosen_option_id.is_none() {
        return Err(HttpRpcError::invalid_params(
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

    match shape {
        CaptureShape::Cli => to_value(&OutputEnvelope {
            subcommand: "emit",
            kind: "decision_id",
            value: decision_id,
        }),
        CaptureShape::Detailed => to_value(&CapturedDecision {
            decision_id,
            option_ids,
            chosen_option_id,
        }),
    }
}

fn emit_decision_accepted(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let decision_id = required_string_any(&args, &["decision_id", "id"])?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let event_id = commands.accept_decision(&decision_id, &actor_id)?;
    to_value(&OutputEnvelope {
        subcommand: "emit",
        kind: "event_id",
        value: event_id.to_string(),
    })
}

fn emit_decision_rejected(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let decision_id = required_string_any(&args, &["decision_id", "id"])?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let event_id = commands.reject_decision(&decision_id, &actor_id)?;
    to_value(&OutputEnvelope {
        subcommand: "emit",
        kind: "event_id",
        value: event_id.to_string(),
    })
}

fn emit_decision_superseded(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let old_decision_id = required_string_any(&args, &["old_decision_id", "old"])?;
    let new_decision_id = required_string_any(&args, &["new_decision_id", "new"])?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let event_id = commands.supersede_decision(&old_decision_id, &new_decision_id, &actor_id)?;
    to_value(&OutputEnvelope {
        subcommand: "emit",
        kind: "event_id",
        value: event_id.to_string(),
    })
}

fn record_evidence(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
    shape: CaptureShape,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let content = required_string(&args, "content")?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let evidence_id = commands.record_evidence(&actor_id, &content)?;
    match shape {
        CaptureShape::Cli => to_value(&OutputEnvelope {
            subcommand: "emit",
            kind: "evidence_id",
            value: evidence_id,
        }),
        CaptureShape::Detailed => Ok(json!({ "evidence_id": evidence_id })),
    }
}

fn record_hypothesis(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
    shape: CaptureShape,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let statement = required_string(&args, "statement")?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let hypothesis_id = commands.record_hypothesis(&actor_id, &statement)?;
    match shape {
        CaptureShape::Cli => to_value(&OutputEnvelope {
            subcommand: "emit",
            kind: "hypothesis_id",
            value: hypothesis_id,
        }),
        CaptureShape::Detailed => Ok(json!({ "hypothesis_id": hypothesis_id })),
    }
}

fn record_option(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let label = required_string(&args, "label")?;
    let description = required_string(&args, "description")?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let option_id = commands.record_option(&actor_id, &label, &description)?;
    to_value(&OutputEnvelope {
        subcommand: "emit",
        kind: "option_id",
        value: option_id,
    })
}

fn emit_relation_added(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let kind = parse_relation_kind(&required_string(&args, "kind")?)?;
    let from_id = required_string_any(&args, &["from_id", "from"])?;
    let to_id = required_string_any(&args, &["to_id", "to"])?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let event_id = match kind {
        RelationKind::BasedOn => commands.attach_evidence(&from_id, &to_id, &actor_id)?,
        RelationKind::Supports | RelationKind::Refutes => {
            commands.relate_evidence_to_hypothesis(&from_id, &to_id, kind, &actor_id)?
        }
        RelationKind::HasOption | RelationKind::Chose | RelationKind::Assumes => {
            return Err(HttpRpcError::invalid_params(
                "relation kind must be based_on, supports, or refutes",
            ))
        }
    };
    to_value(&OutputEnvelope {
        subcommand: "emit",
        kind: "event_id",
        value: event_id.to_string(),
    })
}

fn emit_attach_evidence(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let decision_id = required_string(&args, "decision_id")?;
    let evidence_id = required_string(&args, "evidence_id")?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let event_id = commands.attach_evidence(&decision_id, &evidence_id, &actor_id)?;
    to_value(&OutputEnvelope {
        subcommand: "emit",
        kind: "event_id",
        value: event_id.to_string(),
    })
}

fn disagree_decision(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let decision_id = required_string(&args, "decision_id")?;
    let reason = required_string(&args, "reason")?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
    let event_id = commands.disagree(&actor_id, &decision_id, &reason)?;
    let graph = rebuild_memory_graph(&ledger, &context.tenant_id)?;
    let decision_status = derive_decision_status(&graph, &decision_id)?;
    to_value(&DisagreeOutput {
        decision_id,
        event_id,
        decision_status,
    })
}

fn supersede_decision(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let actor_id = actor_from_params_or_context(&args, context)?;
    let old_decision_id = required_string(&args, "old_decision_id")?;
    let title = required_string(&args, "title")?;
    let rationale = required_string(&args, "rationale")?;
    let topic_keys = optional_string_array(&args, "topic_keys")?;
    let option_labels = optional_option_labels(&args, "options")?;
    let chosen_option_label = optional_string_any(&args, &["chosen_option_label", "chose"])?;
    let hypothesis_ids = optional_string_array_any(&args, &["hypothesis_ids", "hypotheses"])?;
    let evidence_ids = optional_string_array_any(&args, &["evidence_ids", "evidence"])?;

    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let commands = Commands::new_with_context(&ledger, context.command_context());
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
    let graph = rebuild_memory_graph(&ledger, &context.tenant_id)?;
    let old_decision_status = derive_decision_status(&graph, &old_decision_id)?;
    let new_decision_status = derive_decision_status(&graph, &outcome.new_decision_id)?;
    to_value(&SupersedeOutput {
        old_decision_id,
        new_decision_id: outcome.new_decision_id,
        proposal_event_id: outcome.proposal_event_id,
        relation_event_ids: outcome.relation_event_ids,
        superseded_event_id: outcome.superseded_event_id,
        old_decision_status,
        new_decision_status,
    })
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

fn query_get_decision(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let decision_id = required_string_any(&args, &["decision_id", "id"])?;
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&get_decision(&graph, &decision_id)?)
}

fn query_get_relevant_decisions(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let topic = required_string(&args, "topic")?;
    let status = optional_string(&args, "status")?
        .as_deref()
        .map(parse_decision_status)
        .transpose()?;
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&get_relevant_decisions(&graph, &topic, status)?)
}

fn query_get_supersession_chain(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let decision_id = required_string_any(&args, &["decision_id", "id"])?;
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&get_supersession_chain(&graph, &decision_id)?)
}

fn query_get_decision_neighborhood(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let decision_id = required_string_any(&args, &["decision_id", "id"])?;
    let depth = optional_usize(&args, "depth")?.unwrap_or(1);
    if depth != 1 {
        return Err(HttpRpcError::invalid_params(
            "depth other than 1 is not supported",
        ));
    }
    let relation_names = optional_string_array(&args, "relations")?;
    let request = if relation_names.is_empty() {
        NeighborhoodRequest::all()
    } else {
        NeighborhoodRequest::with_relations(
            relation_names
                .iter()
                .map(|relation| parse_graph_relation_kind(relation))
                .collect::<std::result::Result<Vec<_>, _>>()?,
        )
    };
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&get_decision_neighborhood(&graph, &decision_id, &request)?)
}

fn query_search_decisions(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let statuses = optional_string_array(&args, "status")?
        .into_iter()
        .map(|status| parse_decision_status(&status))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let request = SearchDecisionRequest {
        query: optional_string_any(&args, &["q", "query"])?,
        topic_keys: optional_string_array_any(&args, &["topic_keys", "topic"])?,
        statuses,
        actor_ids: optional_string_array(&args, "actor_id")?,
        sources: optional_string_array(&args, "source")?,
        since: optional_datetime(&args, "since")?,
        until: optional_datetime(&args, "until")?,
        limit: optional_usize(&args, "limit")?.unwrap_or(25),
        cursor: optional_string(&args, "cursor")?,
    };
    let (ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&search_decisions_fts_with_context(
        &context.query_context(),
        &ledger,
        &graph,
        &request,
    )?)
}

fn query_get_active_decision_blockers(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let priorities = optional_string_array(&args, "priority")?
        .into_iter()
        .map(|priority| parse_blocker_priority(&priority))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let request = ActiveDecisionBlockersRequest {
        filters: DecisionBlockerFilters {
            decision_ids: optional_string_array(&args, "decision_id")?,
            topic_keys: optional_string_array_any(&args, &["topic_keys", "topic"])?,
            required_owner_ids: optional_string_array_any(&args, &["required_owner_ids", "owner"])?,
            blocked_actor_ids: optional_string_array_any(
                &args,
                &["blocked_actor_ids", "blocked_actor"],
            )?,
            priorities,
            now: optional_datetime(&args, "now")?,
            stale_after_seconds: optional_i64(&args, "stale_after_seconds")?,
        },
        limit: optional_usize(&args, "limit")?.unwrap_or(25),
        cursor: optional_string(&args, "cursor")?,
    };
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&get_active_decision_blockers(&graph, &request)?)
}

fn query_get_blocker_notification_candidates(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let request = BlockerNotificationCandidatesRequest {
        now: required_datetime(&args, "now")?,
        policy_version: optional_string(&args, "policy_version")?
            .unwrap_or_else(|| "default-v1".to_owned()),
        limit: optional_usize(&args, "limit")?.unwrap_or(25),
        cursor: optional_string(&args, "cursor")?,
    };
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    to_value(&get_blocker_notification_candidates(&graph, &request)?)
}

fn query_recent_decisions(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let statuses = optional_string_array(&args, "status")?
        .into_iter()
        .map(|status| parse_decision_status(&status))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let request = RecentDecisionsRequest {
        since_timestamp: required_datetime(&args, "since")?,
        until_timestamp: optional_datetime(&args, "until")?,
        filters: RecentDecisionFilterRequest {
            actor_patterns: optional_string_array_any(&args, &["actor_patterns", "actor"])?,
            sources: optional_string_array(&args, "source")?,
            topic_keys: optional_string_array_any(&args, &["topic_keys", "topic"])?,
            statuses,
        },
        limit: optional_usize(&args, "limit")?.unwrap_or(25),
        cursor: optional_string(&args, "cursor")?,
    };
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
    to_value(&get_recent_decisions(&scoped_ledger, &request)?)
}

fn query_recent_activity(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let request = RecentActivityRequest {
        filters: history_filters(&args)?,
        limit: optional_usize(&args, "limit")?.unwrap_or(25),
        cursor: optional_string(&args, "cursor")?,
    };
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
    to_value(&get_recent_activity(&scoped_ledger, &request)?)
}

fn query_get_decisions_changed_since(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let request = changed_since_request(&args)?;
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
    to_value(&get_decisions_changed_since(&scoped_ledger, &request)?)
}

fn query_get_decisions_added_since(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let statuses = optional_string_array(&args, "status")?
        .into_iter()
        .map(|status| parse_decision_status(&status))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let request = DecisionsAddedSinceRequest {
        since_offset: optional_u64(&args, "since_offset")?,
        since_timestamp: optional_datetime_any(&args, &["since_ts", "since_timestamp", "since"])?,
        until_offset: optional_u64(&args, "until_offset")?,
        until_timestamp: optional_datetime_any(&args, &["until_ts", "until_timestamp", "until"])?,
        filters: DecisionsAddedSinceFilterRequest {
            actor_ids: optional_string_array(&args, "actor_id")?,
            sources: optional_string_array(&args, "source")?,
            source_refs: optional_string_array(&args, "source_ref")?,
            import_run_ids: optional_string_array_any(&args, &["import_run_ids", "import_run"])?,
            topic_keys: optional_string_array_any(&args, &["topic_keys", "topic"])?,
            statuses,
        },
        limit: optional_usize(&args, "limit")?.unwrap_or(25),
        cursor: optional_string(&args, "cursor")?,
    };
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
    to_value(&get_decisions_added_since(&scoped_ledger, &request)?)
}

fn query_export_read_only_summary(
    params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let args = params_object(params)?;
    let format = match optional_string(&args, "format")?
        .unwrap_or_else(|| "json".to_owned())
        .as_str()
    {
        "json" => ReadOnlyExportFormat::Json,
        "markdown" => ReadOnlyExportFormat::Markdown,
        other => {
            return Err(HttpRpcError::invalid_params(format!(
                "unknown export format `{other}`"
            )))
        }
    };
    let query = match required_string(&args, "query")?.as_str() {
        "recent_activity" => ReadOnlyExportQuery::RecentActivity(RecentActivityRequest {
            filters: history_filters(&args)?,
            limit: optional_usize(&args, "limit")?.unwrap_or(25),
            cursor: optional_string(&args, "cursor")?,
        }),
        "decisions_changed_since" => {
            ReadOnlyExportQuery::DecisionsChangedSince(changed_since_request(&args)?)
        }
        other => {
            return Err(HttpRpcError::invalid_params(format!(
                "unknown export query `{other}`"
            )))
        }
    };
    let request = ReadOnlyExportRequest {
        query,
        format,
        generated_at: optional_datetime(&args, "generated_at")?.unwrap_or_else(Utc::now),
    };
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
    to_value(&export_read_only_summary(&scoped_ledger, &request)?)
}

fn dump_graph(
    _params: Value,
    config: &HttpConfig,
    context: &HttpRequestContext,
) -> std::result::Result<Value, HttpRpcError> {
    let (_ledger, graph) = open_graph(config, &context.tenant_id)?;
    let dot = crate::cli::render_decision_dot(&graph)?;
    Ok(json!({ "format": "dot", "content": dot }))
}

// ---------------------------------------------------------------------------
// Request construction helpers
// ---------------------------------------------------------------------------

fn open_graph(
    config: &HttpConfig,
    tenant_id: &TenantId,
) -> std::result::Result<(SqliteEventLedger, MemoryGraph), HttpRpcError> {
    let ledger = SqliteEventLedger::open(&config.hivemind_dir)?;
    let graph = rebuild_memory_graph(&ledger, tenant_id)?;
    Ok((ledger, graph))
}

fn rebuild_memory_graph(
    ledger: &SqliteEventLedger,
    tenant_id: &TenantId,
) -> std::result::Result<MemoryGraph, HttpRpcError> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    Ok(graph)
}

fn changed_since_request(
    args: &Map<String, Value>,
) -> std::result::Result<ChangedSinceRequest, HttpRpcError> {
    Ok(ChangedSinceRequest {
        since_offset: optional_u64(args, "since_offset")?,
        since_timestamp: optional_datetime_any(args, &["since_ts", "since_timestamp"])?,
        until_offset: optional_u64(args, "until_offset")?,
        until_timestamp: optional_datetime_any(args, &["until_ts", "until_timestamp"])?,
        filters: history_filters(args)?,
        limit: optional_usize(args, "limit")?.unwrap_or(25),
        cursor: optional_string(args, "cursor")?,
    })
}

fn history_filters(
    args: &Map<String, Value>,
) -> std::result::Result<HistoryFilterRequest, HttpRpcError> {
    let statuses = optional_string_array(args, "status")?
        .into_iter()
        .map(|status| parse_decision_status(&status))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(HistoryFilterRequest {
        actor_ids: optional_string_array(args, "actor_id")?,
        sources: optional_string_array(args, "source")?,
        source_refs: optional_string_array(args, "source_ref")?,
        topic_keys: optional_string_array_any(args, &["topic_keys", "topic"])?,
        statuses,
    })
}

fn actor_from_params_or_context(
    args: &Map<String, Value>,
    context: &HttpRequestContext,
) -> std::result::Result<String, HttpRpcError> {
    if let Some(actor_id) = optional_string(args, "actor_id")? {
        if actor_id != context.actor_id {
            return Err(HttpRpcError::invalid_params(
                "actor_id must match X-HiveMind-Actor",
            ));
        }
    }
    Ok(context.actor_id.clone())
}

fn params_object(params: Value) -> std::result::Result<Map<String, Value>, HttpRpcError> {
    match params {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(Map::new()),
        _ => Err(HttpRpcError::invalid_params("params must be an object")),
    }
}

fn required_header(headers: &HeaderMap, name: &str) -> std::result::Result<String, HttpRpcError> {
    let value = optional_header(headers, name)?
        .ok_or_else(|| HttpRpcError::unauthorized(format!("missing {name}")))?;
    if value.trim().is_empty() {
        return Err(HttpRpcError::unauthorized(format!(
            "{name} must not be empty"
        )));
    }
    Ok(value)
}

fn optional_header(
    headers: &HeaderMap,
    name: &str,
) -> std::result::Result<Option<String>, HttpRpcError> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|error| {
        HttpRpcError::unauthorized(format!("{name} is not valid UTF-8: {error}"))
    })?;
    Ok(Some(value.trim().to_owned()))
}

fn required_string(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<String, HttpRpcError> {
    match args.get(field) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(Value::String(_)) => Err(HttpRpcError::invalid_params(format!(
            "`{field}` must not be empty"
        ))),
        Some(_) => Err(HttpRpcError::invalid_params(format!(
            "`{field}` must be a string"
        ))),
        None => Err(HttpRpcError::invalid_params(format!("missing `{field}`"))),
    }
}

fn required_string_any(
    args: &Map<String, Value>,
    fields: &[&str],
) -> std::result::Result<String, HttpRpcError> {
    for field in fields {
        if args.contains_key(*field) {
            return required_string(args, field);
        }
    }
    Err(HttpRpcError::invalid_params(format!(
        "missing `{}`",
        fields.join("` or `")
    )))
}

fn optional_string(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<String>, HttpRpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(HttpRpcError::invalid_params(format!(
            "`{field}` must be a string"
        ))),
    }
}

fn optional_string_any(
    args: &Map<String, Value>,
    fields: &[&str],
) -> std::result::Result<Option<String>, HttpRpcError> {
    for field in fields {
        if args.contains_key(*field) {
            return optional_string(args, field);
        }
    }
    Ok(None)
}

fn required_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, HttpRpcError> {
    match args.get(field) {
        Some(value) => string_array_value(value, field),
        None => Err(HttpRpcError::invalid_params(format!("missing `{field}`"))),
    }
}

fn optional_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, HttpRpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => string_array_value(value, field),
    }
}

fn optional_string_array_any(
    args: &Map<String, Value>,
    fields: &[&str],
) -> std::result::Result<Vec<String>, HttpRpcError> {
    for field in fields {
        if args.contains_key(*field) {
            return optional_string_array(args, field);
        }
    }
    Ok(Vec::new())
}

fn string_array_value(
    value: &Value,
    field: &str,
) -> std::result::Result<Vec<String>, HttpRpcError> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Ok(vec![value.clone()]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| match item {
                Value::String(value) if !value.trim().is_empty() => Ok(value.clone()),
                _ => Err(HttpRpcError::invalid_params(format!(
                    "`{field}[{index}]` must be a non-empty string"
                ))),
            })
            .collect(),
        _ => Err(HttpRpcError::invalid_params(format!(
            "`{field}` must be a string or array of strings"
        ))),
    }
}

fn required_options(
    args: &Map<String, Value>,
) -> std::result::Result<Vec<OptionSpec>, HttpRpcError> {
    let value = args
        .get("options")
        .or_else(|| args.get("option_ids"))
        .ok_or_else(|| HttpRpcError::invalid_params("missing `options`"))?;
    let options = option_specs(value, "options")?;
    if options.is_empty() {
        return Err(HttpRpcError::invalid_params("options must not be empty"));
    }
    Ok(options)
}

fn optional_option_labels(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Vec<String>, HttpRpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => option_specs(value, field).map(|options| {
            options
                .into_iter()
                .map(|option| option.label)
                .collect::<Vec<_>>()
        }),
    }
}

fn option_specs(value: &Value, field: &str) -> std::result::Result<Vec<OptionSpec>, HttpRpcError> {
    let Value::Array(items) = value else {
        return Err(HttpRpcError::invalid_params(format!(
            "`{field}` must be an array"
        )));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::String(label) if !label.trim().is_empty() => Ok(OptionSpec {
                label: label.clone(),
                description: None,
            }),
            Value::Object(map) => {
                let label = map
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .ok_or_else(|| {
                        HttpRpcError::invalid_params(format!(
                            "`{field}[{index}].label` must be a non-empty string"
                        ))
                    })?
                    .to_owned();
                let description = map
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|description| !description.is_empty())
                    .map(str::to_owned);
                Ok(OptionSpec { label, description })
            }
            _ => Err(HttpRpcError::invalid_params(format!(
                "`{field}[{index}]` must be a string or object"
            ))),
        })
        .collect()
}

fn optional_datetime(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<DateTime<Utc>>, HttpRpcError> {
    optional_string(args, field)?
        .map(|value| parse_datetime(field, &value))
        .transpose()
}

fn optional_datetime_any(
    args: &Map<String, Value>,
    fields: &[&str],
) -> std::result::Result<Option<DateTime<Utc>>, HttpRpcError> {
    for field in fields {
        if args.contains_key(*field) {
            return optional_datetime(args, field);
        }
    }
    Ok(None)
}

fn required_datetime(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<DateTime<Utc>, HttpRpcError> {
    parse_datetime(field, &required_string(args, field)?)
}

fn parse_datetime(field: &str, value: &str) -> std::result::Result<DateTime<Utc>, HttpRpcError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            HttpRpcError::invalid_params(format!("`{field}` must be RFC3339: {error}"))
        })
}

fn optional_usize(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<usize>, HttpRpcError> {
    optional_u64(args, field)?
        .map(|value| {
            usize::try_from(value).map_err(|error| {
                HttpRpcError::invalid_params(format!("`{field}` is too large: {error}"))
            })
        })
        .transpose()
}

fn optional_u64(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<u64>, HttpRpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number.as_u64().map(Some).ok_or_else(|| {
            HttpRpcError::invalid_params(format!("`{field}` must be a non-negative integer"))
        }),
        Some(_) => Err(HttpRpcError::invalid_params(format!(
            "`{field}` must be an integer"
        ))),
    }
}

fn optional_i64(
    args: &Map<String, Value>,
    field: &str,
) -> std::result::Result<Option<i64>, HttpRpcError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| HttpRpcError::invalid_params(format!("`{field}` must be an integer"))),
        Some(_) => Err(HttpRpcError::invalid_params(format!(
            "`{field}` must be an integer"
        ))),
    }
}

fn parse_decision_status(value: &str) -> std::result::Result<DecisionStatus, HttpRpcError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "proposed" => Ok(DecisionStatus::Proposed),
        "accepted" => Ok(DecisionStatus::Accepted),
        "rejected" => Ok(DecisionStatus::Rejected),
        "contested" => Ok(DecisionStatus::Contested),
        "superseded" => Ok(DecisionStatus::Superseded),
        other => Err(HttpRpcError::invalid_params(format!(
            "unknown decision status `{other}`"
        ))),
    }
}

fn parse_blocker_priority(value: &str) -> std::result::Result<BlockerPriority, HttpRpcError> {
    match value.trim().to_ascii_uppercase().as_str() {
        "P0" => Ok(BlockerPriority::P0),
        "P1" => Ok(BlockerPriority::P1),
        "P2" => Ok(BlockerPriority::P2),
        "P3" => Ok(BlockerPriority::P3),
        "P4" => Ok(BlockerPriority::P4),
        other => Err(HttpRpcError::invalid_params(format!(
            "unknown blocker priority `{other}`"
        ))),
    }
}

fn parse_relation_kind(value: &str) -> std::result::Result<RelationKind, HttpRpcError> {
    match normalize_enum(value).as_str() {
        "based_on" => Ok(RelationKind::BasedOn),
        "supports" => Ok(RelationKind::Supports),
        "refutes" => Ok(RelationKind::Refutes),
        "has_option" => Ok(RelationKind::HasOption),
        "chose" => Ok(RelationKind::Chose),
        "assumes" => Ok(RelationKind::Assumes),
        other => Err(HttpRpcError::invalid_params(format!(
            "unknown relation kind `{other}`"
        ))),
    }
}

fn parse_graph_relation_kind(
    value: &str,
) -> std::result::Result<crate::projector::RelationKind, HttpRpcError> {
    match normalize_enum(value).as_str() {
        "proposed_by" => Ok(crate::projector::RelationKind::ProposedBy),
        "accepted_by" => Ok(crate::projector::RelationKind::AcceptedBy),
        "rejected_by" => Ok(crate::projector::RelationKind::RejectedBy),
        "supersedes" => Ok(crate::projector::RelationKind::Supersedes),
        "based_on" => Ok(crate::projector::RelationKind::BasedOn),
        "has_option" => Ok(crate::projector::RelationKind::HasOption),
        "chose" => Ok(crate::projector::RelationKind::Chose),
        "assumes" => Ok(crate::projector::RelationKind::Assumes),
        "supports" => Ok(crate::projector::RelationKind::Supports),
        "refutes" => Ok(crate::projector::RelationKind::Refutes),
        other => Err(HttpRpcError::invalid_params(format!(
            "unknown graph relation kind `{other}`"
        ))),
    }
}

fn normalize_enum(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

fn to_value(value: &impl Serialize) -> std::result::Result<Value, HttpRpcError> {
    serde_json::to_value(value).map_err(Into::into)
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        },
    })
}

fn transport_error(message: String) -> HivemindError {
    CliError::InvalidInput(message).into()
}

#[cfg(test)]
mod tests;
