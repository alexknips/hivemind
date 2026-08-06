use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write as IoWrite};

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::commands::{CommandContext, Commands, DecisionProposalInput, SupersedeInput};
use crate::error::CliError;
use crate::events::{
    CaptureItem, Event, EventPayload, EventProvenance, RelationKind as EventRelationKind, TenantId,
};
use crate::identity::{
    agent_actor_id, default_agent_session, default_agent_tool, default_human_actor_id,
};
use crate::ingest::{
    extract_slack_decision_draft, import_documents, import_slack_thread,
    parse_slack_thread_fixture, prepare_document_texts, DocumentImportRequest,
    DocumentPreparationRequest, SlackIngestOutcome,
};
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::PostgresEventLedger;
use crate::ledger::{EventLedger, SqliteEventLedger, TenantScopedLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant, GraphView};
use crate::queries::{
    derive_decision_status, export_read_only_summary, get_active_decision_blockers,
    get_blocker_notification_candidates, get_compact_view, get_decision, get_decision_neighborhood,
    get_decisions_added_since, get_decisions_changed_since, get_recent_activity,
    get_recent_decisions, get_relevant_decisions, get_supersession_chain, search_decisions,
    search_decisions_fts_with_context, ActiveDecisionBlockersRequest,
    BlockerNotificationCandidatesRequest, ChangedSinceRequest, DecisionBlockerFilters,
    DecisionStatus, DecisionsAddedSinceFilterRequest, DecisionsAddedSinceRequest,
    HistoryFilterRequest, NeighborhoodRequest, QueryContext, ReadOnlyExportQuery,
    ReadOnlyExportRequest, RecentActivityRequest, RecentDecisionEntry, RecentDecisionFilterRequest,
    RecentDecisionsRequest, SearchDecisionRequest,
};
use crate::slack_app::{
    handle_slack_command, slack_app_manifest, slack_oauth_install_url, SlackAppStore,
    SlackCaptureRequest, SlackCommandRequest, SlackWorkspaceInstall,
};
use crate::suggest::{
    materialize_document_extraction_candidates, propose_document_extraction_candidates,
    DocumentCandidateExtractor, DocumentCandidateMaterializationRequest, DocumentCandidateRequest,
};
use crate::summarize::{
    recall_decisions, weekly_digest, DigestRequest, RecallRequest, RECALL_MAX_LIMIT,
};
use crate::{HivemindError, Result};

use super::args::{
    ClassifyQueueArgs, ClassifyQueueCommand, ClassifyQueueListArgs, ClassifyQueueSubmitArgs, Cli,
    Command, ConnectorArgs, ConnectorAuthArgs, ConnectorCommand, DecisionCaptureSource, DigestArgs,
    DisagreeArgs, DumpArgs, DumpFormat, EmitArgs, EmitCaptureProvenanceArgs, EmitCommand,
    EmitDecisionProposedArgs, EmitRelationKind, GraphBackend, ImportArgs, ImportCommand,
    ImportConnectorCommand, IngestArgs, IngestCommand, IngestSlackThreadArgs, MapArgs, McpArgs,
    QueryAddedSinceArgs, QueryArgs, QueryBlockerPriority, QueryChangedSinceArgs, QueryCommand,
    QueryDecisionStatus, QueryExportKind, QueryExportReadOnlySummaryArgs, QueryHistoryFilterArgs,
    QueryRecentActivityArgs, QueryRecentDecisionsArgs, QueryRelationKind, QuerySearchDecisionsArgs,
    QuickstartArgs, ReviewArgs, ServeArgs, SlackAppArgs, SlackAppCommand, SuggestArgs,
    SuggestCommand, SuggestDocumentCandidatesArgs, SupersedeArgs, TuiArgs,
};
use super::render::{
    append_truncation_notice, decision_status_label, format_disagree_output, format_import_output,
    format_json_value, format_output, format_prepare_documents_output, format_query_response,
    format_review_output, format_supersede_output, render_active_blockers_summary,
    render_added_since_summary, render_blocker_notifications_summary, render_changed_since_summary,
    render_compact_view_summary, render_decision_list_summary, render_decision_summary, render_dot,
    render_neighborhood_summary, render_read_only_export_summary, render_recall_summary,
    render_recent_activity_summary, render_recent_decisions_summary, render_search_summary,
    render_supersession_summary, DisagreeCommandOutput, OutputEnvelope, ReviewActionOutput,
    ReviewCommandOutput, SupersedeCommandOutput,
};
#[cfg(feature = "shared-backend-postgres")]
use super::render::{MigrateReport, ParityCheckResult};

pub fn run(cli: &Cli) -> Result<String> {
    validate_global_flags(cli)?;

    match &cli.command {
        Command::Quickstart(args) => run_quickstart(cli, args),
        Command::Emit(command) => run_emit(cli, command),
        Command::Disagree(args) => run_disagree(cli, args),
        Command::Supersede(args) => run_supersede(cli, args),
        Command::Review(args) => run_review(cli, args),
        Command::Import(import) => run_import(cli, import),
        Command::Suggest(suggest) => run_suggest(cli, suggest),
        Command::Query(query) => run_query(cli, query),
        Command::Dump(dump) => run_dump(cli, dump),
        Command::Tui(args) => run_tui(cli, args),
        Command::Ingest(args) => run_ingest(cli, args),
        Command::SlackApp(args) => run_slack_app(cli, args),
        Command::Mcp(args) => run_mcp(cli, args),
        Command::Serve(args) => run_serve(cli, args),
        #[cfg(feature = "shared-backend-postgres")]
        Command::Migrate(args) => run_migrate(cli, args),
        Command::Map(args) => run_map(cli, args),
        Command::Digest(args) => run_digest(cli, args),
        Command::ClassifyQueue(args) => run_classify_queue(cli, args),
        Command::Connector(args) => run_connector(cli, args),
    }
}

fn run_quickstart(cli: &Cli, _args: &QuickstartArgs) -> Result<String> {
    let ledger_dir = std::env::temp_dir().join(format!("hivemind-quickstart-{}", Uuid::new_v4()));
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&ledger_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::cli()),
    );
    let decision_args = EmitDecisionProposedArgs {
        title: "Try HiveMind quickstart".to_owned(),
        rationale: "A first decision should be captured with actor provenance and queried back immediately.".to_owned(),
        topic_keys: vec!["quickstart".to_owned(), "onboarding".to_owned()],
        option_ids: vec!["local-ledger".to_owned(), "spreadsheet".to_owned()],
        chosen_option_id: Some("local-ledger".to_owned()),
        hypothesis_ids: Vec::new(),
        evidence_ids: Vec::new(),
    };
    let decision_id = propose_decision_from_option_labels(&commands, &cli.actor, &decision_args)?;

    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
    let query = search_decisions(
        &graph,
        &SearchDecisionRequest {
            query: Some("quickstart".to_owned()),
            topic_keys: vec!["quickstart".to_owned()],
            statuses: vec![DecisionStatus::Proposed],
            actor_ids: vec![cli.actor.clone()],
            sources: vec!["cli".to_owned()],
            since: None,
            until: None,
            limit: 5,
            cursor: None,
        },
    )?;
    let first_result_id = query
        .data
        .items
        .first()
        .map(|item| item.decision.id.clone());

    if first_result_id.as_deref() != Some(decision_id.as_str()) {
        return Err(CliError::InvalidInput(
            "quickstart query did not return captured decision".to_owned(),
        )
        .into());
    }

    let report = QuickstartReport {
        ledger_dir: ledger_dir.display().to_string(),
        actor_id: cli.actor.clone(),
        decision_id,
        query: QuickstartQueryReport {
            result_count: query.result_count,
            total_matches: query.data.total_matches,
            truncated: query.truncated,
            first_result_id,
        },
    };

    if cli.json {
        format_json_value(true, &report)
    } else {
        Ok(format_quickstart_report(&report))
    }
}

fn format_quickstart_report(report: &QuickstartReport) -> String {
    format!(
        "HiveMind quickstart complete.\n\
         Ledger: {ledger_dir}\n\
         Actor: {actor_id}\n\
         Captured: {decision_id}\n\
         Queried: found {first_result_id} ({result_count} result, truncated={truncated})\n\n\
         Try the query again:\n\
           hivemind --hivemind-dir {ledger_dir} query search_decisions --topic quickstart --limit 5",
        ledger_dir = report.ledger_dir,
        actor_id = report.actor_id,
        decision_id = report.decision_id,
        first_result_id = report
            .query
            .first_result_id
            .as_deref()
            .unwrap_or("<missing>"),
        result_count = report.query.result_count,
        truncated = report.query.truncated
    )
}

fn run_mcp(cli: &Cli, args: &McpArgs) -> Result<String> {
    let mut config =
        crate::mcp::McpConfig::new(cli.hivemind_dir.clone()).with_tenant(cli_tenant(cli)?);
    if let Some(agent_tool) = args.agent_tool.as_deref().map(str::trim) {
        if !agent_tool.is_empty() {
            config = config.with_agent_tool(agent_tool);
        }
    }
    if let Some(session_id) = args.session_id.as_deref().map(str::trim) {
        if !session_id.is_empty() {
            config = config.with_session_id(session_id);
        }
    }
    crate::mcp::serve_stdio(&config)?;
    // The stdio loop only returns once stdin closes — no payload to print.
    Ok(String::new())
}

