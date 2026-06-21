// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use clap::CommandFactory;

type CliTestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn ensure(condition: bool, context: &str) -> CliTestResult {
    if condition {
        Ok(())
    } else {
        Err(context.to_owned().into())
    }
}

fn ensure_eq<T>(actual: T, expected: T, context: &str) -> CliTestResult
where
    T: std::fmt::Debug + PartialEq,
{
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{context}: expected {expected:?}, got {actual:?}").into())
    }
}

fn json_at<'a>(
    value: &'a serde_json::Value,
    pointer: &str,
) -> std::result::Result<&'a serde_json::Value, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("missing json pointer {pointer}").into())
}

fn ensure_json_eq(
    actual: &serde_json::Value,
    expected: serde_json::Value,
    context: &str,
) -> CliTestResult {
    if actual == &expected {
        Ok(())
    } else {
        Err(format!("{context}: expected {expected}, got {actual}").into())
    }
}

#[test]
fn resolves_since_last_week_against_frozen_now_in_utc() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
    let resolved = resolve_diff_bound(
        "--since",
        Some("last week"),
        None,
        Some(now),
        TimeZoneSpec::Utc,
    )
    .expect("resolves last week");
    assert_eq!(
        resolved,
        Some(Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap()),
        "last week must resolve to the start of the previous ISO week (Mon 00:00 UTC)"
    );
}

#[test]
fn resolves_today_yesterday_this_week_against_frozen_now() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
    assert_eq!(
        resolve_diff_bound("--since", Some("today"), None, Some(now), TimeZoneSpec::Utc).unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap())
    );
    assert_eq!(
        resolve_diff_bound(
            "--since",
            Some("yesterday"),
            None,
            Some(now),
            TimeZoneSpec::Utc,
        )
        .unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap())
    );
    assert_eq!(
        resolve_diff_bound(
            "--since",
            Some("this week"),
            None,
            Some(now),
            TimeZoneSpec::Utc,
        )
        .unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap())
    );
    assert_eq!(
        resolve_diff_bound("--since", Some("now"), None, Some(now), TimeZoneSpec::Utc).unwrap(),
        Some(now)
    );
}

#[test]
fn non_utc_timezone_is_rejected_in_slice_1() {
    let error = TimeZoneSpec::parse("America/New_York").expect_err("non-utc rejected");
    assert!(error.to_string().contains("only UTC is accepted"));
}

#[test]
fn explicit_rfc3339_in_since_takes_precedence_over_phrase_parser() {
    let resolved = resolve_diff_bound(
        "--since",
        Some("2026-05-01T08:30:00Z"),
        None,
        None,
        TimeZoneSpec::Utc,
    )
    .expect("rfc3339 parses");
    use chrono::TimeZone;
    assert_eq!(
        resolved,
        Some(Utc.with_ymd_and_hms(2026, 5, 1, 8, 30, 0).unwrap())
    );
}

#[test]
fn resolves_duration_and_date_bounds_against_frozen_now() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();

    assert_eq!(
        resolve_diff_bound("--since", Some("7d"), None, Some(now), TimeZoneSpec::Utc).unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap())
    );
    assert_eq!(
        resolve_diff_bound("--since", Some("24h"), None, Some(now), TimeZoneSpec::Utc).unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap())
    );
    assert_eq!(
        resolve_diff_bound(
            "--since",
            Some("2026-05-15"),
            None,
            Some(now),
            TimeZoneSpec::Utc
        )
        .unwrap(),
        Some(Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap())
    );
}

#[test]
fn unknown_phrase_returns_friendly_error() {
    use chrono::TimeZone;
    let now = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();
    let error = resolve_diff_bound(
        "--since",
        Some("two fortnights ago"),
        None,
        Some(now),
        TimeZoneSpec::Utc,
    )
    .expect_err("unknown phrase rejected");
    assert!(error.to_string().contains("supported phrase"));
}

#[test]
fn parses_get_decisions_added_since_command() -> std::result::Result<(), Box<dyn std::error::Error>>
{
    let cli = Cli::parse_from([
        "hivemind",
        "query",
        "get_decisions_added_since",
        "--since",
        "last week",
        "--timezone",
        "UTC",
        "--now",
        "2026-05-19T12:00:00Z",
        "--source",
        "document",
        "--limit",
        "10",
    ]);
    let Command::Query(args) = cli.command else {
        return Err("expected query command".into());
    };
    let QueryCommand::GetDecisionsAddedSince(args) = args.command else {
        return Err("expected GetDecisionsAddedSince".into());
    };
    assert_eq!(args.since.as_deref(), Some("last week"));
    assert_eq!(args.now.as_deref(), Some("2026-05-19T12:00:00Z")); // ubs:ignore: test-only CLI fixture assertion.
    assert_eq!(args.filters.sources, vec!["document"]);
    assert_eq!(args.limit, 10);

    let request = added_since_request(&args).expect("request built");
    use chrono::TimeZone;
    assert_eq!(
        request.since_timestamp,
        Some(Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap())
    );
    assert_eq!(request.filters.sources, vec!["document"]);
    Ok(())
}

#[test]
fn parses_recent_decisions_command_with_composable_filters(
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse_from([
        "hivemind",
        "query",
        "recent_decisions",
        "--since",
        "7d",
        "--until",
        "2026-05-19",
        "--now",
        "2026-05-19T12:00:00Z",
        "--actor",
        "agent:claude:*",
        "--topic",
        "architecture",
        "--status",
        "accepted",
        "--source",
        "agent",
        "--summary",
    ]);
    let Command::Query(query_args) = cli.command else {
        return Err("expected query command".into());
    };
    assert!(query_args.summary); // ubs:ignore: test-only CLI parser assertion.
    let QueryCommand::RecentDecisions(args) = query_args.command else {
        return Err("expected RecentDecisions".into());
    };
    assert_eq!(args.since, "7d");
    assert_eq!(args.until.as_deref(), Some("2026-05-19"));
    assert_eq!(args.actor_patterns, vec!["agent:claude:*"]);
    assert_eq!(args.topic_keys, vec!["architecture"]);
    assert_eq!(args.statuses, vec![QueryDecisionStatus::Accepted]);
    assert_eq!(args.sources, vec!["agent"]);

    let request = recent_decisions_request(&args).expect("request built");
    use chrono::TimeZone;
    assert_eq!(
        request.since_timestamp,
        Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap()
    );
    assert_eq!(
        request.until_timestamp,
        Some(Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap())
    );
    assert_eq!(request.filters.actor_patterns, vec!["agent:claude:*"]);
    Ok(())
}

#[test]
fn parses_legacy_recent_alias() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse_from(["hivemind", "query", "recent", "--since", "7d"]);
    let Command::Query(args) = cli.command else {
        return Err("expected query command".into());
    };
    let is_recent = matches!(args.command, QueryCommand::RecentDecisions(_));
    assert!(is_recent, "recent alias must map to RecentDecisions"); // ubs:ignore: test-only.
    Ok(())
}

#[test]
fn query_summary_flag_is_global_for_graph_queries(
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse_from([
        "hivemind",
        "query",
        "search_decisions",
        "--q",
        "queue",
        "--summary",
    ]);
    let Command::Query(args) = cli.command else {
        return Err("expected query command".into());
    };
    assert!(args.summary); // ubs:ignore: test-only CLI parser assertion.
    assert!(matches!(args.command, QueryCommand::SearchDecisions(_))); // ubs:ignore: test-only CLI parser assertion.
    Ok(())
}

#[test]
fn query_help_documents_json_default_and_summary_mode(
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut command = Cli::command();
    let Some(query) = command.find_subcommand_mut("query") else {
        return Err("query subcommand exists".into());
    };
    let help = query.render_help().to_string();

    assert!(help.contains("JSON is the default output")); // ubs:ignore: test-only CLI help assertion.
    assert!(help.contains("--summary")); // ubs:ignore: test-only CLI help assertion.
    Ok(())
}

#[test]
fn parses_review_command_with_actor_window_and_unreviewed_filter() -> CliTestResult {
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "human:senior",
        "review",
        "--actor",
        "agent:*",
        "--since",
        "7d",
        "--until",
        "2026-05-19",
        "--now",
        "2026-05-19T12:00:00Z",
        "--unreviewed-only",
        "--limit",
        "10",
    ]);
    ensure_eq(cli.actor.as_str(), "human:senior", "reviewer actor")?;
    let Command::Review(args) = cli.command else {
        return Err("expected review command".into());
    };
    ensure_eq(args.actor_patterns.len(), 1, "review actor filter count")?;
    ensure_eq(
        args.actor_patterns.first().map(String::as_str),
        Some("agent:*"),
        "review actor filter",
    )?;
    ensure_eq(args.since.as_str(), "7d", "review since")?;
    ensure_eq(args.until.as_deref(), Some("2026-05-19"), "review until")?;
    ensure(args.unreviewed_only, "review unreviewed-only flag")?;
    ensure_eq(args.limit, 10, "review limit")?;

    let request = review_recent_decisions_request(&args)?;
    use chrono::TimeZone;
    let expected_since = Utc
        .with_ymd_and_hms(2026, 5, 12, 12, 0, 0)
        .single()
        .ok_or("valid expected timestamp")?;
    let expected_until = Utc
        .with_ymd_and_hms(2026, 5, 19, 0, 0, 0)
        .single()
        .ok_or("valid expected timestamp")?;
    ensure_eq(
        request.since_timestamp,
        expected_since,
        "review since timestamp",
    )?;
    ensure_eq(
        request.until_timestamp,
        Some(expected_until),
        "review until timestamp",
    )?;
    ensure_eq(
        request.filters.actor_patterns.len(),
        1,
        "review request actor filter count",
    )?;
    ensure_eq(
        request.filters.actor_patterns.first().map(String::as_str),
        Some("agent:*"),
        "review request actor filter",
    )?;
    Ok(())
}