fn run_serve(cli: &Cli, args: &ServeArgs) -> Result<String> {
    let config = crate::api::ApiConfig::new(cli.hivemind_dir.clone()).with_port(args.port);
    // Build AppState (which constructs r2d2/postgres pool) BEFORE entering
    // the tokio runtime. r2d2 pool construction internally calls block_on,
    // which panics if already inside an existing runtime.
    let state = crate::api::AppState::from_config(&config)?;
    // Hold a clone so the Arc<ApiBackend> (postgres pool) survives until AFTER
    // the runtime is dropped. Rust drops locals in reverse declaration order:
    // `runtime` (declared below) drops before `_pg_guard`, so the pool's Drop
    // runs outside any runtime context — preventing the block_on-within-block_on
    // SIGABRT on scale-to-zero autostop (hivemind-noc9).
    let _pg_guard = state.clone();
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| CliError::InvalidInput(format!("failed to create tokio runtime: {e}")))?;
    runtime.block_on(crate::api::serve_http(state, &config))?;
    Ok(String::new())
}

fn run_map(cli: &Cli, args: &MapArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;

    let alphas: Vec<f64> = if args.alpha.is_empty() {
        vec![0.5]
    } else {
        args.alpha.clone()
    };

    if alphas.len() == 1 {
        let result = crate::map::compute_map(&graph, &cli.hivemind_dir, alphas[0]) // ubs:ignore: alphas[0] guarded by len()==1 check above
            .map_err(|e| CliError::InvalidInput(e.to_string()))?; // ubs:ignore: error conversion at CLI boundary
        if args.summary {
            let mut out = format!(
                "Decision map: {} decisions, alpha={:.2}, gen={}\n", // ubs:ignore: format! for CLI output
                result.n,
                result.alpha,
                &result.gen_id[..8] // ubs:ignore: UUID is 36 chars; 8-char prefix always in-bounds
            );
            let points: String = result
                .points
                .iter()
                .map(|p| {
                    format!(
                        "  [{:>6.2}, {:>6.2}] {:8} {}\n",
                        p.x_time, p.y_spectral, p.status, p.title
                    )
                })
                .collect();
            out.push_str(&points);
            Ok(out)
        } else {
            serde_json::to_string_pretty(&result)
                .map_err(|e| CliError::InvalidInput(e.to_string()).into()) // ubs:ignore: error conversion at CLI boundary
        }
    } else {
        let mut results = Vec::new();
        for &alpha in &alphas {
            let r = crate::map::compute_map(&graph, &cli.hivemind_dir, alpha)
                .map_err(|e| CliError::InvalidInput(e.to_string()))?; // ubs:ignore: error conversion at CLI boundary
            results.push(r);
        }
        serde_json::to_string_pretty(&results)
            .map_err(|e| CliError::InvalidInput(e.to_string()).into()) // ubs:ignore: error conversion at CLI boundary
    }
}

fn parse_window_duration(window: &str) -> Result<chrono::Duration> {
    let window = window.trim();
    let (digits, unit) = window.split_at(
        window
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(window.len()), // ubs:ignore: unwrap_or — safe default: all-digits treated as days
    );
    let n: i64 = digits
        .parse()
        .map_err(|_| CliError::InvalidInput(format!("--window: invalid duration '{window}'")))?;
    if n <= 0 {
        return Err(CliError::InvalidInput(format!(
            "--window: duration must be positive, got '{window}'"
        ))
        .into());
    }
    match unit {
        "h" => Ok(chrono::Duration::hours(n)),
        "d" | "" => Ok(chrono::Duration::days(n)),
        "w" => Ok(chrono::Duration::weeks(n)),
        other => Err(CliError::InvalidInput(format!(
            "--window: unknown unit '{other}'; use h (hours), d (days), or w (weeks)"
        ))
        .into()),
    }
}

fn run_digest(cli: &Cli, args: &DigestArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let context = QueryContext::new(tenant_id.clone());
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;

    let now = Utc::now();

    let until = match args.until.as_deref() {
        Some(s) => parse_required_query_datetime(s, "--until")?,
        None => now,
    };
    let since = match args.since.as_deref() {
        Some(s) => parse_required_query_datetime(s, "--since")?,
        None => {
            let duration = parse_window_duration(&args.window)?;
            until - duration
        }
    };

    let request = DigestRequest {
        since,
        until,
        actor_ids: args.actor_ids.clone(), // ubs:ignore: clone necessary — building owned DigestRequest from borrowed DigestArgs
        limit: args.limit,
    };
    let response = weekly_digest(&context, &ledger, &graph, &request)?;

    if args.summary {
        let mut out = response.data.text.clone(); // ubs:ignore: clone necessary — formatting owned output from borrowed DigestResponse
        append_truncation_notice(&mut out, response.truncated, None);
        Ok(out.trim_end().to_owned())
    } else {
        format_json_value(true, &response)
    }
}

fn run_classify_queue(cli: &Cli, args: &ClassifyQueueArgs) -> Result<String> {
    match &args.command {
        ClassifyQueueCommand::List(args) => run_classify_queue_list(cli, args),
        ClassifyQueueCommand::Submit(args) => run_classify_queue_submit(cli, args),
    }
}

fn run_classify_queue_list(cli: &Cli, args: &ClassifyQueueListArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let mut batches = crate::classifier::list_pending_batches(&cli.hivemind_dir, &tenant_id)
        .map_err(|e| CliError::InvalidInput(format!("ledger scan failed: {e}")))?;
    batches.truncate(args.limit);
    format_json_value(cli.json, &batches)
}

fn run_classify_queue_submit(cli: &Cli, args: &ClassifyQueueSubmitArgs) -> Result<String> {
    let captures: Vec<CaptureItem> = serde_json::from_str(&args.captures)
        .map_err(|e| CliError::InvalidInput(format!("--captures is not valid JSON: {e}")))?;

    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id, EventProvenance::cli()),
    );

    let capture_count = captures.len();
    commands.record_ingest_batch_classified(
        &cli.actor,
        &args.batch_id,
        &args.model,
        crate::classifier::SCHEMA_VERSION,
        captures,
        None,
    )?;

    let result = serde_json::json!({
        "batch_id": args.batch_id,
        "capture_count": capture_count,
    });
    format_json_value(cli.json, &result)
}

fn run_connector(cli: &Cli, args: &ConnectorArgs) -> Result<String> {
    match &args.command {
        ConnectorCommand::Auth(auth_args) => run_connector_auth(cli, auth_args),
    }
}

fn run_connector_auth(cli: &Cli, args: &ConnectorAuthArgs) -> Result<String> {
    match args.connector.as_str() {
        "gdocs" | "google" | "google-docs" | "googledocs" => run_connector_auth_gdocs(cli),
        other => Err(CliError::InvalidInput(format!(
            "unknown connector '{other}'; supported: gdocs"
        ))
        .into()),
    }
}

fn run_connector_auth_gdocs(cli: &Cli) -> Result<String> {
    let client_id = std::env::var("HIVEMIND_GOOGLE_CLIENT_ID").map_err(|_| {
        CliError::InvalidInput(
            "HIVEMIND_GOOGLE_CLIENT_ID environment variable must be set".to_owned(),
        )
    })?;
    let client_secret = std::env::var("HIVEMIND_GOOGLE_CLIENT_SECRET").map_err(|_| {
        CliError::InvalidInput(
            "HIVEMIND_GOOGLE_CLIENT_SECRET environment variable must be set".to_owned(),
        )
    })?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| CliError::InvalidInput(format!("failed to create async runtime: {e}")))?;

    let (auth_code, redirect_uri) =
        runtime.block_on(crate::connector::listen_for_google_oauth_code(&client_id))?;

    let token_store = crate::connector::GoogleTokenStore::new(&cli.hivemind_dir);
    crate::connector::exchange_google_oauth_code(
        &client_id,
        &client_secret,
        &auth_code,
        &redirect_uri,
        &token_store,
    )?;

    Ok(
        "Google Docs connector authenticated successfully. Token saved to connector-tokens.json."
            .to_owned(),
    )
}

fn run_ingest(cli: &Cli, ingest: &IngestArgs) -> Result<String> {
    match &ingest.command {
        IngestCommand::SlackThread(args) => run_ingest_slack_thread(cli, args),
    }
}

fn run_ingest_slack_thread(cli: &Cli, args: &IngestSlackThreadArgs) -> Result<String> {
    let contents = std::fs::read_to_string(&args.file).map_err(|error| {
        CliError::InvalidInput(format!(
            "could not read slack thread file {}: {error}",
            args.file.display()
        ))
    })?;
    let thread = parse_slack_thread_fixture(&contents)?;
    let draft = extract_slack_decision_draft(&thread, &args.mention)?;

    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, cli_tenant(cli)?);
    let outcome = import_slack_thread(&scoped_ledger, &draft)?;

    let kind = match outcome {
        SlackIngestOutcome::Imported { .. } => "decision_id",
        SlackIngestOutcome::AlreadyImported { .. } => "decision_id_existing",
    };
    let envelope = OutputEnvelope::new("ingest", kind, outcome.decision_id().to_owned());
    format_output(cli.json, &envelope)
}

fn run_slack_app(cli: &Cli, args: &SlackAppArgs) -> Result<String> {
    let store = SlackAppStore::new(&cli.hivemind_dir);
    match &args.command {
        SlackAppCommand::Manifest(args) => {
            let manifest = slack_app_manifest(
                &args.request_url,
                args.event_url.as_deref(),
                args.redirect_url.as_deref(),
            )?;
            format_json_value(cli.json, &manifest)
        }
        SlackAppCommand::OauthUrl(args) => {
            slack_oauth_install_url(&args.client_id, &args.redirect_uri, &args.state)
        }
        SlackAppCommand::Install(args) => {
            let summary = store.install_workspace(SlackWorkspaceInstall {
                team_id: args.team_id.clone(),
                team_name: args.team_name.clone(),
                bot_token: args.bot_token.clone(),
                signing_secret: args.signing_secret.clone(),
                hivemind_url: args.hivemind_url.clone(),
                reaction_emoji: args.reaction_emoji.clone(),
                actor_mappings: parse_actor_mappings(&args.actor_mappings)?,
            })?;
            format_json_value(cli.json, &summary)
        }
        SlackAppCommand::EnqueueCapture(args) => {
            let event = store.enqueue_capture(SlackCaptureRequest {
                team_id: args.team_id.clone(),
                user_id: args.user_id.clone(),
                channel_id: args.channel_id.clone(),
                message_ts: args.message_ts.clone(),
                thread_ts: args
                    .thread_ts
                    .clone()
                    .unwrap_or_else(|| args.message_ts.clone()),
                permalink: args.permalink.clone(),
                surface: args.surface.as_slack_surface(),
                reaction_emoji: args.reaction_emoji.clone(),
                title: args.title.clone(),
                rationale: args.rationale.clone(),
                topic_keys: args.topic_keys.clone(),
                option_labels: args.option_labels.clone(),
                chosen_option_label: args.chosen_option_label.clone(),
                thread_text: args.thread_text.clone(),
            })?;
            format_json_value(cli.json, &event)
        }
        SlackAppCommand::Drain(_) => {
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let scoped_ledger = TenantScopedLedger::new(&ledger, cli_tenant(cli)?);
            let report = store.drain_queue(&scoped_ledger)?;
            format_json_value(cli.json, &report)
        }
        SlackAppCommand::Command(args) => {
            let tenant_id = cli_tenant(cli)?;
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let scoped_ledger = TenantScopedLedger::new(&ledger, tenant_id.clone());
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
            let response = handle_slack_command(
                &scoped_ledger,
                &graph,
                &store,
                &SlackCommandRequest {
                    team_id: args.team_id.clone(),
                    user_id: args.user_id.clone(),
                    text: args.text.clone(),
                    limit: args.limit,
                },
            )?;
            format_json_value(cli.json, &response)
        }
    }
}

fn run_emit(cli: &Cli, emit: &EmitArgs) -> Result<String> {
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        cli_command_context(cli, cli_emit_provenance(&cli.actor))?,
    );

    let output = match &emit.command {
        EmitCommand::DecisionCapture(args) => {
            let (actor_id, provenance) = capture_actor_and_provenance(&args.provenance)?;
            let commands =
                Commands::new_with_context(&ledger, cli_command_context(cli, provenance)?);
            let decision_id =
                propose_decision_from_option_labels(&commands, &actor_id, &args.decision)?;
            OutputEnvelope::new("emit", "decision_id", decision_id)
        }
        EmitCommand::DecisionProposed(args) => {
            let decision_id = propose_decision_from_option_labels(&commands, &cli.actor, args)?;
            OutputEnvelope::new("emit", "decision_id", decision_id)
        }
        EmitCommand::DecisionAccepted(args) => {
            let event_id = commands.accept_decision(&args.decision_id, &cli.actor)?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::DecisionRejected(args) => {
            let event_id = commands.reject_decision(&args.decision_id, &cli.actor)?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::DecisionSuperseded(args) => {
            let event_id = commands.supersede_decision(
                &args.old_decision_id,
                &args.new_decision_id,
                &cli.actor,
            )?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::EvidenceRecorded(args) => {
            let (actor_id, commands) = emit_actor_and_commands(cli, &ledger, &args.provenance)?;
            let evidence_id = commands.record_evidence(&actor_id, &args.content)?;
            OutputEnvelope::new("emit", "evidence_id", evidence_id)
        }
        EmitCommand::HypothesisRecorded(args) => {
            let (actor_id, commands) = emit_actor_and_commands(cli, &ledger, &args.provenance)?;
            let hypothesis_id = commands.record_hypothesis(&actor_id, &args.statement)?;
            OutputEnvelope::new("emit", "hypothesis_id", hypothesis_id)
        }
        EmitCommand::OptionRecorded(args) => {
            let option_id = commands.record_option(&cli.actor, &args.label, &args.description)?;
            OutputEnvelope::new("emit", "option_id", option_id)
        }
        EmitCommand::RelationAdded(args) => {
            let event_id = match args.kind {
                EmitRelationKind::BasedOn => {
                    commands.attach_evidence(&args.from_id, &args.to_id, &cli.actor)?
                }
                EmitRelationKind::Supports => commands.relate_evidence_to_hypothesis(
                    &args.from_id,
                    &args.to_id,
                    EventRelationKind::Supports,
                    &cli.actor,
                )?,
                EmitRelationKind::Refutes => commands.relate_evidence_to_hypothesis(
                    &args.from_id,
                    &args.to_id,
                    EventRelationKind::Refutes,
                    &cli.actor,
                )?,
            };

            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::AttachEvidence(args) => {
            let event_id =
                commands.attach_evidence(&args.decision_id, &args.evidence_id, &cli.actor)?;
            OutputEnvelope::new("emit", "event_id", event_id.to_string())
        }
        EmitCommand::IngestBatchClassified(args) => {
            let (actor_id, provenance) = capture_actor_and_provenance(&args.provenance)?;
            let commands =
                Commands::new_with_context(&ledger, cli_command_context(cli, provenance)?);
            let json_text = std::fs::read_to_string(&args.captures_file).map_err(|e| {
                CliError::InvalidInput(format!(
                    "cannot read captures file {:?}: {e}",
                    args.captures_file
                ))
            })?;
            let captures: Vec<CaptureItem> = serde_json::from_str(&json_text)
                .map_err(|e| CliError::InvalidInput(format!("captures JSON parse error: {e}")))?;
            let batch_id = Uuid::new_v4().to_string();
            let _event_id = commands.record_ingest_batch_classified(
                &actor_id,
                &batch_id,
                &args.classifier_model,
                &args.schema_version,
                captures,
                None,
            )?;
            OutputEnvelope::new("emit", "batch_id", batch_id)
        }
    };

    format_output(cli.json, &output)
}

fn run_disagree(cli: &Cli, args: &DisagreeArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::human(cli.actor.clone())),
    );
    let event_id = commands.disagree(&cli.actor, &args.decision_id, &args.reason)?;
    let decision_status = decision_status_after_write(&ledger, &tenant_id, &args.decision_id)?;

    format_disagree_output(
        cli.json,
        &DisagreeCommandOutput {
            decision_id: args.decision_id.clone(),
            event_id,
            decision_status,
        },
    )
}

fn run_supersede(cli: &Cli, args: &SupersedeArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::human(cli.actor.clone())),
    );
    let outcome = commands.supersede(SupersedeInput {
        actor_id: &cli.actor,
        old_decision_id: &args.old_decision_id,
        new_title: &args.title,
        new_rationale: &args.rationale,
        topic_keys: &args.topic_keys,
        option_labels: &args.option_labels,
        chosen_option_label: args.chosen_option_label.as_deref(),
        hypothesis_ids: &args.hypothesis_ids,
        evidence_ids: &args.evidence_ids,
    })?;
    let old_decision_status =
        decision_status_after_write(&ledger, &tenant_id, &args.old_decision_id)?;
    let new_decision_status =
        decision_status_after_write(&ledger, &tenant_id, &outcome.new_decision_id)?;

    format_supersede_output(
        cli.json,
        &SupersedeCommandOutput {
            old_decision_id: args.old_decision_id.clone(),
            new_decision_id: outcome.new_decision_id,
            proposal_event_id: outcome.proposal_event_id,
            relation_event_ids: outcome.relation_event_ids,
            superseded_event_id: outcome.superseded_event_id,
            old_decision_status,
            new_decision_status,
        },
    )
}

fn run_review(cli: &Cli, args: &ReviewArgs) -> Result<String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stderr = io::stderr();
    let mut prompt_output = stderr.lock();
    run_review_session(cli, args, &mut input, &mut prompt_output)
}