#[test]
fn parses_global_flags_and_emit_subcommand() {
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--json",
        "--hivemind-dir",
        "./state",
        "--graph-backend",
        "memory",
        "-vv",
        "emit",
        "evidence.recorded",
        "--content",
        "sample",
    ]);

    assert_eq!(cli.actor, "agent-1");
    assert!(cli.json);
    assert_eq!(cli.verbose, 2);
    assert_eq!(cli.hivemind_dir, PathBuf::from("./state"));
    assert_eq!(cli.graph_backend, Some(GraphBackend::Memory));
    assert!(matches!(
        &cli.command,
        Command::Emit(command)
            if matches!(command.command, EmitCommand::EvidenceRecorded(_))
    ));
}

#[test]
fn parses_suggest_document_candidates_fixed_extractor_command() {
    let cli = Cli::parse_from([
        "hivemind",
        "suggest",
        "document-candidates",
        "--file",
        "memo.txt",
        "--format",
        "text",
        "--extractor-command",
        "hivemind-document-extractor",
        "--extractor-arg=--model=reviewer",
    ]);

    let args = match cli.command {
        Command::Suggest(args) => args,
        command => {
            assert!(
                matches!(command, Command::Suggest(_)),
                "expected suggest command"
            );
            return;
        }
    };
    let args = match args.command {
        SuggestCommand::DocumentCandidates(args) => args,
        command => {
            assert!(
                matches!(command, SuggestCommand::DocumentCandidates(_)),
                "expected document-candidates command"
            );
            return;
        }
    };
    assert_eq!(args.files, vec![PathBuf::from("memo.txt")]);
    assert_eq!(args.format, ImportDocumentFormat::Text);
    assert_eq!(
        args.extractor_command,
        Some(DocumentExtractorCommandArg::HivemindDocumentExtractor)
    );
    assert_eq!(args.extractor_args, vec!["--model=reviewer"]);
}

#[test]
fn cli_version_comes_from_cargo_package_version() {
    assert_eq!(
        Cli::command().get_version(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn quickstart_records_and_queries_decision_on_temp_ledger() {
    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
        "--json",
        "quickstart",
    ]))
    .expect("quickstart succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid quickstart json");
    let ledger_dir = PathBuf::from(
        output["ledger_dir"]
            .as_str()
            .expect("quickstart reports ledger dir"),
    );
    let decision_id = output["decision_id"]
        .as_str()
        .expect("quickstart reports decision id");

    assert_eq!(output["actor_id"], serde_json::json!("human:alice"));
    assert_eq!(output["query"]["result_count"], serde_json::json!(1));
    assert_eq!(output["query"]["total_matches"], serde_json::json!(1));
    assert_eq!(output["query"]["truncated"], serde_json::json!(false));
    assert_eq!(
        output["query"]["first_result_id"],
        serde_json::json!(decision_id)
    );
    assert!(ledger_dir.join("ledger.sqlite").exists());

    let _ = std::fs::remove_dir_all(&ledger_dir);
}

#[test]
fn parses_tui_filters_and_export_path() {
    let cli = Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        "./state",
        "tui",
        "--q",
        "queue",
        "--topic",
        "infra,storage",
        "--status",
        "accepted",
        "--actor-id",
        "agent:codex:1",
        "--source",
        "agent",
        "--limit",
        "5",
        "--dot-output",
        "focused.dot",
    ]);

    let args = match cli.command {
        Command::Tui(args) => args,
        command => {
            assert!(matches!(command, Command::Tui(_)), "expected tui command");
            return;
        }
    };
    assert_eq!(args.query.as_deref(), Some("queue"));
    assert_eq!(args.topic_keys, vec!["infra", "storage"]);
    assert_eq!(args.statuses, vec![QueryDecisionStatus::Accepted]);
    assert_eq!(args.actor_ids, vec!["agent:codex:1"]);
    assert_eq!(args.sources, vec!["agent"]);
    assert_eq!(args.limit, 5);
    assert_eq!(args.dot_output, PathBuf::from("focused.dot"));
}

#[cfg(not(feature = "tui"))]
#[test]
fn tui_command_requires_feature() {
    let cli = Cli::parse_from(["hivemind", "tui"]);

    let error = run(&cli).expect_err("tui needs feature");

    assert!(error
        .to_string()
        .contains("requires building with --features tui"));
}

#[test]
fn parses_graph_backend_from_env_aliases() {
    assert_eq!(parse_graph_backend("memory").unwrap(), GraphBackend::Memory);
    assert_eq!(
        parse_graph_backend("in-memory").unwrap(),
        GraphBackend::Memory
    );
    assert_eq!(parse_graph_backend("kuzu").unwrap(), GraphBackend::Kuzu);
    assert!(parse_graph_backend("postgres").is_err());
}

#[test]
fn maps_exit_codes_by_error_kind() {
    assert_eq!(
        exit_code_for_error(&HivemindError::Cli(CliError::InvalidInput("x".into()))).code(),
        2
    );
    assert_eq!(
        exit_code_for_error(&HivemindError::Command(CommandError::Validation(
            "x".into()
        )))
        .code(),
        2
    );
    assert_eq!(
        exit_code_for_error(&HivemindError::Command(CommandError::Invariant("x".into()))).code(),
        3
    );
    assert_eq!(
        exit_code_for_error(&HivemindError::Ledger(crate::LedgerError::Storage(
            "x".into()
        )))
        .code(),
        4
    );
    assert_eq!(
        exit_code_for_error(&HivemindError::Query(crate::QueryError::Execution(
            "x".into()
        )))
        .code(),
        1
    );
}

#[test]
fn emit_records_evidence_as_json() {
    let hivemind_dir = unique_test_dir("emit-records-evidence");
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "evidence.recorded",
        "--content",
        "API latency evidence",
    ]);

    let output = run(&cli).expect("emit evidence succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid json output");

    assert_eq!(
        output.get("subcommand").and_then(|value| value.as_str()),
        Some("emit")
    );
    assert_eq!(
        output.get("kind").and_then(|value| value.as_str()),
        Some("evidence_id")
    );
    assert!(output
        .get("value")
        .and_then(|value| value.as_str())
        .expect("evidence id")
        .starts_with("evidence-"));
}

#[test]
fn emit_proposes_decision_with_cli_option_labels() {
    let hivemind_dir = unique_test_dir("emit-proposes-decision");
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Pick queue",
        "--rationale",
        "Need durable ingestion",
        "--topic-keys",
        "infra,queue",
        "--options",
        "sync,async",
        "--chose",
        "async",
    ]);

    let output = run(&cli).expect("emit decision succeeds");

    assert!(output.starts_with("decision-"));
}