pub(crate) fn run_review_session<R: BufRead, W: IoWrite>(
    cli: &Cli,
    args: &ReviewArgs,
    input: &mut R,
    prompt_output: &mut W,
) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let scoped_ledger = TenantScopedLedger::new(&ledger, tenant_id.clone());
    let request = review_recent_decisions_request(args)?;
    let response = get_recent_decisions(&scoped_ledger, &request)?;
    let events = read_ledger_events(&scoped_ledger)?;
    let context = ReviewLedgerContext::from_events(&events)?;
    let reviewed_decision_ids = reviewed_decision_ids_by_actor(&events, &cli.actor)?;
    let mut items = response.data.items;

    if args.unreviewed_only {
        items.retain(|item| !reviewed_decision_ids.contains(&item.decision_id));
    }

    if items.is_empty() {
        writeln!(prompt_output, "No matching decisions to review.").map_err(cli_io_error)?;
        return format_review_output(
            cli.json,
            &ReviewCommandOutput {
                reviewer_actor_id: cli.actor.clone(),
                matched_count: 0,
                reviewed_count: 0,
                skipped_count: 0,
                quit: false,
                truncated: response.truncated,
                next_cursor: response.data.next_cursor,
                unreviewed_only: args.unreviewed_only,
                reviewed_semantics: REVIEWED_SEMANTICS,
                actions: Vec::new(),
            },
        );
    }

    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(tenant_id.clone(), EventProvenance::human(cli.actor.clone())),
    );
    let mut actions = Vec::new();
    let mut quit = false;
    let matched_count = items.len();

    for (index, item) in items.into_iter().enumerate() {
        render_review_item(prompt_output, index + 1, matched_count, &item, &context)?;

        loop {
            let Some(action) = prompt_line(
                input,
                prompt_output,
                "Action [a approve, d disagree, s supersede, n next, q quit]: ",
            )?
            else {
                quit = true;
                break;
            };
            match action.trim().to_ascii_lowercase().as_str() {
                "a" | "approve" => {
                    let event_id = commands.accept_decision(&item.decision_id, &cli.actor)?;
                    let status =
                        decision_status_after_write(&ledger, &tenant_id, &item.decision_id)?;
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "approved",
                        event_id: Some(event_id),
                        proposal_event_id: None,
                        superseded_event_id: None,
                        new_decision_id: None,
                        old_decision_status: Some(status),
                        new_decision_status: None,
                    });
                    break;
                }
                "d" | "disagree" => {
                    let Some(reason) = prompt_required_line(
                        input,
                        prompt_output,
                        "Disagreement reason: ",
                        "reason must not be empty",
                    )?
                    else {
                        quit = true;
                        break;
                    };
                    let event_id = commands.disagree(&cli.actor, &item.decision_id, &reason)?;
                    let status =
                        decision_status_after_write(&ledger, &tenant_id, &item.decision_id)?;
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "disagreed",
                        event_id: Some(event_id),
                        proposal_event_id: None,
                        superseded_event_id: None,
                        new_decision_id: None,
                        old_decision_status: Some(status),
                        new_decision_status: None,
                    });
                    break;
                }
                "s" | "supersede" => {
                    let Some(title) = prompt_required_line(
                        input,
                        prompt_output,
                        "New decision title: ",
                        "title must not be empty",
                    )?
                    else {
                        quit = true;
                        break;
                    };
                    let Some(rationale) = prompt_required_line(
                        input,
                        prompt_output,
                        "New decision rationale: ",
                        "rationale must not be empty",
                    )?
                    else {
                        quit = true;
                        break;
                    };
                    let option_labels = prompt_line(
                        input,
                        prompt_output,
                        "New option labels, comma-separated (blank for default): ",
                    )?
                    .map(|line| split_review_list(&line))
                    .unwrap_or_default();
                    let chosen_option_label = prompt_line(
                        input,
                        prompt_output,
                        "Chosen option label (blank for none): ",
                    )?
                    .and_then(|line| non_empty_owned(&line));

                    let outcome = commands.supersede(SupersedeInput {
                        actor_id: &cli.actor,
                        old_decision_id: &item.decision_id,
                        new_title: &title,
                        new_rationale: &rationale,
                        topic_keys: &item.topic_keys,
                        option_labels: &option_labels,
                        chosen_option_label: chosen_option_label.as_deref(),
                        hypothesis_ids: &item.hypothesis_ids,
                        evidence_ids: &item.evidence_ids,
                    })?;
                    let old_status =
                        decision_status_after_write(&ledger, &tenant_id, &item.decision_id)?;
                    let new_status =
                        decision_status_after_write(&ledger, &tenant_id, &outcome.new_decision_id)?;
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "superseded",
                        event_id: None,
                        proposal_event_id: Some(outcome.proposal_event_id),
                        superseded_event_id: Some(outcome.superseded_event_id),
                        new_decision_id: Some(outcome.new_decision_id),
                        old_decision_status: Some(old_status),
                        new_decision_status: Some(new_status),
                    });
                    break;
                }
                "" | "n" | "next" | "skip" => {
                    actions.push(ReviewActionOutput {
                        decision_id: item.decision_id,
                        action: "skipped",
                        event_id: None,
                        proposal_event_id: None,
                        superseded_event_id: None,
                        new_decision_id: None,
                        old_decision_status: Some(item.status),
                        new_decision_status: None,
                    });
                    break;
                }
                "q" | "quit" => {
                    quit = true;
                    break;
                }
                other => {
                    writeln!(
                        prompt_output,
                        "Unknown action '{other}'. Use a, d, s, n, or q."
                    )
                    .map_err(cli_io_error)?;
                }
            }
        }

        if quit {
            break;
        }
    }

    let reviewed_count = actions
        .iter()
        .filter(|action| action.action != "skipped")
        .count();
    let skipped_count = actions
        .iter()
        .filter(|action| action.action == "skipped")
        .count();

    format_review_output(
        cli.json,
        &ReviewCommandOutput {
            reviewer_actor_id: cli.actor.clone(),
            matched_count,
            reviewed_count,
            skipped_count,
            quit,
            truncated: response.truncated,
            next_cursor: response.data.next_cursor,
            unreviewed_only: args.unreviewed_only,
            reviewed_semantics: REVIEWED_SEMANTICS,
            actions,
        },
    )
}

fn run_import(cli: &Cli, import: &ImportArgs) -> Result<String> {
    match &import.command {
        ImportCommand::Documents(args) => {
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let scoped_ledger = TenantScopedLedger::new(&ledger, cli_tenant(cli)?);
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = import_documents(
                &scoped_ledger,
                &DocumentImportRequest {
                    paths,
                    importer_actor_id: cli.actor.clone(),
                    format: args.format.as_ingest_format(),
                    conflict_resolution: args.on_conflict.as_ingest_action(),
                },
            )?;
            format_import_output(cli.json, &report)
        }
        ImportCommand::PrepareDocuments(args) => {
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = prepare_document_texts(&DocumentPreparationRequest {
                paths,
                format: args.format.as_ingest_format(),
                output_dir: args.output_dir.clone(),
            })?;
            format_prepare_documents_output(cli.json, &report)
        }
        ImportCommand::Connector(args) => {
            use crate::connector::{
                confirm_same_as, find_connector_same_as_candidates, import_via_connector,
                retract_same_as, ConnectorImportRequest, GitFileConnector, GoogleDocsConnector,
                SameAsConfig,
            };
            let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
            let tenant_id = cli_tenant(cli)?;
            match &args.command {
                ImportConnectorCommand::Run(run_args) => {
                    let mut connectors: Vec<Box<dyn crate::connector::Connector>> =
                        vec![Box::new(GitFileConnector)];
                    if let Ok(Some(gdocs)) = GoogleDocsConnector::new(&cli.hivemind_dir) {
                        connectors.push(Box::new(gdocs));
                    }
                    let report = import_via_connector(
                        &ledger,
                        &tenant_id,
                        &ConnectorImportRequest {
                            url_or_id: run_args.url_or_id.clone(), // ubs:ignore: clone required to own from &ImportConnectorRunArgs
                            importer_actor_id: cli.actor.clone(), // ubs:ignore: clone required to own from &Cli
                            max_versions: run_args.max_versions,
                            import_run_id: None,
                        },
                        &connectors,
                    )?;
                    format_json_value(cli.json, &report)
                }
                ImportConnectorCommand::SameAsCandidates(ca) => {
                    let report = find_connector_same_as_candidates(
                        &ledger,
                        &tenant_id,
                        &ca.import_run_id,
                        &SameAsConfig::default(),
                    )?;
                    format_json_value(cli.json, &report)
                }
                ImportConnectorCommand::ConfirmSameAs(ca) => {
                    let report = confirm_same_as(
                        &ledger,
                        &tenant_id,
                        &ca.left_id,
                        &ca.right_id,
                        &cli.actor,
                    )?;
                    format_json_value(cli.json, &report)
                }
                ImportConnectorCommand::RetractSameAs(ra) => {
                    let report = retract_same_as(
                        &ledger,
                        &tenant_id,
                        &ra.left_id,
                        &ra.right_id,
                        &cli.actor,
                    )?;
                    format_json_value(cli.json, &report)
                }
            }
        }
    }
}

fn run_suggest(cli: &Cli, suggest: &SuggestArgs) -> Result<String> {
    match &suggest.command {
        SuggestCommand::DocumentCandidates(args) => {
            let mut paths = args.files.clone();
            paths.extend(args.paths.clone());
            let report = propose_document_extraction_candidates(&DocumentCandidateRequest {
                paths,
                format: args.format.as_ingest_format(),
                extractor: document_candidate_extractor(args)?,
            })?;
            format_json_value(cli.json, &report)
        }
        SuggestCommand::MaterializeDocumentCandidates(args) => {
            let report = materialize_document_extraction_candidates(
                &DocumentCandidateMaterializationRequest {
                    input: args.input.clone(),
                    candidate_ids: args.candidate_ids.clone(),
                    output: args.output.clone(),
                    reviewed_by: cli.actor.clone(),
                },
            )?;
            format_json_value(cli.json, &report)
        }
    }
}

fn document_candidate_extractor(
    args: &SuggestDocumentCandidatesArgs,
) -> Result<DocumentCandidateExtractor> {
    match (&args.extractor_command, &args.llm_response) {
        (Some(_), Some(_)) => Err(CliError::InvalidInput(
            "use either --extractor-command or --llm-response, not both".to_owned(),
        )
        .into()),
        (Some(_), None) => Ok(DocumentCandidateExtractor::Command {
            args: args.extractor_args.clone(),
        }),
        (None, Some(path)) => {
            if !args.extractor_args.is_empty() {
                return Err(CliError::InvalidInput(
                    "--extractor-arg requires --extractor-command".to_owned(),
                )
                .into());
            }
            Ok(DocumentCandidateExtractor::ResponseFile(path.clone()))
        }
        (None, None) => Err(CliError::InvalidInput(
            "document-candidates requires --extractor-command or --llm-response".to_owned(),
        )
        .into()),
    }
}

fn propose_decision_from_option_labels<L: EventLedger>(
    commands: &Commands<'_, L>,
    actor_id: &str,
    args: &EmitDecisionProposedArgs,
) -> Result<String> {
    let mut option_ids = Vec::with_capacity(args.option_ids.len());
    let mut chosen_option_id = None;
    for option_label in &args.option_ids {
        let mut option_description =
            String::with_capacity("Option generated from CLI value ''".len() + option_label.len());
        let _ = write!(
            option_description,
            "Option generated from CLI value '{option_label}'"
        );
        let option_id = commands.record_option(actor_id, option_label, &option_description)?;
        if args.chosen_option_id.as_deref() == Some(option_label.as_str()) {
            chosen_option_id = Some(option_id.clone());
        }
        option_ids.push(option_id);
    }

    if args.chosen_option_id.is_some() && chosen_option_id.is_none() {
        return Err(CliError::InvalidInput(
            "--chose must match one of the values passed to --options".to_owned(),
        )
        .into());
    }

    commands.propose_decision(DecisionProposalInput {
        actor_id,
        title: &args.title,
        rationale: &args.rationale,
        topic_keys: &args.topic_keys,
        option_ids: &option_ids,
        chosen_option_id: chosen_option_id.as_deref(),
        hypothesis_ids: &args.hypothesis_ids,
        evidence_ids: &args.evidence_ids,
    })
}

fn emit_actor_and_commands<'a>(
    cli: &Cli,
    ledger: &'a SqliteEventLedger,
    provenance_args: &EmitCaptureProvenanceArgs,
) -> Result<(String, Commands<'a, SqliteEventLedger>)> {
    if !provenance_args.has_override() {
        let commands = Commands::new_with_context(
            ledger,
            cli_command_context(cli, cli_emit_provenance(&cli.actor))?,
        );
        return Ok((cli.actor.clone(), commands));
    }

    let (actor_id, provenance) = capture_actor_and_provenance(provenance_args)?;
    let commands = Commands::new_with_context(ledger, cli_command_context(cli, provenance)?);
    Ok((actor_id, commands))
}

fn capture_actor_and_provenance(
    args: &EmitCaptureProvenanceArgs,
) -> Result<(String, EventProvenance)> {
    let actor_id = capture_actor_id(args)?;
    let provenance = capture_provenance(args, &actor_id)?;
    Ok((actor_id, provenance))
}

fn capture_actor_id(args: &EmitCaptureProvenanceArgs) -> Result<String> {
    if let Some(actor_id) = trimmed_optional("--actor-id", &args.actor_id)? {
        return Ok(actor_id.to_owned());
    }

    match args.source.unwrap_or(DecisionCaptureSource::Agent) {
        DecisionCaptureSource::Agent => {
            let tool = capture_agent_tool(args)?;
            let session = capture_agent_session(args, &tool)?;
            Ok(agent_actor_id(&tool, &session))
        }
        DecisionCaptureSource::Human => Ok(default_human_actor_id()),
    }
}

fn capture_agent_tool(args: &EmitCaptureProvenanceArgs) -> Result<String> {
    trimmed_optional("--agent-tool", &args.agent_tool).map(|value| {
        value
            .map(ToOwned::to_owned)
            .unwrap_or_else(default_agent_tool)
    })
}

fn capture_agent_session(args: &EmitCaptureProvenanceArgs, tool: &str) -> Result<String> {
    trimmed_optional("--agent-session", &args.agent_session).map(|value| {
        value
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_agent_session(tool))
    })
}

fn capture_provenance(args: &EmitCaptureProvenanceArgs, actor_id: &str) -> Result<EventProvenance> {
    let source = args.source.unwrap_or(DecisionCaptureSource::Agent);
    if let Some(source_ref) = trimmed_optional("--source-ref", &args.source_ref)? {
        return Ok(match source {
            DecisionCaptureSource::Agent => EventProvenance::agent(source_ref),
            DecisionCaptureSource::Human => EventProvenance::human(source_ref),
        });
    }

    Ok(match source {
        DecisionCaptureSource::Agent => EventProvenance::agent(actor_id),
        DecisionCaptureSource::Human => EventProvenance::human(actor_id),
    })
}

fn cli_emit_provenance(actor_id: &str) -> EventProvenance {
    if actor_id.trim().starts_with("human:") {
        EventProvenance::human(actor_id.trim().to_owned())
    } else {
        EventProvenance::cli()
    }
}

fn trimmed_required<'a>(field: &'static str, value: &'a str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(CliError::InvalidInput(format!("{field} must not be empty")).into())
    } else {
        Ok(trimmed)
    }
}

fn trimmed_optional<'a>(field: &'static str, value: &'a Option<String>) -> Result<Option<&'a str>> {
    match value.as_deref() {
        Some(raw) => Ok(Some(trimmed_required(field, raw)?)),
        None => Ok(None),
    }
}

fn run_query(cli: &Cli, query: &QueryArgs) -> Result<String> {
    let context = cli_query_context(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    if query.command.is_ledger_history_query() {
        let scoped_ledger = TenantScopedLedger::new(&ledger, context.tenant_id.clone());
        return run_query_with_ledger(&scoped_ledger, query);
    }

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &context.tenant_id, &graph)?;
            run_query_with_graph(&context, &ledger, &graph, query)
        }
        GraphBackend::Kuzu => run_query_with_kuzu(&context, &ledger, &cli.hivemind_dir, query),
    }
}

impl QueryCommand {
    fn is_ledger_history_query(&self) -> bool {
        matches!(
            self,
            QueryCommand::RecentDecisions(_)
                | QueryCommand::GetRecentActivity(_)
                | QueryCommand::GetDecisionsChangedSince(_)
                | QueryCommand::GetDecisionsAddedSince(_)
                | QueryCommand::ExportReadOnlySummary(_)
        )
    }
}