#[test]
fn disagree_cli_records_reason_contests_and_is_idempotent() {
    let hivemind_dir = unique_test_dir("disagree-cli");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:alice",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Keep current auth",
        "--rationale",
        "Lowest immediate migration cost",
        "--topic-keys",
        "auth",
        "--options",
        "keep",
    ]))
    .expect("decision proposed");
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:bob",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.accepted",
        "--decision-id",
        &decision_id,
    ]))
    .expect("decision accepted");

    let first_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:carol",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "disagree",
        "--decision",
        &decision_id,
        "--reason",
        "misses auth implications",
    ]))
    .expect("disagree succeeds");
    let first_output: serde_json::Value =
        serde_json::from_str(&first_output).expect("valid disagree json");
    assert_eq!(first_output["decision_id"], serde_json::json!(decision_id));
    assert_eq!(
        first_output["decision_status"],
        serde_json::json!("contested")
    );
    let first_event_id = first_output["event_id"].as_u64().expect("event id");

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let second_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:carol",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "disagree",
        "--decision",
        &decision_id,
        "--reason",
        "misses auth implications",
    ]))
    .expect("disagree retry succeeds");
    let second_output: serde_json::Value =
        serde_json::from_str(&second_output).expect("valid disagree json");
    assert_eq!(second_output["event_id"].as_u64(), Some(first_event_id));
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let events = ledger.read(0, 20).expect("events read");
    let rejected = events
        .iter()
        // ubs:ignore: public ledger event IDs are not secrets.
        .find(|event| event.event_id == Some(first_event_id))
        .expect("rejected event");
    assert_eq!(
        rejected.event_type,
        crate::events::EventType::DecisionRejected
    );
    assert_eq!(rejected.source, crate::events::EventSource::Human);
    assert_eq!(
        rejected
            .payload
            .get("reason")
            .and_then(|value| value.as_str()),
        Some("misses auth implications")
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn supersede_cli_proposes_replacement_marks_old_and_is_idempotent() {
    let hivemind_dir = unique_test_dir("supersede-cli");
    let old_decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:alice",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Use shared admin token",
        "--rationale",
        "Fastest path",
        "--topic-keys",
        "auth",
        "--options",
        "shared-token",
    ]))
    .expect("decision proposed");

    let first_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:bob",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "supersede",
        "--old",
        &old_decision_id,
        "--title",
        "Use scoped service tokens",
        "--rationale",
        "Scoped tokens preserve audit boundaries",
        "--options",
        "scoped-service-tokens",
        "--chose",
        "scoped-service-tokens",
    ]))
    .expect("supersede succeeds");
    let first_output: serde_json::Value =
        serde_json::from_str(&first_output).expect("valid supersede json");
    assert_eq!(
        first_output["old_decision_id"],
        serde_json::json!(old_decision_id)
    );
    assert!(first_output["new_decision_id"]
        .as_str()
        .expect("new decision id")
        .starts_with("decision-"));
    assert_eq!(
        first_output["old_decision_status"],
        serde_json::json!("superseded")
    );
    assert_eq!(
        first_output["new_decision_status"],
        serde_json::json!("proposed")
    );

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let second_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:bob",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "supersede",
        "--old",
        &old_decision_id,
        "--title",
        "Use scoped service tokens",
        "--rationale",
        "Scoped tokens preserve audit boundaries",
        "--options",
        "scoped-service-tokens",
        "--chose",
        "scoped-service-tokens",
    ]))
    .expect("supersede retry succeeds");
    let second_output: serde_json::Value =
        serde_json::from_str(&second_output).expect("valid supersede json");
    assert_eq!(
        second_output["new_decision_id"],
        first_output["new_decision_id"]
    );
    assert_eq!(
        second_output["superseded_event_id"],
        first_output["superseded_event_id"]
    );
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn review_cli_walkthrough_accepts_disagrees_and_filters_reviewed_decisions() -> CliTestResult {
    let hivemind_dir = unique_test_dir("review-cli");
    let hivemind_dir_arg = hivemind_dir.to_string_lossy().into_owned();
    let disagree_decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent:codex:one",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "emit",
        "decision.proposed",
        "--title",
        "Use permissive deploys",
        "--rationale",
        "It speeds up agent delivery",
        "--topic-keys",
        "deploy",
        "--options",
        "permissive,guarded",
        "--chose",
        "permissive",
    ]))?;
    let approve_decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent:codex:two",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "emit",
        "decision.proposed",
        "--title",
        "Keep guardrail tests",
        "--rationale",
        "They preserve governance invariants",
        "--topic-keys",
        "testing",
        "--options",
        "keep,drop",
        "--chose",
        "keep",
    ]))?;
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:architect",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "emit",
        "decision.accepted",
        "--decision-id",
        &disagree_decision_id,
    ]))?;

    let review_cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "human:senior",
        "--json",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "review",
        "--actor",
        "agent:codex:*",
        "--since",
        "2000-01-01",
    ]);
    let Command::Review(review_args) = &review_cli.command else {
        return Err("expected review command".into());
    };
    let mut input = std::io::Cursor::new("a\nd\nmisses rollout risk\n".as_bytes());
    let mut prompts = Vec::new();
    let output = run_review_session(&review_cli, review_args, &mut input, &mut prompts)?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    let prompts = String::from_utf8(prompts)?;

    ensure(
        prompts.contains("Keep guardrail tests"),
        "approval candidate prompt",
    )?;
    ensure(
        prompts.contains("Use permissive deploys"),
        "disagreement candidate prompt",
    )?;
    ensure_json_eq(
        json_at(&output, "/matched_count")?,
        serde_json::json!(2),
        "matched count",
    )?;
    ensure_json_eq(
        json_at(&output, "/reviewed_count")?,
        serde_json::json!(2),
        "reviewed count",
    )?;
    ensure_json_eq(
        json_at(&output, "/skipped_count")?,
        serde_json::json!(0),
        "skipped count",
    )?;
    ensure_json_eq(
        json_at(&output, "/quit")?,
        serde_json::json!(false),
        "quit flag",
    )?;
    ensure_json_eq(
        json_at(&output, "/reviewed_semantics")?,
        serde_json::json!(
            "derived from reviewer-authored decision.accepted, decision.rejected, or decision.superseded events"
        ),
        "reviewed semantics",
    )?;
    ensure_json_eq(
        json_at(&output, "/actions/0/decision_id")?,
        serde_json::json!(approve_decision_id.as_str()),
        "approved action decision",
    )?;
    ensure_json_eq(
        json_at(&output, "/actions/0/action")?,
        serde_json::json!("approved"),
        "approved action kind",
    )?;
    ensure_json_eq(
        json_at(&output, "/actions/0/old_decision_status")?,
        serde_json::json!("accepted"),
        "approved old status",
    )?;
    ensure_json_eq(
        json_at(&output, "/actions/1/decision_id")?,
        serde_json::json!(disagree_decision_id.as_str()),
        "disagreed action decision",
    )?;
    ensure_json_eq(
        json_at(&output, "/actions/1/action")?,
        serde_json::json!("disagreed"),
        "disagreed action kind",
    )?;
    ensure_json_eq(
        json_at(&output, "/actions/1/old_decision_status")?,
        serde_json::json!("contested"),
        "disagreed old status",
    )?;

    let ledger = SqliteEventLedger::open(&hivemind_dir)?;
    let graph = MemoryGraph::default();
    let tenant_id = cli_tenant(&review_cli)?;
    rebuild_graph_for_tenant(&ledger, &tenant_id, &graph)?;
    ensure_eq(
        derive_decision_status(&graph, &approve_decision_id)?,
        DecisionStatus::Accepted,
        "approved derived status",
    )?;
    ensure_eq(
        derive_decision_status(&graph, &disagree_decision_id)?,
        DecisionStatus::Contested,
        "disagreed derived status",
    )?;

    let events = ledger.read(0, 50)?;
    let accepted = events
        .iter()
        .find(|event| {
            event.actor_id == "human:senior" // ubs:ignore: public actor ID is not secret material.
                && event.event_type == crate::events::EventType::DecisionAccepted // ubs:ignore: public event type is not secret material.
                && event
                    .payload
                    .get("decision_id")
                    .and_then(|value| value.as_str())
                    == Some(approve_decision_id.as_str())
        })
        .ok_or("review acceptance event")?;
    ensure_eq(
        &accepted.source,
        &crate::events::EventSource::Human,
        "accepted provenance",
    )?;
    let rejected = events
        .iter()
        .find(|event| {
            event.actor_id == "human:senior" // ubs:ignore: public actor ID is not secret material.
                && event.event_type == crate::events::EventType::DecisionRejected // ubs:ignore: public event type is not secret material.
                && event
                    .payload
                    .get("decision_id")
                    .and_then(|value| value.as_str())
                    == Some(disagree_decision_id.as_str())
        })
        .ok_or("review disagreement event")?;
    ensure_eq(
        &rejected.source,
        &crate::events::EventSource::Human,
        "rejected provenance",
    )?;
    ensure_eq(
        rejected
            .payload
            .get("reason")
            .and_then(|value| value.as_str()),
        Some("misses rollout risk"),
        "rejection reason",
    )?;

    let unreviewed_cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "human:senior",
        "--json",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "review",
        "--actor",
        "agent:codex:*",
        "--since",
        "2000-01-01",
        "--unreviewed-only",
    ]);
    let Command::Review(unreviewed_args) = &unreviewed_cli.command else {
        return Err("expected review command".into());
    };
    let mut input = std::io::Cursor::new(Vec::<u8>::new());
    let mut prompts = Vec::new();
    let unreviewed_output =
        run_review_session(&unreviewed_cli, unreviewed_args, &mut input, &mut prompts)?;
    let unreviewed_output: serde_json::Value = serde_json::from_str(&unreviewed_output)?;
    ensure_json_eq(
        json_at(&unreviewed_output, "/matched_count")?,
        serde_json::json!(0),
        "unreviewed matched count",
    )?;
    let unreviewed_prompts = String::from_utf8(prompts)?;
    ensure(
        unreviewed_prompts.contains("No matching decisions"),
        "unreviewed empty prompt",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn search_decisions_cli_returns_query_response(
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let hivemind_dir = unique_test_dir("query-search-decisions");
    let hivemind_dir_arg = hivemind_dir.to_string_lossy().into_owned();
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "emit",
        "decision.proposed",
        "--title",
        "Pick queue",
        "--rationale",
        "Need durable ingestion",
        "--topic-keys",
        "infra,queue",
        "--options",
        "sync,async",
        "--chose",
        "async",
    ]))?;

    let query = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "query",
        "search_decisions",
        "--q",
        "queue",
        "--topic",
        "infra",
        "--status",
        "proposed",
        "--actor-id",
        "agent-1",
        "--source",
        "cli",
        "--limit",
        "5",
    ]))?;
    let query: serde_json::Value = serde_json::from_str(&query)?;

    assert_eq!(query["result_count"], serde_json::json!(1));
    assert_eq!(query["data"]["items"][0]["decision"]["id"], decision_id);
    assert_eq!(query["data"]["items"][0]["rank"], serde_json::json!(1));
    assert_eq!(query["data"]["next_cursor"], serde_json::Value::Null);

    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir_arg.as_str(),
        "query",
        "search_decisions",
        "--q",
        "queue",
        "--summary",
    ]))?;
    assert!(summary.contains(&decision_id)); // ubs:ignore: test-only CLI summary assertion.
    assert!(summary.contains("rank=1")); // ubs:ignore: test-only CLI summary assertion.
    assert!(summary.contains("Pick queue")); // ubs:ignore: test-only CLI summary assertion.

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn search_cli_alias_uses_fts_surface_with_time_filters() {
    let hivemind_dir = unique_test_dir("query-search-fts");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-fts",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Adopt authentication boundary",
        "--rationale",
        "OAuth routing keeps search reproducible",
        "--topic-keys",
        "security,auth",
        "--options",
        "gateway,sidecar",
        "--chose",
        "gateway",
    ]))
    .expect("emit decision succeeds");

    let query = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "search",
        "--q",
        "gateway",
        "--topic",
        "security",
        "--actor-id",
        "agent-fts",
        "--since",
        "2000-01-01T00:00:00Z",
        "--until",
        "2999-01-01T00:00:00Z",
        "--limit",
        "5",
    ]))
    .expect("search query succeeds");
    let query: serde_json::Value = serde_json::from_str(&query).expect("valid query json");

    assert_eq!(query["result_count"], serde_json::json!(1));
    assert_eq!(query["data"]["items"][0]["decision"]["id"], decision_id);
    assert_eq!(
        query["data"]["items"][0]["matched_fields"],
        serde_json::json!(["option.id"])
    );
    assert_eq!(
        query["data"]["filters"]["since"],
        serde_json::json!("2000-01-01T00:00:00Z")
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn recall_cli_returns_ranked_decisions_and_digest() {
    let hivemind_dir = unique_test_dir("query-recall");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-recall",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Use circuit breaker for downstream calls",
        "--rationale",
        "Circuit breaker prevents cascading failures in distributed services",
        "--topic-keys",
        "reliability,architecture",
        "--options",
        "circuit-breaker,timeout-only",
        "--chose",
        "circuit-breaker",
    ]))
    .expect("emit decision succeeds");

    // JSON mode: structured recall response
    let query = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "recall",
        "circuit breaker",
        "--limit",
        "5",
    ]))
    .expect("recall query succeeds");
    let query: serde_json::Value = serde_json::from_str(&query).expect("valid recall json");
    assert_eq!(query["result_count"], serde_json::json!(1)); // ubs:ignore: test-only assertion
    assert_eq!(
        query["data"]["ranked"]["items"][0]["decision"]["id"],
        decision_id
    ); // ubs:ignore: test-only assertion
    let cited = query["data"]["digest"]["cited_decision_ids"]
        .as_array()
        .expect("cited_decision_ids array"); // ubs:ignore: test-only; panicking is correct
    assert!(
        cited
            .iter()
            .any(|id| id.as_str() == Some(decision_id.as_str())),
        "decision_id must appear in digest cited_decision_ids"
    ); // ubs:ignore: test-only assertion
    assert!(
        query["data"]["digest"]["summary"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "digest summary must be non-empty"
    ); // ubs:ignore: test-only assertion

    // Summary mode: human-friendly text
    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "--summary",
        "recall",
        "circuit breaker",
    ]))
    .expect("recall summary succeeds");
    assert!(
        summary.contains("digest\t"),
        "summary output must contain digest line"
    ); // ubs:ignore: test-only assertion
    assert!(
        summary.contains("match\t"),
        "summary output must contain match line"
    ); // ubs:ignore: test-only assertion

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn ledger_history_cli_queries_and_exports_read_only_summary() {
    let hivemind_dir = unique_test_dir("query-ledger-history");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Pick queue",
        "--rationale",
        "Need durable ingestion",
        "--topic-keys",
        "infra,queue",
        "--options",
        "sync,async",
        "--chose",
        "async",
    ]))
    .expect("emit decision succeeds");

    let recent = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_recent_activity",
        "--limit",
        "1",
        "--source",
        "cli",
    ]))
    .expect("recent activity query succeeds");
    let recent: serde_json::Value =
        serde_json::from_str(&recent).expect("valid recent activity json");
    assert_eq!(recent["result_count"], serde_json::json!(1));
    assert_eq!(recent["data"]["items"][0]["decision_ids"][0], decision_id);
    assert!(recent["data"]["items"][0]["citation_id"]
        .as_str()
        .expect("citation id")
        .starts_with("event:"));

    let recent_decisions = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "recent_decisions",
        "--since",
        "7d",
        "--actor",
        "agent-1",
        "--topic",
        "infra",
        "--status",
        "proposed",
        "--source",
        "cli",
        "--limit",
        "5",
    ]))
    .expect("recent decisions query succeeds");
    let recent_decisions: serde_json::Value =
        serde_json::from_str(&recent_decisions).expect("valid recent decisions json");
    assert_eq!(recent_decisions["result_count"], serde_json::json!(1));
    assert_eq!(
        recent_decisions["data"]["items"][0]["decision_id"],
        decision_id
    );
    assert_eq!(
        recent_decisions["data"]["items"][0]["status"],
        serde_json::json!("proposed")
    );

    let empty_recent_decisions = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "recent_decisions",
        "--since",
        "9999-01-01",
    ]))
    .expect("empty recent decisions query succeeds");
    let empty_recent_decisions: serde_json::Value =
        serde_json::from_str(&empty_recent_decisions).expect("valid empty recent decisions json");
    assert_eq!(empty_recent_decisions["result_count"], serde_json::json!(0));
    assert_eq!(
        empty_recent_decisions["data"]["items"],
        serde_json::json!([])
    );

    let recent_summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "recent_decisions",
        "--since",
        "7d",
        "--summary",
    ]))
    .expect("recent decisions summary query succeeds");
    assert!(recent_summary.contains(&decision_id));
    assert!(recent_summary.contains("proposed"));

    let changed = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decisions_changed_since",
        "--since-offset",
        "0",
        "--limit",
        "1",
    ]))
    .expect("changed-since query succeeds");
    let changed: serde_json::Value =
        serde_json::from_str(&changed).expect("valid changed-since json");
    assert_eq!(
        changed["data"]["items"][0]["change_kind"],
        serde_json::json!("new_decision")
    );

    let export = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "export_read_only_summary",
        "--query",
        "recent_activity",
        "--format",
        "markdown",
        "--generated-at",
        "2026-05-19T12:00:00Z",
        "--limit",
        "10",
    ]))
    .expect("export query succeeds");
    let export: serde_json::Value = serde_json::from_str(&export).expect("valid export json");
    assert_eq!(export["data"]["format"], serde_json::json!("markdown"));
    assert_eq!(
        export["data"]["citation_map"]["event:1"]["source"],
        serde_json::json!("cli")
    );
    assert!(export["data"]["markdown"]
        .as_str()
        .expect("markdown body")
        .contains("citation=event:1"));

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn suggest_document_candidates_materializes_reviewed_block_before_import() {
    let root = unique_test_dir("suggest-document-candidates");
    let hivemind_dir = root.join("hive");
    std::fs::create_dir_all(&root).expect("scratch dir");
    let document = root.join("memo.txt");
    let memo = "Architecture memo\nThe team chose reviewed document extraction for local decision notes because automatic ledger writes would bypass review. Considered options were review candidates first and auto-import everything. The assumption is that reviewers can inspect the candidate file quickly.\n";
    std::fs::write(&document, memo).expect("write memo");
    let excerpt = "The team chose reviewed document extraction for local decision notes because automatic ledger writes would bypass review.";
    let byte_start = memo.find(excerpt).expect("excerpt start");
    let byte_end = byte_start + excerpt.len();
    let response = root.join("llm-response.json");
    std::fs::write(
        &response,
        serde_json::json!({
            "candidates": [{
                "file_index": 0,
                "source_span": {
                    "byte_start": byte_start,
                    "byte_end": byte_end,
                    "line_start": 2,
                    "line_end": 2
                },
                "title": "Use reviewed document extraction",
                "topic_keys": ["documents", "layer3"],
                "rationale": "Automatic ledger writes would bypass review.",
                "option_labels": ["review candidates first", "auto-import everything"],
                "chosen_option_label": "review candidates first",
                "evidence": ["The memo says automatic ledger writes would bypass review."],
                "hypotheses": ["Reviewers can inspect the candidate file quickly."],
                "explanation": "The excerpt names a choice and its rationale."
            }]
        })
        .to_string(),
    )
    .expect("write llm response");

    let candidates = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "suggest",
        "document-candidates",
        "--file",
        document.to_str().expect("utf-8 document path"),
        "--format",
        "text",
        "--llm-response",
        response.to_str().expect("utf-8 response path"),
    ]))
    .expect("candidate suggestion succeeds");
    assert!(
        !hivemind_dir.join("ledger.sqlite").exists(),
        "suggestion must not open or append to the ledger"
    );
    let candidates_json: serde_json::Value =
        serde_json::from_str(&candidates).expect("valid candidate json");
    assert_eq!(
        candidates_json["workflow"],
        serde_json::json!("hivemind.document_extraction_candidates.v1")
    );
    assert_eq!(
        candidates_json["summary"]["candidates_proposed"],
        serde_json::json!(1)
    );
    assert_eq!(
        candidates_json["candidates"][0]["review_status"],
        serde_json::json!("pending_review")
    );
    assert!(candidates_json["candidates"][0]["source"]["snippet"]
        .as_str()
        .expect("source snippet")
        .contains("reviewed document extraction"));
    let candidate_id = candidates_json["candidates"][0]["candidate_id"]
        .as_str()
        .expect("candidate id")
        .to_owned();
    let candidate_report = root.join("candidates.json");
    std::fs::write(&candidate_report, candidates).expect("write candidate report");
    let materialized = root.join("reviewed.md");

    let materialized_report = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:alice",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "suggest",
        "materialize-document-candidates",
        "--input",
        candidate_report.to_str().expect("utf-8 candidate path"),
        "--candidate-id",
        &candidate_id,
        "--output",
        materialized.to_str().expect("utf-8 output path"),
    ]))
    .expect("materialization succeeds");
    assert!(
        !hivemind_dir.join("ledger.sqlite").exists(),
        "materialization must not append to the ledger"
    );
    let materialized_report: serde_json::Value =
        serde_json::from_str(&materialized_report).expect("valid materialization json");
    assert_eq!(
        materialized_report["candidates_materialized"],
        serde_json::json!(1)
    );
    let reviewed_block = std::fs::read_to_string(&materialized).expect("materialized block");
    assert!(reviewed_block.contains("Decision:"));
    assert!(reviewed_block.contains("reviewed_by: reviewer:alice"));
    assert!(reviewed_block.contains("Source document"));

    let imported = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:alice",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        materialized.to_str().expect("utf-8 materialized path"),
    ]))
    .expect("reviewed materialized block imports");
    let imported: serde_json::Value = serde_json::from_str(&imported).expect("valid import json");
    assert_eq!(imported["summary"]["blocks_imported"], serde_json::json!(1));
    assert!(hivemind_dir.join("ledger.sqlite").exists());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn import_documents_cli_imports_queryable_document_decisions_and_reimport_noops() {
    let hivemind_dir = unique_test_dir("import-documents");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/documents");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        fixtures.to_str().expect("utf-8 fixture path"),
    ]))
    .expect("document import succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    assert_eq!(output["summary"]["blocks_imported"], serde_json::json!(2));
    assert_eq!(output["summary"]["events_written"].as_u64(), Some(15));

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");

    let search = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "search_decisions",
        "--source",
        "document",
        "--topic",
        "storage",
    ]))
    .expect("document decision search succeeds");
    let search: serde_json::Value = serde_json::from_str(&search).expect("valid search json");
    assert_eq!(search["result_count"], serde_json::json!(1));
    assert_eq!(
        search["data"]["items"][0]["decision"]["status"],
        serde_json::json!("accepted")
    );

    let events = ledger.read(0, 100).expect("events read");
    let storage_proposal = events
        .iter()
        .find(|event| {
            // ubs:ignore: public event types and decision titles are not secrets.
            event.event_type == crate::events::EventType::DecisionProposed
                && event.payload.get("title").and_then(|value| value.as_str())
                    == Some("Use SQLite for the local prototype")
        })
        .expect("storage proposal event");
    assert_eq!(storage_proposal.actor_id, "actor:alice");
    assert_eq!(
        storage_proposal.source,
        crate::events::EventSource::Document
    );
    let storage_ref: serde_json::Value = serde_json::from_str(
        storage_proposal
            .source_ref
            .as_deref()
            .expect("document source ref"),
    )
    .expect("document source ref json");
    assert_eq!(storage_ref["source"], serde_json::json!("document"));
    assert_eq!(storage_ref["block_id"], serde_json::json!("local-storage"));
    assert_eq!(storage_ref["provisional_actor"], serde_json::json!(false));
    assert!(storage_ref["path"]
        .as_str()
        .expect("source path")
        .ends_with("storage_decision.md"));
    assert!(storage_ref["sha256"].as_str().expect("source hash").len() >= 64);
    assert!(storage_ref["source_span"]["line_start"].as_u64().unwrap() > 0);
    assert!(storage_ref["source_snippet"]
        .as_str()
        .expect("snippet")
        .contains("Decision:"));

    let report_proposal = events
        .iter()
        .find(|event| {
            // ubs:ignore: public event types and decision titles are not secrets.
            event.event_type == crate::events::EventType::DecisionProposed
                && event.payload.get("title").and_then(|value| value.as_str())
                    == Some("Import weekly decision notes locally")
        })
        .expect("report proposal event");
    assert_eq!(report_proposal.actor_id, "importer:local");
    let report_ref: serde_json::Value = serde_json::from_str(
        report_proposal
            .source_ref
            .as_deref()
            .expect("document source ref"),
    )
    .expect("document source ref json");
    assert_eq!(report_ref["provisional_actor"], serde_json::json!(true));
    assert_eq!(report_ref["original_actor_id"], serde_json::Value::Null);

    let second_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        fixtures.to_str().expect("utf-8 fixture path"),
    ]))
    .expect("document re-import succeeds");
    let second_output: serde_json::Value =
        serde_json::from_str(&second_output).expect("valid import json");
    assert_eq!(
        second_output["summary"]["blocks_imported"],
        serde_json::json!(0)
    );
    assert_eq!(
        second_output["summary"]["blocks_noop"],
        serde_json::json!(2)
    );
    assert_eq!(
        second_output["summary"]["events_written"],
        serde_json::json!(0)
    );
    let reimport_blocks = second_output["files"]
        .as_array()
        .expect("files array")
        .iter()
        .flat_map(|file| file["blocks"].as_array().expect("blocks array"));
    for block in reimport_blocks {
        assert!(
            block.get("similarity_matches").is_none(),
            "exact re-import must stay separate from fuzzy advisory matching"
        );
    }
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn prepare_documents_cli_extracts_pdf_text_for_reviewed_import() {
    let hivemind_dir = unique_test_dir("prepare-pdf-ledger");
    let scratch_dir = unique_test_dir("prepare-pdf-source");
    let output_dir = scratch_dir.join("prepared");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let pdf_path = scratch_dir.join("decision.pdf");
    write_simple_pdf(
        &pdf_path,
        &[
            "Decision:",
            "id: pdf-ingestion",
            "title: Preserve PDF decisions",
            "status: accepted",
            "actor: actor:pdf",
            "topic_keys: documents, pdf",
            "rationale: Text PDF extraction keeps reviewers in front of ledger writes.",
            "options:",
            "- prepare text",
            "- direct ledger write",
            "chose: prepare text",
            "evidence:",
            "- Page extraction keeps the original page reference.",
        ],
    );

    let prepared = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "import",
        "prepare-documents",
        "--output-dir",
        output_dir.to_str().expect("utf-8 output dir"),
        pdf_path.to_str().expect("utf-8 pdf path"),
    ]))
    .expect("PDF preparation succeeds");
    let prepared: serde_json::Value =
        serde_json::from_str(&prepared).expect("valid preparation json");
    assert_eq!(prepared["summary"]["files_prepared"], serde_json::json!(1));
    assert_eq!(
        prepared["summary"]["files_review_required"],
        serde_json::json!(0)
    );
    assert_eq!(
        prepared["files"][0]["source_kind"],
        serde_json::json!("pdf_text")
    );
    let prepared_path = PathBuf::from(
        prepared["files"][0]["prepared_path"]
            .as_str()
            .expect("prepared path"),
    );
    let prepared_text = std::fs::read_to_string(&prepared_path).expect("prepared text");
    assert!(prepared_text.contains("# hivemind-source-ref:"));
    assert!(prepared_text.contains("Decision:"));

    let imported = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:pdf",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        prepared_path.to_str().expect("utf-8 prepared path"),
    ]))
    .expect("prepared PDF text imports");
    let imported: serde_json::Value = serde_json::from_str(&imported).expect("valid import json");
    assert_eq!(imported["summary"]["blocks_imported"], serde_json::json!(1));

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 100).expect("events read");
    let proposal = events
        .iter()
        .find(|event| {
            // ubs:ignore: public event types and decision titles are not secrets.
            event.event_type == crate::events::EventType::DecisionProposed
                && event.payload.get("title").and_then(|value| value.as_str())
                    == Some("Preserve PDF decisions")
        })
        .expect("PDF proposal event");
    let source_ref: serde_json::Value =
        serde_json::from_str(proposal.source_ref.as_deref().expect("document source ref"))
            .expect("document source ref json");
    assert_eq!(
        source_ref["prepared_from"]["extraction_kind"],
        serde_json::json!("pdf_text")
    );
    assert_eq!(
        source_ref["prepared_from"]["page_number"],
        serde_json::json!(1)
    );
    assert_eq!(
        source_ref["prepared_from"]["ocr_review_required"],
        serde_json::json!(false)
    );
    assert!(source_ref["prepared_from"]["path"]
        .as_str()
        .expect("source path")
        .ends_with("decision.pdf"));

    let _ = std::fs::remove_dir_all(&scratch_dir);
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn prepare_documents_cli_surfaces_ocr_uncertainty_before_import() {
    let hivemind_dir = unique_test_dir("prepare-ocr-ledger");
    let scratch_dir = unique_test_dir("prepare-ocr-source");
    let output_dir = scratch_dir.join("prepared");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let ocr_path = scratch_dir.join("scanned.ocr.txt");
    std::fs::write(
        &ocr_path,
        "Decision:\nid: scanned-ingestion\ntitle: Review scanned decisions\nstatus: proposed\ntopic_keys: documents, ocr\nrationale: OCR text can contain recognition mistakes and must be reviewed.\noptions:\n- prepare text\n- trust raw OCR\nchose: prepare text\n",
    )
    .expect("write OCR text");

    let prepared = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "import",
        "prepare-documents",
        "--output-dir",
        output_dir.to_str().expect("utf-8 output dir"),
        ocr_path.to_str().expect("utf-8 OCR path"),
    ]))
    .expect("OCR preparation succeeds");
    let prepared: serde_json::Value =
        serde_json::from_str(&prepared).expect("valid preparation json");
    assert_eq!(prepared["summary"]["files_prepared"], serde_json::json!(1));
    assert_eq!(
        prepared["summary"]["files_review_required"],
        serde_json::json!(1)
    );
    assert_eq!(
        prepared["files"][0]["source_kind"],
        serde_json::json!("ocr_text")
    );
    assert_eq!(
        prepared["files"][0]["pages"][0]["ocr_uncertainty"][0],
        serde_json::json!("ocr_confidence_unavailable")
    );
    let prepared_path = PathBuf::from(
        prepared["files"][0]["prepared_path"]
            .as_str()
            .expect("prepared path"),
    );
    let prepared_text = std::fs::read_to_string(&prepared_path).expect("prepared text");
    assert!(prepared_text.contains("# ocr_review_required: true"));

    let imported = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:ocr",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        prepared_path.to_str().expect("utf-8 prepared path"),
    ]))
    .expect("reviewed OCR text imports");
    let imported: serde_json::Value = serde_json::from_str(&imported).expect("valid import json");
    assert_eq!(imported["summary"]["blocks_imported"], serde_json::json!(1));
    assert!(imported["files"][0]["blocks"][0]["message"]
        .as_str()
        .expect("OCR import message")
        .contains("OCR"));

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 100).expect("events read");
    let proposal = events
        .iter()
        .find(|event| {
            // ubs:ignore: public event types and decision titles are not secrets.
            event.event_type == crate::events::EventType::DecisionProposed
                && event.payload.get("title").and_then(|value| value.as_str())
                    == Some("Review scanned decisions")
        })
        .expect("OCR proposal event");
    let source_ref: serde_json::Value =
        serde_json::from_str(proposal.source_ref.as_deref().expect("document source ref"))
            .expect("document source ref json");
    assert_eq!(
        source_ref["prepared_from"]["ocr_review_required"],
        serde_json::json!(true)
    );
    assert_eq!(
        source_ref["prepared_from"]["ocr_uncertainty"][0],
        serde_json::json!("ocr_confidence_unavailable")
    );

    let _ = std::fs::remove_dir_all(&scratch_dir);
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn import_documents_cli_reports_changed_same_id_as_conflict_without_writes() {
    let hivemind_dir = unique_test_dir("import-document-conflict-ledger");
    let scratch_dir = unique_test_dir("import-document-conflict-doc");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let document_path = scratch_dir.join("decision.md");
    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: proposed\n  topic_keys: conflict\n  rationale: First rationale.\n  options:\n    - first option\n",
    )
    .expect("write initial doc");

    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("initial import succeeds");
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");

    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Changed title\n  status: proposed\n  topic_keys: conflict\n  rationale: Changed rationale.\n  options:\n    - first option\n",
    )
    .expect("write changed doc");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("conflict import reports successfully");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    assert_eq!(output["summary"]["blocks_conflicted"], serde_json::json!(1));
    assert_eq!(output["summary"]["events_written"], serde_json::json!(0));
    let block = &output["files"][0]["blocks"][0];
    assert!(block["message"]
        .as_str()
        .expect("conflict message")
        .contains("stable decision id already exists"));
    assert_eq!(
        block["reviewer_action"],
        serde_json::json!("resolve_import_conflict")
    );
    let matches = block["similarity_matches"]
        .as_array()
        .expect("conflict should include traceable fuzzy match");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["review_required"], serde_json::json!(true));
    assert!(matches[0]["event_origin"].as_u64().is_some());
    assert_eq!(
        matches[0]["basis"]["algorithm"],
        serde_json::json!("document_fuzzy_v1")
    );
    assert_eq!(
        matches[0]["basis"]["same_stable_block_id"],
        serde_json::json!(true)
    );
    assert!(matches[0]["basis"]["source_ref"].as_str().is_some());
    let conflict = &block["conflict"];
    assert_eq!(conflict["selected_action"], serde_json::json!("report"));
    assert_eq!(
        conflict["existing"]["title"],
        serde_json::json!("Keep first title")
    );
    assert_eq!(
        conflict["proposed_update"]["title"],
        serde_json::json!("Changed title")
    );
    assert_eq!(
        conflict["proposed_update"]["source"]["block_id"],
        serde_json::json!("conflict-demo")
    );
    assert_eq!(
        conflict["affected_dependencies"]["option_ids"]
            .as_array()
            .expect("option ids")
            .len(),
        1
    );
    assert!(conflict["available_actions"]
        .as_array()
        .expect("actions")
        .contains(&serde_json::json!("supersede")));
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let kept_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--on-conflict",
        "keep_existing",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("keep-existing conflict import reports successfully");
    let kept_output: serde_json::Value =
        serde_json::from_str(&kept_output).expect("valid import json");
    assert_eq!(
        kept_output["files"][0]["blocks"][0]["status"],
        serde_json::json!("conflict_kept_existing")
    );
    assert_eq!(
        kept_output["summary"]["blocks_resolved"],
        serde_json::json!(1)
    );
    assert_eq!(
        kept_output["summary"]["events_written"],
        serde_json::json!(0)
    );
    assert_eq!(
        ledger
            .latest_offset()
            .expect("latest offset still unchanged"),
        latest_after_first
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn import_documents_cli_can_resolve_conflict_as_supersession() {
    let hivemind_dir = unique_test_dir("import-document-conflict-supersede-ledger");
    let scratch_dir = unique_test_dir("import-document-conflict-supersede-doc");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let document_path = scratch_dir.join("decision.md");
    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: accepted\n  actor: actor:alice\n  topic_keys: conflict\n  rationale: First rationale.\n  options:\n    - first option\n  chose: first option\n",
    )
    .expect("write initial doc");

    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("initial import succeeds");
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");

    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Superseding title\n  status: accepted\n  actor: actor:bob\n  topic_keys: conflict\n  rationale: Replacement rationale.\n  options:\n    - second option\n  chose: second option\n",
    )
    .expect("write changed doc");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--on-conflict",
        "supersede",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("supersede import succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    assert_eq!(output["summary"]["blocks_resolved"], serde_json::json!(1));
    assert_eq!(
        output["files"][0]["blocks"][0]["status"],
        serde_json::json!("conflict_superseded")
    );
    let old_decision_id = output["files"][0]["blocks"][0]["decision_id"]
        .as_str()
        .expect("old decision id");
    let new_decision_id = output["files"][0]["blocks"][0]["conflict"]["resolved_decision_id"]
        .as_str()
        .expect("resolved decision id");
    assert_ne!(old_decision_id, new_decision_id);
    assert!(ledger.latest_offset().expect("events appended") > latest_after_first);

    let old_view = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decision",
        "--id",
        old_decision_id,
    ]))
    .expect("old decision query succeeds");
    let old_view: serde_json::Value = serde_json::from_str(&old_view).expect("valid json");
    assert_eq!(old_view["data"]["status"], serde_json::json!("superseded"));

    let new_view = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decision",
        "--id",
        new_decision_id,
    ]))
    .expect("new decision query succeeds");
    let new_view: serde_json::Value = serde_json::from_str(&new_view).expect("valid json");
    assert_eq!(new_view["data"]["status"], serde_json::json!("accepted"));
    assert_eq!(
        new_view["data"]["title"],
        serde_json::json!("Superseding title")
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn import_documents_cli_can_resolve_conflict_by_contesting_existing_decision() {
    let hivemind_dir = unique_test_dir("import-document-conflict-contest-ledger");
    let scratch_dir = unique_test_dir("import-document-conflict-contest-doc");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let document_path = scratch_dir.join("decision.md");
    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: accepted\n  actor: actor:alice\n  topic_keys: conflict\n  rationale: First rationale.\n  options:\n    - first option\n  chose: first option\n",
    )
    .expect("write initial doc");

    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("initial import succeeds");

    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Contesting title\n  status: proposed\n  topic_keys: conflict\n  rationale: This changed import disagrees with the accepted decision.\n  options:\n    - second option\n",
    )
    .expect("write changed doc");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--on-conflict",
        "contest",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("contest import succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    assert_eq!(
        output["files"][0]["blocks"][0]["status"],
        serde_json::json!("conflict_contested")
    );
    let decision_id = output["files"][0]["blocks"][0]["decision_id"]
        .as_str()
        .expect("decision id");

    let view = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decision",
        "--id",
        decision_id,
    ]))
    .expect("decision query succeeds");
    let view: serde_json::Value = serde_json::from_str(&view).expect("valid json");
    assert_eq!(view["data"]["status"], serde_json::json!("contested"));

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn import_documents_cli_can_resolve_conflict_by_adding_context() {
    let hivemind_dir = unique_test_dir("import-document-conflict-context-ledger");
    let scratch_dir = unique_test_dir("import-document-conflict-context-doc");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let document_path = scratch_dir.join("decision.md");
    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: proposed\n  topic_keys: conflict\n  rationale: First rationale.\n  options:\n    - first option\n",
    )
    .expect("write initial doc");

    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("initial import succeeds");
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");

    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Changed title\n  status: proposed\n  topic_keys: conflict\n  rationale: Changed rationale.\n  options:\n    - first option\n  evidence:\n    - New evidence from re-import.\n  hypotheses:\n    - New assumption from re-import.\n",
    )
    .expect("write changed doc");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--on-conflict",
        "add_context",
        "--file",
        document_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("add context import succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    assert_eq!(
        output["files"][0]["blocks"][0]["status"],
        serde_json::json!("conflict_context_added")
    );
    let decision_id = output["files"][0]["blocks"][0]["decision_id"]
        .as_str()
        .expect("decision id");

    let view = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decision",
        "--id",
        decision_id,
    ]))
    .expect("decision query succeeds");
    let view: serde_json::Value = serde_json::from_str(&view).expect("valid json");
    assert_eq!(
        view["data"]["evidence_ids"]
            .as_array()
            .expect("evidence ids")
            .len(),
        1
    );
    assert_eq!(
        view["data"]["hypotheses"]
            .as_array()
            .expect("hypotheses")
            .len(),
        1
    );

    let diff = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decisions_added_since",
        "--since-offset",
        &latest_after_first.to_string(),
    ]))
    .expect("diff query succeeds");
    let diff: serde_json::Value = serde_json::from_str(&diff).expect("valid json");
    assert_eq!(diff["data"]["total_changed_existing"], serde_json::json!(1));
    assert_eq!(
        diff["data"]["changed_existing_decisions"][0]["decision_id"],
        serde_json::json!(decision_id)
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn import_documents_cli_reports_near_duplicate_fuzzy_candidate_without_writes() {
    let hivemind_dir = unique_test_dir("import-document-fuzzy-ledger");
    let scratch_dir = unique_test_dir("import-document-fuzzy-docs");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let original_path = scratch_dir.join("storage.md");
    let edited_path = scratch_dir.join("edited-storage.md");
    std::fs::write(
        &original_path,
        "Decision:\n  id: local-sqlite\n  title: Use SQLite for local prototype storage\n  status: accepted\n  actor: actor:alice\n  topic_keys: storage, local\n  rationale: SQLite keeps setup small and replay fast for the local prototype.\n  options:\n    - sqlite\n    - postgres\n  chose: sqlite\n",
    )
    .expect("write original doc");

    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        original_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("original import succeeds");
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_original = ledger.latest_offset().expect("latest offset");

    std::fs::write(
        &edited_path,
        "Decision:\n  id: choose-sqlite-store\n  title: Choose SQLite as the local prototype store\n  status: accepted\n  topic_keys: local, storage\n  rationale: Embedded SQLite keeps setup light and replay tests fast for the prototype.\n  options:\n    - sqlite\n    - flat files\n  chose: sqlite\n",
    )
    .expect("write near duplicate doc");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        edited_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("near duplicate import reports successfully");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    assert_eq!(
        output["summary"]["duplicate_candidates"],
        serde_json::json!(1)
    );
    assert_eq!(output["summary"]["events_written"], serde_json::json!(0));
    let block = &output["files"][0]["blocks"][0];
    assert_eq!(block["status"], serde_json::json!("duplicate_candidate"));
    assert_eq!(
        block["reviewer_action"],
        serde_json::json!("review_fuzzy_duplicate_candidate")
    );
    assert!(block["message"]
        .as_str()
        .expect("candidate message")
        .contains("fuzzy duplicate candidate"));
    let matches = block["similarity_matches"]
        .as_array()
        .expect("similarity matches");
    assert_eq!(matches.len(), 1);
    assert!(matches[0]["score"].as_u64().expect("score") >= 70);
    assert_eq!(matches[0]["review_required"], serde_json::json!(true));
    assert!(matches[0]["decision_id"]
        .as_str()
        .expect("matched decision id")
        .contains("local-sqlite"));
    assert!(matches[0]["basis"]["matched_fields"]
        .as_array()
        .expect("matched fields")
        .iter()
        .any(|field| field == "title"));
    assert!(matches[0]["basis"]["source_ref"].as_str().is_some());
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_original
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn import_documents_cli_requires_review_for_ambiguous_fuzzy_matches() {
    let hivemind_dir = unique_test_dir("import-document-ambiguous-ledger");
    let scratch_dir = unique_test_dir("import-document-ambiguous-docs");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let storage_path = scratch_dir.join("storage.md");
    let offline_path = scratch_dir.join("offline.md");
    let candidate_path = scratch_dir.join("candidate.md");
    std::fs::write(
        &storage_path,
        "Decision:\n  id: local-storage\n  title: Use SQLite for local prototype storage\n  status: proposed\n  topic_keys: storage\n  rationale: SQLite keeps local storage durable and replay tests fast without a service.\n  options:\n    - sqlite\n    - postgres\n  chose: sqlite\n",
    )
    .expect("write storage doc");
    std::fs::write(
        &offline_path,
        "Decision:\n  id: offline-cache\n  title: Use SQLite for local offline storage\n  status: proposed\n  topic_keys: offline\n  rationale: SQLite keeps offline data durable without a service.\n  options:\n    - sqlite\n    - indexeddb\n  chose: sqlite\n",
    )
    .expect("write offline doc");

    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        storage_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("storage import succeeds");
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        offline_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("offline import succeeds");
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let latest_after_seed = ledger.latest_offset().expect("latest offset");

    std::fs::write(
        &candidate_path,
        "Decision:\n  id: sqlite-local-offline\n  title: Use SQLite for local offline storage\n  status: proposed\n  topic_keys: storage, offline\n  rationale: SQLite keeps local offline storage durable and replay tests fast without a service.\n  options:\n    - sqlite\n    - postgres\n  chose: sqlite\n",
    )
    .expect("write ambiguous doc");
    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        candidate_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("ambiguous import reports successfully");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    let block = &output["files"][0]["blocks"][0];
    assert_eq!(block["status"], serde_json::json!("duplicate_candidate"));
    assert_eq!(
        block["reviewer_action"],
        serde_json::json!("review_ambiguous_fuzzy_matches")
    );
    let matches = block["similarity_matches"]
        .as_array()
        .expect("ambiguous matches");
    assert_eq!(matches.len(), 2);
    assert_eq!(output["summary"]["events_written"], serde_json::json!(0));
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_seed
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn import_documents_cli_imports_non_duplicate_without_similarity_matches() {
    let hivemind_dir = unique_test_dir("import-document-non-duplicate-ledger");
    let scratch_dir = unique_test_dir("import-document-non-duplicate-docs");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let storage_path = scratch_dir.join("storage.md");
    let notifications_path = scratch_dir.join("notifications.md");
    std::fs::write(
        &storage_path,
        "Decision:\n  id: local-sqlite\n  title: Use SQLite for local prototype storage\n  status: accepted\n  topic_keys: storage, local\n  rationale: SQLite keeps setup small and replay fast for the local prototype.\n  options:\n    - sqlite\n    - postgres\n  chose: sqlite\n",
    )
    .expect("write storage doc");
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        storage_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("storage import succeeds");

    std::fs::write(
        &notifications_path,
        "Decision:\n  id: queued-notifications\n  title: Queue blocker notifications before delivery\n  status: proposed\n  topic_keys: notifications, reliability\n  rationale: A queue lets retries happen independently when Slack is temporarily unavailable.\n  options:\n    - queued delivery\n    - direct send\n  chose: queued delivery\n",
    )
    .expect("write notifications doc");
    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:local",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        notifications_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("non duplicate import succeeds");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid import json");
    let block = &output["files"][0]["blocks"][0];
    assert_eq!(block["status"], serde_json::json!("imported"));
    assert_eq!(output["summary"]["blocks_imported"], serde_json::json!(1));
    assert!(
        block.get("similarity_matches").is_none(),
        "non-duplicates should not carry fuzzy matches"
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
}

#[test]
fn emit_decision_capture_records_codex_and_claude_agent_provenance() {
    let hivemind_dir = unique_test_dir("emit-agent-decision-capture");

    let codex_decision = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.capture",
        "--agent-tool",
        "codex",
        "--agent-session",
        "session-1",
        "--title",
        "Use direct CLI capture for Codex",
        "--rationale",
        "Codex can invoke a deterministic local command from the workspace",
        "--topic-keys",
        "agents,capture",
        "--options",
        "direct-cli,mcp",
        "--chose",
        "direct-cli",
    ]))
    .expect("codex capture succeeds");
    let codex_decision = envelope_value(&codex_decision);

    let claude_decision = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.capture",
        "--agent-tool",
        "claude",
        "--agent-session",
        "session-2",
        "--title",
        "Use direct CLI capture for Claude",
        "--rationale",
        "Claude can call the same command with only identity changed",
        "--topic-keys",
        "agents,capture",
        "--options",
        "direct-cli,hooks",
        "--chose",
        "direct-cli",
    ]))
    .expect("claude capture succeeds");
    let claude_decision = envelope_value(&claude_decision);

    assert_decision_queryable(&hivemind_dir, &codex_decision);
    assert_decision_queryable(&hivemind_dir, &claude_decision);

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 100).expect("events read");
    for (decision_id, actor_id) in [
        (&codex_decision, "agent:codex:session-1"),
        (&claude_decision, "agent:claude:session-2"),
    ] {
        let event = events
            .iter()
            .find(|event| {
                // ubs:ignore: public event types and decision graph IDs are not secrets.
                event.event_type == crate::events::EventType::DecisionProposed
                    && event
                        .payload
                        .get("decision_id")
                        .and_then(|value| value.as_str())
                        == Some(decision_id.as_str())
            })
            .expect("decision proposal exists");

        assert_eq!(event.actor_id, actor_id);
        assert_eq!(event.source, crate::events::EventSource::Agent);
        assert_eq!(event.source_ref.as_deref(), Some(actor_id));

        let proposal_id = event.event_id.expect("proposal has ledger origin");
        let relation_events = events
            .iter()
            // ubs:ignore: public ledger causation IDs are not secrets.
            .filter(|event| event.causation_event_id == Some(proposal_id))
            .collect::<Vec<_>>();
        assert!(!relation_events.is_empty());
        for relation_event in relation_events {
            assert_eq!(relation_event.actor_id, actor_id);
            assert_eq!(relation_event.source, crate::events::EventSource::Agent);
            assert_eq!(relation_event.source_ref.as_deref(), Some(actor_id));
        }
    }

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn emit_decision_capture_records_human_provenance_when_requested() {
    let hivemind_dir = unique_test_dir("emit-human-decision-capture");

    let human_decision = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.capture",
        "--source",
        "human",
        "--actor-id",
        "human:alice",
        "--title",
        "Capture manual Claude Code decisions as human writes",
        "--rationale",
        "A slash command typed by a human should preserve the human as the actor",
        "--topic-keys",
        "agents,capture",
        "--options",
        "manual-slash,agent-inference",
        "--chose",
        "manual-slash",
    ]))
    .expect("human capture succeeds");
    let human_decision = envelope_value(&human_decision);

    assert_decision_queryable(&hivemind_dir, &human_decision);

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 100).expect("events read");
    let event = events
        .iter()
        .find(|event| {
            // ubs:ignore: public event types and decision graph IDs are not secrets.
            event.event_type == crate::events::EventType::DecisionProposed
                && event
                    .payload
                    .get("decision_id")
                    .and_then(|value| value.as_str())
                    == Some(human_decision.as_str())
        })
        .expect("decision proposal exists");

    assert_eq!(event.actor_id, "human:alice");
    assert_eq!(event.source, crate::events::EventSource::Human);
    assert_eq!(event.source_ref.as_deref(), Some("human:alice"));

    let proposal_id = event.event_id.expect("proposal has ledger origin");
    let relation_events = events
        .iter()
        // ubs:ignore: public ledger causation IDs are not secrets.
        .filter(|event| event.causation_event_id == Some(proposal_id))
        .collect::<Vec<_>>();
    assert!(!relation_events.is_empty());
    for relation_event in relation_events {
        assert_eq!(relation_event.actor_id, "human:alice");
        assert_eq!(relation_event.source, crate::events::EventSource::Human);
        assert_eq!(relation_event.source_ref.as_deref(), Some("human:alice"));
    }

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn ingest_slack_thread_creates_queryable_decision_with_slack_provenance() {
    let hivemind_dir = unique_test_dir("ingest-slack-thread");
    let fixture = workspace_fixture("tests/fixtures/slack/thread_with_mention.json");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "ingest",
        "slack-thread",
        "--file",
        fixture.to_str().expect("utf-8 fixture path"),
    ]))
    .expect("ingest succeeds");

    let output: serde_json::Value = serde_json::from_str(&output).expect("json output");
    assert_eq!(output["subcommand"], serde_json::json!("ingest"));
    assert_eq!(output["kind"], serde_json::json!("decision_id"));
    let decision_id = output["value"].as_str().expect("decision id").to_owned();
    assert!(decision_id.starts_with("decision-"));

    assert_decision_queryable(&hivemind_dir, &decision_id);

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 100).expect("events read");
    let proposal = events
        .iter()
        .find(|event| {
            // ubs:ignore: public event types and decision graph IDs are not secrets.
            event.event_type == crate::events::EventType::DecisionProposed
                && event
                    .payload
                    .get("decision_id")
                    .and_then(|value| value.as_str())
                    == Some(decision_id.as_str())
        })
        .expect("proposal event present");
    assert_eq!(proposal.actor_id, "slack:T123:U111");
    assert_eq!(proposal.source, crate::events::EventSource::Slack);
    assert_eq!(
        proposal.source_ref.as_deref(),
        Some("slack://T123/C456/1715970800.000100")
    );

    let proposal_id = proposal.event_id.expect("proposal event id");
    let related: Vec<_> = events
        .iter()
        // ubs:ignore: public ledger causation IDs are not secrets.
        .filter(|event| event.causation_event_id == Some(proposal_id))
        .collect();
    assert!(!related.is_empty(), "proposal must fan out relations");
    for event in &related {
        assert_eq!(event.source, crate::events::EventSource::Slack);
        assert_eq!(
            event.source_ref.as_deref(),
            Some("slack://T123/C456/1715970800.000100")
        );
    }

    let evidence_count = events
        .iter()
        .filter(|event| {
            event.event_type == crate::events::EventType::EvidenceRecorded // ubs:ignore: public event type is not secret material.
                && event.source == crate::events::EventSource::Slack // ubs:ignore: public source classification is not secret material.
        })
        .count();
    assert_eq!(evidence_count, 1);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn ingest_slack_thread_is_idempotent_on_reimport() {
    let hivemind_dir = unique_test_dir("ingest-slack-thread-reimport");
    let fixture = workspace_fixture("tests/fixtures/slack/thread_with_mention.json");

    let args = [
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "ingest",
        "slack-thread",
        "--file",
        fixture.to_str().expect("utf-8 fixture path"),
    ];

    let first: serde_json::Value =
        serde_json::from_str(&run(&Cli::parse_from(args)).expect("first ingest")).unwrap();
    assert_eq!(first["kind"], serde_json::json!("decision_id"));
    let first_decision = first["value"]
        .as_str()
        .expect("first decision id")
        .to_owned();

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let first_event_count = ledger.read(0, 1024).expect("read events").len();

    let second: serde_json::Value =
        serde_json::from_str(&run(&Cli::parse_from(args)).expect("second ingest")).unwrap();
    assert_eq!(second["kind"], serde_json::json!("decision_id_existing"));
    assert_eq!(second["value"].as_str(), Some(first_decision.as_str()));

    let second_event_count = ledger.read(0, 1024).expect("read events").len();
    assert_eq!(
        first_event_count, second_event_count,
        "re-import must not append events"
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn ingest_slack_thread_rejects_thread_without_mention() {
    let hivemind_dir = unique_test_dir("ingest-slack-thread-no-mention");
    let fixture = workspace_fixture("tests/fixtures/slack/thread_without_mention.json");

    let error = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "ingest",
        "slack-thread",
        "--file",
        fixture.to_str().expect("utf-8 fixture path"),
    ]))
    .expect_err("mention gate rejects thread");

    assert!(
        error.to_string().contains("missing required mention"),
        "error should mention gate: {error}"
    );

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    assert!(
        ledger.read(0, 10).expect("read events").is_empty(),
        "no events should have been written"
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

fn workspace_fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

#[cfg(not(feature = "graph-kuzu"))]
#[test]
fn kuzu_backend_requires_feature() {
    let hivemind_dir = unique_test_dir("kuzu-feature-required");
    let cli = Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "--graph-backend",
        "kuzu",
        "query",
        "get_decision",
        "--id",
        "decision-missing",
    ]);

    let error = run(&cli).expect_err("kuzu backend needs feature");

    assert!(error
        .to_string()
        .contains("requires building with --features graph-kuzu"));
}