fn run_query_with_ledger(ledger: &impl EventLedger, query: &QueryArgs) -> Result<String> {
    let output = match &query.command {
        QueryCommand::RecentDecisions(args) => {
            let response = get_recent_decisions(ledger, &recent_decisions_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_recent_decisions_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetRecentActivity(args) => {
            let response = get_recent_activity(ledger, &recent_activity_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_recent_activity_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetDecisionsChangedSince(args) => {
            let response = get_decisions_changed_since(ledger, &changed_since_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_changed_since_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetDecisionsAddedSince(args) => {
            let response = get_decisions_added_since(ledger, &added_since_request(args)?)?;
            format_query_response(
                query.summary,
                &response,
                render_added_since_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::ExportReadOnlySummary(args) => {
            let request = export_read_only_summary_request(args)?;
            let response = export_read_only_summary(ledger, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_read_only_export_summary,
                response.data.continuation_cursor.as_deref(),
            )?
        }
        QueryCommand::GetDecision(_)
        | QueryCommand::GetRelevantDecisions(_)
        | QueryCommand::GetSupersessionChain(_)
        | QueryCommand::GetDecisionNeighborhood(_)
        | QueryCommand::GetCompactView(_)
        | QueryCommand::Search(_)
        | QueryCommand::SearchDecisions(_)
        | QueryCommand::Recall(_)
        | QueryCommand::GetActiveDecisionBlockers(_)
        | QueryCommand::GetBlockerNotificationCandidates(_) => {
            return Err(
                CliError::InvalidInput("query requires graph-backed execution".to_owned()).into(),
            )
        }
    };

    Ok(output)
}

const REVIEWED_SEMANTICS: &str =
    "derived from reviewer-authored decision.accepted, decision.rejected, or decision.superseded events";

pub(crate) fn review_recent_decisions_request(args: &ReviewArgs) -> Result<RecentDecisionsRequest> {
    let now = parse_utc_timestamp("--now", &args.now)?;
    let timezone = TimeZoneSpec::parse(&args.timezone)?;
    let since_timestamp =
        resolve_diff_bound("--since", Some(args.since.as_str()), None, now, timezone)?
            .ok_or_else(|| CliError::InvalidInput("--since must not be empty".to_owned()))?;
    let until_timestamp =
        resolve_diff_bound("--until", args.until.as_deref(), None, now, timezone)?;

    Ok(RecentDecisionsRequest {
        since_timestamp,
        until_timestamp,
        filters: RecentDecisionFilterRequest {
            actor_patterns: args.actor_patterns.clone(),
            sources: Vec::new(),
            topic_keys: Vec::new(),
            statuses: Vec::new(),
        },
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn read_ledger_events(ledger: &impl EventLedger) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    ledger.replay_from(0, &mut |event| {
        events.push(event.clone());
        Ok(())
    })?;
    Ok(events)
}

fn reviewed_decision_ids_by_actor(
    events: &[Event],
    reviewer_actor_id: &str,
) -> Result<BTreeSet<String>> {
    let mut reviewed = BTreeSet::new();
    for event in events
        .iter()
        .filter(|event| event.actor_id == reviewer_actor_id)
    {
        match validated_payload(event)? {
            EventPayload::DecisionAccepted(payload) => {
                reviewed.insert(payload.decision_id);
            }
            EventPayload::DecisionRejected(payload) => {
                reviewed.insert(payload.decision_id);
            }
            EventPayload::DecisionSuperseded(payload) => {
                reviewed.insert(payload.old_decision_id);
            }
            EventPayload::DecisionProposed(_)
            | EventPayload::DecisionRequested(_)
            | EventPayload::EvidenceRecorded(_)
            | EventPayload::HypothesisRecorded(_)
            | EventPayload::RelationAdded(_)
            | EventPayload::RelationRemoved(_)
            | EventPayload::BlockerReported(_)
            | EventPayload::BlockerResolved(_)
            | EventPayload::NotificationSent(_)
            | EventPayload::NotificationAcknowledged(_)
            | EventPayload::IngestBatchReceived(_)
            | EventPayload::IngestBatchClassified(_)
            | EventPayload::DecisionScored(_)
            | EventPayload::DecisionMetadataDerived(_) => {}
        }
    }
    Ok(reviewed)
}

#[derive(Debug, Default)]
struct ReviewLedgerContext {
    evidence: BTreeMap<String, String>,
    hypotheses: BTreeMap<String, String>,
}

impl ReviewLedgerContext {
    fn from_events(events: &[Event]) -> Result<Self> {
        let mut context = Self::default();
        for event in events {
            match validated_payload(event)? {
                EventPayload::EvidenceRecorded(payload) => {
                    context
                        .evidence
                        .insert(payload.evidence_id, payload.content);
                }
                EventPayload::HypothesisRecorded(payload) => {
                    context
                        .hypotheses
                        .insert(payload.hypothesis_id, payload.statement);
                }
                EventPayload::DecisionProposed(_)
                | EventPayload::DecisionRequested(_)
                | EventPayload::DecisionAccepted(_)
                | EventPayload::DecisionRejected(_)
                | EventPayload::DecisionSuperseded(_)
                | EventPayload::RelationAdded(_)
                | EventPayload::RelationRemoved(_)
                | EventPayload::BlockerReported(_)
                | EventPayload::BlockerResolved(_)
                | EventPayload::NotificationSent(_)
                | EventPayload::NotificationAcknowledged(_)
                | EventPayload::IngestBatchReceived(_)
                | EventPayload::IngestBatchClassified(_)
                | EventPayload::DecisionScored(_)
                | EventPayload::DecisionMetadataDerived(_) => {}
            }
        }
        Ok(context)
    }
}

fn render_review_item<W: IoWrite>(
    output: &mut W,
    index: usize,
    total: usize,
    item: &RecentDecisionEntry,
    context: &ReviewLedgerContext,
) -> Result<()> {
    writeln!(output, "\n[{index}/{total}] {}", item.decision_id).map_err(cli_io_error)?;
    writeln!(output, "Title: {}", item.title).map_err(cli_io_error)?;
    writeln!(output, "Status: {}", decision_status_label(item.status)).map_err(cli_io_error)?;
    writeln!(output, "Actors: {}", display_review_list(&item.actor_ids)).map_err(cli_io_error)?;
    writeln!(output, "Topics: {}", display_review_list(&item.topic_keys)).map_err(cli_io_error)?;
    writeln!(output, "Rationale: {}", item.rationale).map_err(cli_io_error)?;
    writeln!(output, "Options:").map_err(cli_io_error)?;
    if item.option_ids.is_empty() {
        writeln!(output, "  - <none>").map_err(cli_io_error)?;
    } else {
        for option_id in &item.option_ids {
            if item.chosen_option_id.as_deref() == Some(option_id.as_str()) {
                writeln!(output, "  - {option_id} (chosen)").map_err(cli_io_error)?;
            } else {
                writeln!(output, "  - {option_id}").map_err(cli_io_error)?;
            }
        }
    }
    writeln!(output, "Evidence:").map_err(cli_io_error)?;
    if item.evidence_ids.is_empty() {
        writeln!(output, "  - <none>").map_err(cli_io_error)?;
    } else {
        for evidence_id in &item.evidence_ids {
            match context.evidence.get(evidence_id) {
                Some(content) => {
                    writeln!(output, "  - {evidence_id}: {content}").map_err(cli_io_error)?
                }
                None => writeln!(output, "  - {evidence_id}").map_err(cli_io_error)?,
            }
        }
    }
    writeln!(output, "Hypotheses:").map_err(cli_io_error)?;
    if item.hypothesis_ids.is_empty() {
        writeln!(output, "  - <none>").map_err(cli_io_error)?;
    } else {
        for hypothesis_id in &item.hypothesis_ids {
            match context.hypotheses.get(hypothesis_id) {
                Some(statement) => {
                    writeln!(output, "  - {hypothesis_id}: {statement}").map_err(cli_io_error)?
                }
                None => writeln!(output, "  - {hypothesis_id}").map_err(cli_io_error)?,
            }
        }
    }
    Ok(())
}

fn prompt_line<R: BufRead, W: IoWrite>(
    input: &mut R,
    output: &mut W,
    prompt: &str,
) -> Result<Option<String>> {
    write!(output, "{prompt}").map_err(cli_io_error)?;
    output.flush().map_err(cli_io_error)?;
    let mut line = String::new();
    let bytes_read = input.read_line(&mut line).map_err(cli_io_error)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_owned()))
}

fn prompt_required_line<R: BufRead, W: IoWrite>(
    input: &mut R,
    output: &mut W,
    prompt: &str,
    empty_message: &str,
) -> Result<Option<String>> {
    loop {
        let Some(value) = prompt_line(input, output, prompt)? else {
            return Ok(None);
        };
        if let Some(value) = non_empty_owned(&value) {
            return Ok(Some(value));
        }
        writeln!(output, "{empty_message}").map_err(cli_io_error)?;
    }
}

fn split_review_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(non_empty_owned)
        .collect::<Vec<_>>()
}

fn non_empty_owned(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn display_review_list(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_owned()
    } else {
        values.join(",")
    }
}

fn validated_payload(event: &Event) -> Result<EventPayload> {
    crate::events::validate(event).map_err(|error| {
        CliError::InvalidInput(format!(
            "ledger event {} failed validation during review: {error}",
            event.event_id.unwrap_or_default()
        ))
        .into()
    })
}

fn cli_io_error(error: io::Error) -> HivemindError {
    CliError::InvalidInput(format!("interactive review I/O failed: {error}")).into()
}

pub(crate) fn added_since_request(
    args: &QueryAddedSinceArgs,
) -> Result<DecisionsAddedSinceRequest> {
    let now = parse_utc_timestamp("--now", &args.now)?;
    let timezone = TimeZoneSpec::parse(&args.timezone)?;
    let since_timestamp = resolve_diff_bound(
        "--since",
        args.since.as_deref(),
        args.since_timestamp.as_deref(),
        now,
        timezone,
    )?;
    let until_timestamp = resolve_diff_bound(
        "--until",
        args.until.as_deref(),
        args.until_timestamp.as_deref(),
        now,
        timezone,
    )?;

    Ok(DecisionsAddedSinceRequest {
        since_offset: args.since_offset,
        since_timestamp,
        until_offset: args.until_offset,
        until_timestamp,
        filters: DecisionsAddedSinceFilterRequest {
            actor_ids: args.filters.actor_ids.clone(),
            sources: args.filters.sources.clone(),
            source_refs: args.filters.source_refs.clone(),
            import_run_ids: args.import_run_ids.clone(),
            topic_keys: args.filters.topic_keys.clone(),
            statuses: args
                .filters
                .statuses
                .iter()
                .copied()
                .map(QueryDecisionStatus::as_decision_status)
                .collect(),
        },
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

pub(crate) fn recent_decisions_request(
    args: &QueryRecentDecisionsArgs,
) -> Result<RecentDecisionsRequest> {
    let now = parse_utc_timestamp("--now", &args.now)?;
    let timezone = TimeZoneSpec::parse(&args.timezone)?;
    let since_timestamp =
        resolve_diff_bound("--since", Some(args.since.as_str()), None, now, timezone)?
            .ok_or_else(|| CliError::InvalidInput("--since must not be empty".to_owned()))?;
    let until_timestamp =
        resolve_diff_bound("--until", args.until.as_deref(), None, now, timezone)?;

    Ok(RecentDecisionsRequest {
        since_timestamp,
        until_timestamp,
        filters: RecentDecisionFilterRequest {
            actor_patterns: args.actor_patterns.clone(),
            sources: args.sources.clone(),
            topic_keys: args.topic_keys.clone(),
            statuses: args
                .statuses
                .iter()
                .copied()
                .map(QueryDecisionStatus::as_decision_status)
                .collect(),
        },
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TimeZoneSpec {
    Utc,
}

impl TimeZoneSpec {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "UTC" | "utc" | "Etc/UTC" => Ok(Self::Utc),
            other => Err(CliError::InvalidInput(format!(
                "--timezone {other} is not supported in slice 1; only UTC is accepted"
            ))
            .into()),
        }
    }
}

pub(crate) fn resolve_diff_bound(
    flag: &'static str,
    raw: Option<&str>,
    explicit_ts: Option<&str>,
    now: Option<DateTime<Utc>>,
    timezone: TimeZoneSpec,
) -> Result<Option<DateTime<Utc>>> {
    if let Some(ts) = explicit_ts {
        return parse_utc_timestamp(flag, &Some(ts.to_owned()));
    }
    let Some(value) = raw.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(Some(parsed.with_timezone(&Utc)));
    }
    if let Some(parsed) = parse_utc_date(value) {
        return Ok(Some(parsed));
    }
    let now = now.unwrap_or_else(Utc::now);
    if let Some(resolved) = resolve_relative_duration(value, now) {
        return Ok(Some(resolved));
    }
    let resolved = resolve_relative_phrase(value, now, timezone).ok_or_else(|| {
        CliError::InvalidInput(format!(
            "{flag} must be an RFC3339 timestamp, YYYY-MM-DD date, duration like 7d/24h, or supported phrase (got: {value})"
        ))
    })?;
    Ok(Some(resolved))
}

fn parse_utc_date(value: &str) -> Option<DateTime<Utc>> {
    use chrono::{NaiveDate, NaiveTime, TimeZone};
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .map(|date| Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN)))
}

fn resolve_relative_duration(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (amount, unit) = value.split_at(value.len().checked_sub(1)?);
    let amount = amount.parse::<i64>().ok()?;
    if amount < 0 {
        return None;
    }
    let duration = match unit.to_ascii_lowercase().as_str() {
        "s" => chrono::Duration::seconds(amount),
        "m" => chrono::Duration::minutes(amount),
        "h" => chrono::Duration::hours(amount),
        "d" => chrono::Duration::days(amount),
        "w" => chrono::Duration::weeks(amount),
        _ => return None,
    };
    now.checked_sub_signed(duration)
}

fn resolve_relative_phrase(
    phrase: &str,
    now: DateTime<Utc>,
    timezone: TimeZoneSpec,
) -> Option<DateTime<Utc>> {
    let normalized = phrase.trim().to_ascii_lowercase();
    let TimeZoneSpec::Utc = timezone;
    match normalized.as_str() {
        "now" => Some(now),
        "last week" | "last_week" | "last-week" => Some(start_of_previous_iso_week_utc(now)),
        "this week" | "this_week" | "this-week" => Some(start_of_current_iso_week_utc(now)),
        "yesterday" => Some(start_of_day_utc(now) - chrono::Duration::days(1)),
        "today" => Some(start_of_day_utc(now)),
        _ => None,
    }
}

fn start_of_day_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::NaiveTime;
    use chrono::TimeZone;
    Utc.from_utc_datetime(&now.date_naive().and_time(NaiveTime::MIN))
}

fn start_of_current_iso_week_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::{Datelike, NaiveTime, TimeZone};
    let date = now.date_naive();
    let days_from_monday = i64::from(date.weekday().num_days_from_monday());
    let monday = date - chrono::Duration::days(days_from_monday);
    Utc.from_utc_datetime(&monday.and_time(NaiveTime::MIN))
}

fn start_of_previous_iso_week_utc(now: DateTime<Utc>) -> DateTime<Utc> {
    start_of_current_iso_week_utc(now) - chrono::Duration::days(7)
}

fn recent_activity_request(args: &QueryRecentActivityArgs) -> Result<RecentActivityRequest> {
    Ok(RecentActivityRequest {
        filters: history_filter_request(&args.filters),
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn changed_since_request(args: &QueryChangedSinceArgs) -> Result<ChangedSinceRequest> {
    Ok(ChangedSinceRequest {
        since_offset: args.since_offset,
        since_timestamp: parse_utc_timestamp("--since-ts", &args.since_timestamp)?,
        until_offset: args.until_offset,
        until_timestamp: parse_utc_timestamp("--until-ts", &args.until_timestamp)?,
        filters: history_filter_request(&args.filters),
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn export_read_only_summary_request(
    args: &QueryExportReadOnlySummaryArgs,
) -> Result<ReadOnlyExportRequest> {
    let generated_at =
        parse_utc_timestamp("--generated-at", &args.generated_at)?.unwrap_or_else(Utc::now);
    let filters = history_filter_request(&args.filters);
    let query = match args.query {
        QueryExportKind::RecentActivity => {
            ReadOnlyExportQuery::RecentActivity(RecentActivityRequest {
                filters,
                limit: args.limit,
                cursor: args.cursor.clone(),
            })
        }
        QueryExportKind::DecisionsChangedSince => {
            ReadOnlyExportQuery::DecisionsChangedSince(ChangedSinceRequest {
                since_offset: args.since_offset,
                since_timestamp: parse_utc_timestamp("--since-ts", &args.since_timestamp)?,
                until_offset: args.until_offset,
                until_timestamp: parse_utc_timestamp("--until-ts", &args.until_timestamp)?,
                filters,
                limit: args.limit,
                cursor: args.cursor.clone(),
            })
        }
    };

    Ok(ReadOnlyExportRequest {
        query,
        format: args.format.as_query_format(),
        generated_at,
    })
}

fn history_filter_request(args: &QueryHistoryFilterArgs) -> HistoryFilterRequest {
    HistoryFilterRequest {
        actor_ids: args.actor_ids.clone(),
        sources: args.sources.clone(),
        source_refs: args.source_refs.clone(),
        topic_keys: args.topic_keys.clone(),
        statuses: args
            .statuses
            .iter()
            .copied()
            .map(QueryDecisionStatus::as_decision_status)
            .collect(),
    }
}

fn parse_utc_timestamp(
    field: &'static str,
    value: &Option<String>,
) -> Result<Option<DateTime<Utc>>> {
    match value.as_deref() {
        None => Ok(None),
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
            .map_err(|error| {
                CliError::InvalidInput(format!("{field} must be an RFC 3339 timestamp: {error}"))
                    .into()
            }),
    }
}

fn run_query_with_graph(
    context: &QueryContext,
    ledger: &SqliteEventLedger,
    graph: &impl GraphView,
    query: &QueryArgs,
) -> Result<String> {
    let output = match &query.command {
        QueryCommand::GetDecision(args) => {
            let response = get_decision(graph, &args.decision_id)?;
            format_query_response(query.summary, &response, render_decision_summary, None)?
        }
        QueryCommand::GetRelevantDecisions(args) => {
            let response = get_relevant_decisions(
                graph,
                &args.topic,
                args.status.map(QueryDecisionStatus::as_decision_status),
            )?;
            format_query_response(
                query.summary,
                &response,
                |decisions| render_decision_list_summary(decisions),
                None,
            )?
        }
        QueryCommand::GetSupersessionChain(args) => {
            let response = get_supersession_chain(graph, &args.decision_id)?;
            format_query_response(query.summary, &response, render_supersession_summary, None)?
        }
        QueryCommand::GetDecisionNeighborhood(args) => {
            if args.compact {
                let response = get_compact_view(graph, &args.decision_id)?;
                format_query_response(query.summary, &response, render_compact_view_summary, None)?
            } else {
                if args.depth != 1 {
                    return Err(CliError::InvalidInput(format!(
                        "--depth {} is not supported yet; slice-1 only supports depth=1 with hypothesis SUPPORTS/REFUTES auto-expanded",
                        args.depth
                    ))
                    .into());
                }
                let request = if args.relations.is_empty() {
                    NeighborhoodRequest::all()
                } else {
                    NeighborhoodRequest::with_relations(
                        args.relations
                            .iter()
                            .copied()
                            .map(QueryRelationKind::as_graph_relation),
                    )
                };
                let response = get_decision_neighborhood(graph, &args.decision_id, &request)?;
                format_query_response(query.summary, &response, render_neighborhood_summary, None)?
            }
        }
        QueryCommand::GetCompactView(args) => {
            let response = get_compact_view(graph, &args.decision_id)?;
            format_query_response(query.summary, &response, render_compact_view_summary, None)?
        }
        QueryCommand::Search(args) => {
            let request = search_decision_request(args)?;
            let response = search_decisions_fts_with_context(context, ledger, graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_search_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::SearchDecisions(args) => {
            let request = search_decision_request(args)?;
            let response = search_decisions_fts_with_context(context, ledger, graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_search_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::Recall(args) => {
            let limit = args.limit.clamp(1, RECALL_MAX_LIMIT);
            let request = RecallRequest {
                q: args.query.clone(),
                topic_keys: args.topic_keys.clone(),
                statuses: args
                    .statuses
                    .iter()
                    .copied()
                    .map(QueryDecisionStatus::as_decision_status)
                    .collect(),
                actor_ids: args.actor_ids.clone(),
                sources: args.sources.clone(),
                since: parse_query_datetime(args.since.as_deref(), "--since")?,
                until: parse_query_datetime(args.until.as_deref(), "--until")?,
                limit,
                cursor: args.cursor.clone(),
            };
            let response = recall_decisions(context, ledger, graph, &request)?;
            format_query_response(query.summary, &response, render_recall_summary, None)?
        }
        QueryCommand::GetActiveDecisionBlockers(args) => {
            let request = ActiveDecisionBlockersRequest {
                filters: DecisionBlockerFilters {
                    decision_ids: args.decision_ids.clone(),
                    topic_keys: args.topic_keys.clone(),
                    required_owner_ids: args.required_owner_ids.clone(),
                    blocked_actor_ids: args.blocked_actor_ids.clone(),
                    priorities: args
                        .priorities
                        .iter()
                        .copied()
                        .map(QueryBlockerPriority::as_blocker_priority)
                        .collect(),
                    now: parse_query_datetime(args.now.as_deref(), "--now")?,
                    stale_after_seconds: args.stale_after_seconds,
                },
                limit: args.limit,
                cursor: args.cursor.clone(),
            };
            let response = get_active_decision_blockers(graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_active_blockers_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::GetBlockerNotificationCandidates(args) => {
            let request = BlockerNotificationCandidatesRequest {
                now: parse_required_query_datetime(&args.now, "--now")?,
                policy_version: args.policy_version.clone(),
                limit: args.limit,
                cursor: args.cursor.clone(),
            };
            let response = get_blocker_notification_candidates(graph, &request)?;
            format_query_response(
                query.summary,
                &response,
                render_blocker_notifications_summary,
                response.data.next_cursor.as_deref(),
            )?
        }
        QueryCommand::RecentDecisions(_)
        | QueryCommand::GetRecentActivity(_)
        | QueryCommand::GetDecisionsChangedSince(_)
        | QueryCommand::GetDecisionsAddedSince(_)
        | QueryCommand::ExportReadOnlySummary(_) => {
            return Err(
                CliError::InvalidInput("query requires ledger-backed execution".to_owned()).into(),
            )
        }
    };

    Ok(output)
}

fn parse_query_datetime(value: Option<&str>, flag: &str) -> Result<Option<DateTime<Utc>>> {
    value
        .map(|value| parse_required_query_datetime(value, flag))
        .transpose()
}

fn search_decision_request(args: &QuerySearchDecisionsArgs) -> Result<SearchDecisionRequest> {
    Ok(SearchDecisionRequest {
        query: args.query.clone(),
        topic_keys: args.topic_keys.clone(),
        statuses: args
            .statuses
            .iter()
            .copied()
            .map(QueryDecisionStatus::as_decision_status)
            .collect(),
        actor_ids: args.actor_ids.clone(),
        sources: args.sources.clone(),
        since: parse_query_datetime(args.since.as_deref(), "--since")?,
        until: parse_query_datetime(args.until.as_deref(), "--until")?,
        limit: args.limit,
        cursor: args.cursor.clone(),
    })
}

fn parse_required_query_datetime(value: &str, flag: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            CliError::InvalidInput(format!("{flag} must be an RFC3339 timestamp: {error}")).into()
        })
}

fn run_dump(cli: &Cli, dump: &DumpArgs) -> Result<String> {
    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
            run_dump_with_graph(&graph, dump)
        }
        GraphBackend::Kuzu => run_dump_with_kuzu(&tenant_id, &ledger, &cli.hivemind_dir, dump),
    }
}

#[cfg(feature = "tui")]
fn run_tui(cli: &Cli, args: &TuiArgs) -> Result<String> {
    if cli.json {
        return Err(CliError::InvalidInput(
            "--json is not supported for the interactive tui command".to_owned(),
        )
        .into());
    }

    let tenant_id = cli_tenant(cli)?;
    let ledger = SqliteEventLedger::open(&cli.hivemind_dir)?;
    let config = crate::tui::TuiConfig {
        query: args.query.clone(),
        topic_keys: args.topic_keys.clone(),
        statuses: args
            .statuses
            .iter()
            .copied()
            .map(QueryDecisionStatus::as_decision_status)
            .collect(),
        actor_ids: args.actor_ids.clone(),
        sources: args.sources.clone(),
        limit: args.limit,
        dot_output: args.dot_output.clone(),
    };

    match selected_graph_backend(cli)? {
        GraphBackend::Memory => {
            let graph = MemoryGraph::default();
            rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
            crate::tui::run(&graph, config)?;
        }
        GraphBackend::Kuzu => run_tui_with_kuzu(&tenant_id, &ledger, &cli.hivemind_dir, config)?,
    }

    Ok("tui exited".to_owned())
}

#[cfg(not(feature = "tui"))]
fn run_tui(_cli: &Cli, _args: &TuiArgs) -> Result<String> {
    Err(
        CliError::InvalidInput("tui command requires building with --features tui".to_owned())
            .into(),
    )
}

#[cfg(all(feature = "tui", feature = "graph-kuzu"))]
fn run_tui_with_kuzu(
    tenant_id: &TenantId,
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    config: crate::tui::TuiConfig,
) -> Result<()> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    crate::tui::run(&graph, config)
}

#[cfg(all(feature = "tui", not(feature = "graph-kuzu")))]
fn run_tui_with_kuzu(
    _tenant_id: &TenantId,
    _ledger: &impl EventLedger,
    _hivemind_dir: &std::path::Path,
    _config: crate::tui::TuiConfig,
) -> Result<()> {
    Err(CliError::InvalidInput(
        "graph backend 'kuzu' requires building with --features graph-kuzu".to_owned(),
    )
    .into())
}

fn run_dump_with_graph(graph: &impl GraphView, dump: &DumpArgs) -> Result<String> {
    match dump.format {
        DumpFormat::Dot => render_dot(graph),
    }
}

#[cfg(feature = "graph-kuzu")]
fn run_query_with_kuzu(
    context: &QueryContext,
    ledger: &SqliteEventLedger,
    hivemind_dir: &std::path::Path,
    query: &QueryArgs,
) -> Result<String> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph_for_tenant(ledger, &context.tenant_id, &graph)?;
    run_query_with_graph(context, ledger, &graph, query)
}

#[cfg(not(feature = "graph-kuzu"))]
fn run_query_with_kuzu(
    _context: &QueryContext,
    _ledger: &SqliteEventLedger,
    _hivemind_dir: &std::path::Path,
    _query: &QueryArgs,
) -> Result<String> {
    Err(CliError::InvalidInput(
        "graph backend 'kuzu' requires building with --features graph-kuzu".to_owned(),
    )
    .into())
}

#[cfg(feature = "graph-kuzu")]
fn run_dump_with_kuzu(
    tenant_id: &TenantId,
    ledger: &impl EventLedger,
    hivemind_dir: &std::path::Path,
    dump: &DumpArgs,
) -> Result<String> {
    let graph = crate::projector::kuzu::KuzuGraph::open(hivemind_dir)?;
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    run_dump_with_graph(&graph, dump)
}

#[cfg(not(feature = "graph-kuzu"))]
fn run_dump_with_kuzu(
    _tenant_id: &TenantId,
    _ledger: &impl EventLedger,
    _hivemind_dir: &std::path::Path,
    _dump: &DumpArgs,
) -> Result<String> {
    Err(CliError::InvalidInput(
        "graph backend 'kuzu' requires building with --features graph-kuzu".to_owned(),
    )
    .into())
}

fn decision_status_after_write(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    decision_id: &str,
) -> Result<DecisionStatus> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    derive_decision_status(&graph, decision_id)
}

fn parse_actor_mappings(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut mappings = BTreeMap::new();
    for value in values {
        let (slack_user, actor_id) = value.split_once('=').ok_or_else(|| {
            CliError::InvalidInput(
                "--actor-map must use SlackUser=HiveMindActorId format".to_owned(),
            )
        })?;
        let slack_user = trimmed_required("--actor-map Slack user", slack_user)?;
        let actor_id = trimmed_required("--actor-map actor id", actor_id)?;
        mappings.insert(slack_user.to_owned(), actor_id.to_owned());
    }
    Ok(mappings)
}

fn validate_global_flags(cli: &Cli) -> Result<()> {
    if cli.actor.trim().is_empty() {
        return Err(CliError::InvalidInput("--actor must not be empty".to_owned()).into());
    }
    cli_tenant(cli)?;

    Ok(())
}

pub(crate) fn cli_tenant(cli: &Cli) -> Result<TenantId> {
    TenantId::new(cli.tenant.trim().to_owned())
        .map_err(|error| CliError::InvalidInput(format!("--tenant is invalid: {error}")).into())
}

fn cli_command_context(cli: &Cli, provenance: EventProvenance) -> Result<CommandContext> {
    Ok(CommandContext::new(cli_tenant(cli)?, provenance))
}

fn cli_query_context(cli: &Cli) -> Result<QueryContext> {
    Ok(QueryContext::new(cli_tenant(cli)?))
}

fn selected_graph_backend(cli: &Cli) -> Result<GraphBackend> {
    if let Some(backend) = cli.graph_backend {
        return Ok(backend);
    }

    match std::env::var("HIVEMIND_GRAPH_BACKEND") {
        Ok(value) => parse_graph_backend(&value),
        Err(std::env::VarError::NotPresent) => Ok(GraphBackend::Memory),
        Err(error) => Err(CliError::InvalidInput(format!(
            "HIVEMIND_GRAPH_BACKEND is not valid unicode: {error}"
        ))
        .into()),
    }
}

pub(crate) fn parse_graph_backend(value: &str) -> Result<GraphBackend> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "memory" | "in-memory" | "in_memory" => Ok(GraphBackend::Memory),
        "kuzu" | "graph-kuzu" | "graph_kuzu" => Ok(GraphBackend::Kuzu),
        other => Err(CliError::InvalidInput(format!(
            "unknown graph backend '{other}'; expected 'memory' or 'kuzu'"
        ))
        .into()),
    }
}

/// Public DOT renderer for callers outside the CLI module (e.g. the MCP
/// server). Delegates to the same internal implementation `hivemind dump`
/// uses so output stays identical across transports.

#[derive(Debug, Serialize)]
struct QuickstartReport {
    ledger_dir: String,
    actor_id: String,
    decision_id: String,
    query: QuickstartQueryReport,
}

#[derive(Debug, Serialize)]
struct QuickstartQueryReport {
    result_count: usize,
    total_matches: usize,
    truncated: bool,
    first_result_id: Option<String>,
}

// ---------------------------------------------------------------------------
// migrate subcommand
// ---------------------------------------------------------------------------

#[cfg(feature = "shared-backend-postgres")]
fn run_migrate(cli: &Cli, args: &MigrateArgs) -> Result<String> {
    let source_dir = match &args.from {
        Some(s) => std::path::PathBuf::from(s.strip_prefix("sqlite://").unwrap_or(s.as_str())),
        None => cli.hivemind_dir.clone(),
    };
    let source_tenant = cli_tenant(cli)?;
    let sqlite = SqliteEventLedger::open(&source_dir)?;

    if args.dry_run {
        let mut count = 0u64;
        sqlite.replay_from_for_tenant(&source_tenant, 0, &mut |_event| {
            count += 1;
            Ok(())
        })?;
        let report = MigrateReport {
            dry_run: true,
            source_dir: source_dir.display().to_string(),
            source_tenant: source_tenant.to_string(),
            destination_tenant: args.to_tenant.clone(),
            events_migrated: count,
            parity_check: None,
        };
        return if cli.json {
            format_json_value(true, &report)
        } else {
            Ok(format!(
                "Dry run: {count} events would be migrated\n\
                 Source: {} (tenant: {})\n\
                 Destination tenant: {}",
                report.source_dir, report.source_tenant, report.destination_tenant
            ))
        };
    }

    let pg = PostgresEventLedger::connect(&args.to, &args.to_tenant)?;

    let mut migrated = 0u64;
    sqlite.replay_from_for_tenant(&source_tenant, 0, &mut |event| {
        pg.append(event.clone())?;
        migrated += 1;
        Ok(())
    })?;

    let mut pg_count = 0u64;
    pg.replay_from(0, &mut |_event| {
        pg_count += 1;
        Ok(())
    })?;

    let parity_ok = pg_count >= migrated;
    let report = MigrateReport {
        dry_run: false,
        source_dir: source_dir.display().to_string(),
        source_tenant: source_tenant.to_string(),
        destination_tenant: args.to_tenant.clone(),
        events_migrated: migrated,
        parity_check: Some(ParityCheckResult {
            source_event_count: migrated,
            destination_event_count: pg_count,
            ok: parity_ok,
        }),
    };

    if !parity_ok {
        return Err(CliError::InvalidInput(format!(
            "parity check failed: migrated {migrated} events but found {pg_count} in Postgres tenant '{}'",
            args.to_tenant
        ))
        .into());
    }

    if cli.json {
        format_json_value(true, &report)
    } else {
        Ok(format!(
            "Migration complete: {migrated} events migrated\n\
             Source: {} (tenant: {})\n\
             Destination tenant: {}\n\
             Parity check: OK ({pg_count} events in destination)",
            report.source_dir, report.source_tenant, report.destination_tenant
        ))
    }
}