#[cfg(feature = "graph-kuzu")]
#[test]
fn kuzu_backend_queries_and_dumps_persistent_projection() {
    let hivemind_dir = unique_test_dir("kuzu-query");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Persist query graph",
        "--rationale",
        "Kuzu mode should project SQLite events before reads",
        "--topic-keys",
        "architecture,storage",
        "--options",
        "memory,kuzu",
        "--chose",
        "kuzu",
    ]))
    .expect("emit decision succeeds");

    let query_args = [
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "--graph-backend",
        "kuzu",
        "query",
        "get_relevant_decisions",
        "--topic",
        "architecture",
    ];
    let first_query = run(&Cli::parse_from(query_args)).expect("kuzu query succeeds");
    let second_query = run(&Cli::parse_from(query_args)).expect("repeated kuzu query succeeds");
    let mut first_json: serde_json::Value =
        serde_json::from_str(&first_query).expect("first query json");
    let mut second_json: serde_json::Value =
        serde_json::from_str(&second_query).expect("second query json");
    first_json["latency_ms"] = serde_json::json!(0);
    second_json["latency_ms"] = serde_json::json!(0);

    assert_eq!(first_json, second_json);
    assert_eq!(first_json["result_count"], serde_json::json!(1));
    assert_eq!(first_json["data"][0]["id"], serde_json::json!(decision_id));
    assert!(hivemind_dir.join("graph.kuzu").exists());

    let dot = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "--graph-backend",
        "kuzu",
        "dump",
        "--format",
        "dot",
    ]))
    .expect("kuzu dump succeeds");
    assert!(dot.contains("Persist query graph"));

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn format_error_outputs_structured_json() {
    let error = HivemindError::Command(CommandError::Validation("bad input".to_owned()));
    let output = format_error(true, &error);
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid json error");

    assert_eq!(
        output
            .pointer("/error/exit_code")
            .and_then(|value| value.as_i64()),
        Some(2)
    );
    assert!(output
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .expect("message")
        .contains("bad input"));
}

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-{name}-{}", uuid::Uuid::new_v4()))
}

fn write_simple_pdf(path: &std::path::Path, lines: &[&str]) {
    let mut content = String::from("BT\n/F1 12 Tf\n72 720 Td\n");
    for line in lines {
        content.push_str(&format!("({}) Tj\n0 -14 Td\n", escape_pdf_text(line)));
    }
    content.push_str("ET\n");

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{}endstream", content.len(), content),
    ];

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
    }
    let xref_offset = pdf.len();
    pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
    ));
    std::fs::write(path, pdf).expect("write PDF fixture");
}

fn escape_pdf_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn envelope_value(output: &str) -> String {
    let output: serde_json::Value = serde_json::from_str(output).expect("valid json output");
    assert_eq!(
        output.get("kind").and_then(|value| value.as_str()),
        Some("decision_id")
    );
    output
        .get("value")
        .and_then(|value| value.as_str())
        .expect("decision id")
        .to_owned()
}

fn assert_decision_queryable(hivemind_dir: &std::path::Path, decision_id: &str) {
    let query = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "get_decision",
        "--id",
        decision_id,
    ]))
    .expect("query succeeds");
    let query: serde_json::Value = serde_json::from_str(&query).expect("valid query json");
    assert_eq!(query["result_count"], serde_json::json!(1));
    assert_eq!(query["data"]["id"], serde_json::json!(decision_id));
}
