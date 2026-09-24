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
fn cli_version_is_cargo_semver_plus_build_sha() {
    let command = Cli::command();
    let version = command.get_version().unwrap();
    assert_eq!(version, crate::VERSION);
    let (semver, sha) = version.split_once('+').expect("version must be semver+sha");
    assert_eq!(semver, env!("CARGO_PKG_VERSION"));
    assert!(!sha.is_empty(), "build sha must not be empty");
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
        "Need durable ingestion under heavy load",
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
fn emit_hypothesis_recorded_defaults_to_assumption_kind() {
    let hivemind_dir = unique_test_dir("emit-hypothesis-default");
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "hypothesis.recorded",
        "--statement",
        "The cache will help",
    ]);

    let output = run(&cli).expect("emit hypothesis succeeds");
    assert!(output.starts_with("hypothesis-"));
}

#[test]
fn emit_hypothesis_recorded_accepts_bet_kind_check_by_and_would_change_if() {
    let hivemind_dir = unique_test_dir("emit-hypothesis-bet");
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "hypothesis.recorded",
        "--statement",
        "Latency stays under 50ms at 10x load",
        "--kind",
        "bet",
        "--check-by",
        "2026-10-01",
        "--would-change-if",
        "A 10x load test shows p99 above 50ms",
    ]);

    let output = run(&cli).expect("emit bet hypothesis succeeds");
    assert!(output.starts_with("hypothesis-"));
}

#[test]
fn emit_hypothesis_recorded_rejects_malformed_check_by() {
    let hivemind_dir = unique_test_dir("emit-hypothesis-bad-date");
    let cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "hypothesis.recorded",
        "--statement",
        "A bet with a garbled date",
        "--kind",
        "bet",
        "--check-by",
        "not-a-date",
    ]);

    let error = run(&cli).expect_err("malformed --check-by must be refused");
    assert!(
        error.to_string().contains("--check-by"),
        "unexpected error: {error}"
    );
}

#[test]
fn emit_relation_added_follows_from_links_decisions() {
    let hivemind_dir = unique_test_dir("emit-relation-follows-from");
    let dir_str = hivemind_dir.to_str().expect("utf-8 temp path").to_owned();

    let earlier_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        &dir_str,
        "emit",
        "decision.proposed",
        "--title",
        "Earlier decision",
        "--rationale",
        "Established earlier as a standing premise decision",
        "--topic-keys",
        "infra",
        "--options",
        "only",
    ]))
    .expect("propose earlier decision");

    let later_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        &dir_str,
        "emit",
        "decision.proposed",
        "--title",
        "Later decision",
        "--rationale",
        "Follows from the earlier one",
        "--topic-keys",
        "infra",
        "--options",
        "only",
    ]))
    .expect("propose later decision");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        &dir_str,
        "emit",
        "relation.added",
        "--kind",
        "follows-from",
        "--from",
        later_id.trim(),
        "--to",
        earlier_id.trim(),
    ]))
    .expect("emit relation.added FOLLOWS_FROM succeeds");
    assert!(!output.is_empty());
}

#[test]
fn emit_relation_added_based_on_names_follows_from_when_target_is_a_decision() {
    let hivemind_dir = unique_test_dir("emit-relation-based-on-decision-target");
    let dir_str = hivemind_dir.to_str().expect("utf-8 temp path").to_owned();

    let decision_a = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        &dir_str,
        "emit",
        "decision.proposed",
        "--title",
        "Decision A",
        "--rationale",
        "Rationale long enough for the readable floor",
        "--topic-keys",
        "infra",
        "--options",
        "only",
    ]))
    .expect("propose decision A");

    let decision_b = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        &dir_str,
        "emit",
        "decision.proposed",
        "--title",
        "Decision B",
        "--rationale",
        "Rationale long enough for the readable floor",
        "--topic-keys",
        "infra",
        "--options",
        "only",
    ]))
    .expect("propose decision B");

    let error = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-1",
        "--hivemind-dir",
        &dir_str,
        "emit",
        "relation.added",
        "--kind",
        "based-on",
        "--from",
        decision_a.trim(),
        "--to",
        decision_b.trim(),
    ]))
    .expect_err("BASED_ON must keep refusing a decision target");
    assert!(
        error.to_string().contains("FOLLOWS_FROM"),
        "error should name FOLLOWS_FROM: {error}"
    );
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
fn disagree_cli_records_agent_source_for_agent_actor() {
    // hivemind-xm93: run_disagree previously hardcoded EventProvenance::human
    // regardless of --actor, so an agent-initiated disagree was misattributed as
    // human-sourced (AGENTS.md #2). An `agent:`-shaped actor must record source=agent.
    let hivemind_dir = unique_test_dir("disagree-cli-agent-source");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
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
        "human:bob",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.accepted",
        "--decision-id",
        &decision_id,
    ]))
    .expect("decision accepted");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent:claude:tenv3-smoke-test",
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
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid disagree json");
    let event_id = output["event_id"].as_u64().expect("event id");

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 20).expect("events read");
    let rejected = events
        .iter()
        // ubs:ignore: public ledger event IDs are not secrets.
        .find(|event| event.event_id == Some(event_id))
        .expect("rejected event");
    assert_eq!(rejected.source, crate::events::EventSource::Agent);
    assert_eq!(rejected.actor_id, "agent:claude:tenv3-smoke-test");

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
        "Fastest path to ship given the deadline",
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
        "--bet",
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
        "--bet",
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
fn supersede_cli_records_agent_source_for_agent_actor() {
    // hivemind-xm93: run_supersede previously hardcoded EventProvenance::human
    // regardless of --actor, so an agent-initiated supersede was misattributed as
    // human-sourced (AGENTS.md #2). An `agent:`-shaped actor must record source=agent.
    let hivemind_dir = unique_test_dir("supersede-cli-agent-source");
    let old_decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Use shared admin token",
        "--rationale",
        "Fastest path to ship given the deadline",
        "--topic-keys",
        "auth",
        "--options",
        "shared-token",
    ]))
    .expect("decision proposed");

    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent:claude:tenv3-smoke-test",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "supersede",
        "--old",
        &old_decision_id,
        "--bet",
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
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid supersede json");
    let superseded_event_id = output["superseded_event_id"]
        .as_u64()
        .expect("superseded event id");

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 20).expect("events read");
    let superseded = events
        .iter()
        // ubs:ignore: public ledger event IDs are not secrets.
        .find(|event| event.event_id == Some(superseded_event_id))
        .expect("superseded event");
    assert_eq!(superseded.source, crate::events::EventSource::Agent);
    assert_eq!(superseded.actor_id, "agent:claude:tenv3-smoke-test");

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

fn disagree_fluent_description_resolves_uniquely_and_records_body(
    backend: &TestBackend,
) -> CliTestResult {
    let decision_id = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:alice",
            "emit",
            "decision.proposed",
            "--title",
            "Adopt async billing queue",
            "--rationale",
            "Durability beats latency here",
            "--topic-keys",
            "billing",
            "--options",
            "async",
        ],
    )))?;

    let output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "--json",
            "disagree",
            "async billing queue",
            "--reason",
            "underestimates operational cost",
        ],
    )))?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    ensure_json_eq(
        &output["decision_id"],
        serde_json::json!(decision_id),
        "disagree resolves the unique matching decision",
    )?;
    ensure_json_eq(
        &output["decision_status"],
        serde_json::json!("rejected"),
        "disagree flips status to rejected",
    )?;

    Ok(())
}

#[test]
fn disagree_fluent_description_resolves_uniquely_and_records() -> CliTestResult {
    disagree_fluent_description_resolves_uniquely_and_records_body(&TestBackend::sqlite(
        "disagree-fluent-resolve",
    ))
}

#[test]
fn disagree_fluent_description_resolves_uniquely_and_records_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("disagree-fluent-resolve-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    disagree_fluent_description_resolves_uniquely_and_records_body(&backend)
}

fn disagree_fluent_ambiguous_description_short_circuits_without_writing_body(
    backend: &TestBackend,
) -> CliTestResult {
    for topic in ["billing", "notifications"] {
        run(&Cli::parse_from(cli_args(
            backend,
            &[
                "--actor",
                "actor:alice",
                "emit",
                "decision.proposed",
                "--title",
                &format!("Adopt async queue for {topic}"),
                "--rationale",
                "because those are the reasons we discussed",
                "--topic-keys",
                topic,
                "--options",
                "async",
            ],
        )))?;
    }

    let output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "--json",
            "disagree",
            "adopt async queue",
            "--reason",
            "should not apply to either",
        ],
    )))?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    ensure_json_eq(
        &output["data"]["outcome"],
        serde_json::json!("ambiguous"),
        "two equally-matching decisions must short-circuit as ambiguous",
    )?;
    ensure_eq(
        output["data"]["candidates"]
            .as_array()
            .expect("candidates array")
            .len(),
        2,
        "both decisions are listed as candidates",
    )?;

    // Neither decision was written to: both remain in their original "proposed" status.
    let search = run(&Cli::parse_from(cli_args(
        backend,
        &["query", "search_decisions", "--q", "adopt async queue"],
    )))?;
    let search: serde_json::Value = serde_json::from_str(&search)?;
    for item in search["data"]["items"].as_array().expect("items array") {
        ensure_json_eq(
            &item["decision"]["status"],
            serde_json::json!("proposed"),
            "ambiguous disagree must not mutate either candidate",
        )?;
    }

    Ok(())
}

#[test]
fn disagree_fluent_ambiguous_description_short_circuits_without_writing() -> CliTestResult {
    disagree_fluent_ambiguous_description_short_circuits_without_writing_body(&TestBackend::sqlite(
        "disagree-fluent-ambiguous",
    ))
}

#[test]
fn disagree_fluent_ambiguous_description_short_circuits_without_writing_postgres() -> CliTestResult
{
    let Some(backend) = TestBackend::postgres("disagree-fluent-ambiguous-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    disagree_fluent_ambiguous_description_short_circuits_without_writing_body(&backend)
}

fn disagree_fluent_pick_disambiguates_and_hash_handle_reuses_it_body(
    backend: &TestBackend,
) -> CliTestResult {
    let mut decision_ids = Vec::new();
    for topic in ["billing", "notifications"] {
        decision_ids.push(run(&Cli::parse_from(cli_args(
            backend,
            &[
                "--actor",
                "actor:alice",
                "emit",
                "decision.proposed",
                "--title",
                &format!("Adopt async queue for {topic}"),
                "--rationale",
                "because those are the reasons we discussed",
                "--topic-keys",
                topic,
                "--options",
                "async",
            ],
        )))?);
    }

    // --pick 1 disambiguates deterministically (newest-first: "notifications" was proposed last).
    let picked = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "--json",
            "disagree",
            "adopt async queue",
            "--pick",
            "1",
            "--reason",
            "picked via --pick",
        ],
    )))?;
    let picked: serde_json::Value = serde_json::from_str(&picked)?;
    ensure_json_eq(
        &picked["decision_id"],
        serde_json::json!(decision_ids[1]),
        "--pick 1 selects the newest (notifications) candidate",
    )?;

    // A later invocation's bare `#2` reads the candidate list this ambiguous-then-picked call
    // wrote to the continuation file, addressing the older (billing) candidate without
    // re-resolving the description.
    let handled = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:carol",
            "--json",
            "disagree",
            "#2",
            "--reason",
            "picked via #N handle",
        ],
    )))?;
    let handled: serde_json::Value = serde_json::from_str(&handled)?;
    ensure_json_eq(
        &handled["decision_id"],
        serde_json::json!(decision_ids[0]),
        "#2 addresses the second-listed (billing) candidate from the previous resolver call",
    )?;

    Ok(())
}

#[test]
fn disagree_fluent_pick_disambiguates_and_hash_handle_reuses_it() -> CliTestResult {
    disagree_fluent_pick_disambiguates_and_hash_handle_reuses_it_body(&TestBackend::sqlite(
        "disagree-fluent-pick",
    ))
}

#[test]
fn disagree_fluent_pick_disambiguates_and_hash_handle_reuses_it_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("disagree-fluent-pick-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    disagree_fluent_pick_disambiguates_and_hash_handle_reuses_it_body(&backend)
}

fn disagree_fluent_topic_narrows_ambiguous_to_resolved_body(
    backend: &TestBackend,
) -> CliTestResult {
    for topic in ["billing", "notifications"] {
        run(&Cli::parse_from(cli_args(
            backend,
            &[
                "--actor",
                "actor:alice",
                "emit",
                "decision.proposed",
                "--title",
                &format!("Adopt async queue for {topic}"),
                "--rationale",
                "because those are the reasons we discussed",
                "--topic-keys",
                topic,
                "--options",
                "async",
            ],
        )))?;
    }

    let output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "--json",
            "disagree",
            "adopt async queue",
            "--topic",
            "billing",
            "--reason",
            "narrowed by topic",
        ],
    )))?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    ensure_json_eq(
        &output["decision_status"],
        serde_json::json!("rejected"),
        "--topic narrows the otherwise-ambiguous match down to one candidate",
    )?;

    Ok(())
}

#[test]
fn disagree_fluent_topic_narrows_ambiguous_to_resolved() -> CliTestResult {
    disagree_fluent_topic_narrows_ambiguous_to_resolved_body(&TestBackend::sqlite(
        "disagree-fluent-topic",
    ))
}

#[test]
fn disagree_fluent_topic_narrows_ambiguous_to_resolved_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("disagree-fluent-topic-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    disagree_fluent_topic_narrows_ambiguous_to_resolved_body(&backend)
}

fn supersede_fluent_description_resolves_uniquely_and_records_body(
    backend: &TestBackend,
) -> CliTestResult {
    let old_decision_id = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:alice",
            "emit",
            "decision.proposed",
            "--title",
            "Use shared admin token",
            "--rationale",
            "Fastest path to ship given the deadline",
            "--topic-keys",
            "auth",
            "--options",
            "shared-token",
        ],
    )))?;

    let output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "--json",
            "supersede",
            "shared admin token",
            "--bet",
            "--title",
            "Use scoped service tokens",
            "--rationale",
            "Scoped tokens preserve audit boundaries",
            "--options",
            "scoped-service-tokens",
            "--chose",
            "scoped-service-tokens",
        ],
    )))?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    ensure_json_eq(
        &output["old_decision_id"],
        serde_json::json!(old_decision_id),
        "supersede resolves the unique matching decision via positional description",
    )?;
    ensure_json_eq(
        &output["old_decision_status"],
        serde_json::json!("superseded"),
        "supersede flips the resolved decision's status to superseded",
    )?;

    Ok(())
}

#[test]
fn supersede_fluent_description_resolves_uniquely_and_records() -> CliTestResult {
    supersede_fluent_description_resolves_uniquely_and_records_body(&TestBackend::sqlite(
        "supersede-fluent-resolve",
    ))
}

#[test]
fn supersede_fluent_description_resolves_uniquely_and_records_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("supersede-fluent-resolve-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    supersede_fluent_description_resolves_uniquely_and_records_body(&backend)
}

fn supersede_fluent_ambiguous_description_short_circuits_without_writing_body(
    backend: &TestBackend,
) -> CliTestResult {
    for topic in ["billing", "notifications"] {
        run(&Cli::parse_from(cli_args(
            backend,
            &[
                "--actor",
                "actor:alice",
                "emit",
                "decision.proposed",
                "--title",
                &format!("Adopt async queue for {topic}"),
                "--rationale",
                "because those are the reasons we discussed",
                "--topic-keys",
                topic,
                "--options",
                "async",
            ],
        )))?;
    }

    let output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "--json",
            "supersede",
            "adopt async queue",
            "--bet",
            "--title",
            "Adopt sync queue instead",
            "--rationale",
            "should not apply to either",
            "--options",
            "sync",
            "--chose",
            "sync",
        ],
    )))?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    ensure_json_eq(
        &output["data"]["outcome"],
        serde_json::json!("ambiguous"),
        "two equally-matching decisions must short-circuit supersede as ambiguous, never guess which to supersede",
    )?;
    ensure_eq(
        output["data"]["candidates"]
            .as_array()
            .expect("candidates array")
            .len(),
        2,
        "both decisions are listed as candidates",
    )?;

    // Neither decision was superseded: both remain "proposed" and no replacement was created.
    let search = run(&Cli::parse_from(cli_args(
        backend,
        &["query", "search_decisions", "--q", "adopt async queue"],
    )))?;
    let search: serde_json::Value = serde_json::from_str(&search)?;
    let items = search["data"]["items"].as_array().expect("items array");
    ensure_eq(
        items.len(),
        2,
        "ambiguous supersede must not create a replacement decision",
    )?;
    for item in items {
        ensure_json_eq(
            &item["decision"]["status"],
            serde_json::json!("proposed"),
            "ambiguous supersede must not mutate either candidate",
        )?;
    }

    Ok(())
}

#[test]
fn supersede_fluent_ambiguous_description_short_circuits_without_writing() -> CliTestResult {
    supersede_fluent_ambiguous_description_short_circuits_without_writing_body(
        &TestBackend::sqlite("supersede-fluent-ambiguous"),
    )
}

#[test]
fn supersede_fluent_ambiguous_description_short_circuits_without_writing_postgres() -> CliTestResult
{
    let Some(backend) = TestBackend::postgres("supersede-fluent-ambiguous-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    supersede_fluent_ambiguous_description_short_circuits_without_writing_body(&backend)
}

/// A no-feature build must reject `--database-url` before touching the ledger
/// at all, not silently fall back to SQLite. Gated to the no-feature build:
/// with `shared-backend-postgres` compiled in, this URL would instead attempt
/// a real (failing, since it's a placeholder) Postgres connection.
#[cfg(not(feature = "shared-backend-postgres"))]
#[test]
fn database_url_without_feature_errors_before_any_write() -> CliTestResult {
    let hivemind_dir = unique_test_dir("database-url-no-feature");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");

    let result = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:alice",
        "--hivemind-dir",
        dir,
        "--database-url",
        "postgres://user:pass@127.0.0.1/hivemind",
        "emit",
        "decision.proposed",
        "--title",
        "Should never be written",
        "--rationale",
        "the feature is not compiled in",
        "--topic-keys",
        "test",
        "--options",
        "a",
    ]));
    let error = result.expect_err("--database-url without the feature must fail, not write");
    ensure(
        error.to_string().contains("shared-backend-postgres"),
        "error must name the missing feature",
    )?;
    ensure(
        !hivemind_dir.exists(),
        "must not have created a SQLite ledger as a fallback before failing",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn query_chain_and_why_aliases_resolve_by_description() -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-chain-why-fluent");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let old_decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:alice",
        "--hivemind-dir",
        dir,
        "emit",
        "decision.proposed",
        "--title",
        "Use shared admin token",
        "--rationale",
        "Fastest path to ship given the deadline",
        "--topic-keys",
        "auth",
        "--options",
        "shared-token",
    ]))?;
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:bob",
        "--hivemind-dir",
        dir,
        "supersede",
        "--old",
        &old_decision_id,
        "--bet",
        "--title",
        "Use scoped service tokens",
        "--rationale",
        "Scoped tokens preserve audit boundaries",
        "--options",
        "scoped-service-tokens",
        "--chose",
        "scoped-service-tokens",
    ]))?;

    let chain = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir,
        "query",
        "chain",
        "shared admin token",
    ]))?;
    let chain: serde_json::Value = serde_json::from_str(&chain)?;
    ensure_eq(
        chain["data"]["decision_ids"]
            .as_array()
            .expect("chain array")
            .len(),
        2,
        "chain alias resolves the description and walks the supersession chain",
    )?;
    ensure_json_eq(
        &chain["data"]["decision_ids"][0],
        serde_json::json!(old_decision_id),
        "chain starts at the resolved (oldest) decision",
    )?;

    let why = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir,
        "query",
        "why",
        "shared admin token",
    ]))?;
    let why: serde_json::Value = serde_json::from_str(&why)?;
    ensure_json_eq(
        &why["data"]["root"]["id"],
        serde_json::json!(old_decision_id),
        "why alias resolves the description to the decision's neighborhood root",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn query_why_answers_a_natural_question_with_the_why() -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-why-natural-question");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
        "--hivemind-dir",
        dir,
        "emit",
        "decision.proposed",
        "--title",
        "Demo cell storage moves to shared Postgres backend instead of per-host SQLite",
        "--rationale",
        "One shared ledger keeps every reader consistent without per-host sync",
        "--topic-keys",
        "storage",
        "--options",
        "Shared Postgres backend,Per-host SQLite",
        "--chose",
        "Shared Postgres backend",
    ]))?;

    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "query",
        "why",
        "why did we move the demo cell to shared Postgres",
        "--summary",
    ]))?;
    for expected in [
        "decision: Demo cell storage moves to shared Postgres backend instead of per-host SQLite",
        "rationale: One shared ledger keeps every reader consistent without per-host sync",
        "chose: Shared Postgres backend",
        "rejected: Per-host SQLite",
        "human:alice",
        "label=Shared Postgres backend",
        "label=Per-host SQLite",
    ] {
        ensure(
            summary.contains(expected),
            &format!("why --summary should contain {expected:?}, got:\n{summary}"),
        )?;
    }

    let json = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir,
        "query",
        "why",
        "why did we move the demo cell to shared Postgres",
    ]))?;
    let json: serde_json::Value = serde_json::from_str(&json)?;
    ensure_json_eq(
        &json["data"]["root"]["id"],
        serde_json::json!(decision_id),
        "the question resolves to the captured decision",
    )?;
    ensure_json_eq(
        &json["data"]["root"]["rationale"],
        serde_json::json!("One shared ledger keeps every reader consistent without per-host sync"),
        "root carries the rationale",
    )?;
    ensure_json_eq(
        &json["data"]["root"]["chosen_option"]["label"],
        serde_json::json!("Shared Postgres backend"),
        "root carries the chosen option label",
    )?;
    ensure_json_eq(
        &json["data"]["root"]["rejected_options"][0]["label"],
        serde_json::json!("Per-host SQLite"),
        "root carries the rejected option label",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn query_verify_alias_returns_decision_brief() -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-verify-fluent");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
        "--hivemind-dir",
        dir,
        "emit",
        "decision.proposed",
        "--title",
        "Adopt async billing queue",
        "--rationale",
        "Durability beats latency here",
        "--topic-keys",
        "billing",
        "--options",
        "async,sync",
        "--chose",
        "async",
    ]))?;

    let verify = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir,
        "query",
        "verify",
        "async billing queue",
    ]))?;
    let verify: serde_json::Value = serde_json::from_str(&verify)?;
    ensure_json_eq(
        &verify["data"]["decision_id"],
        serde_json::json!(decision_id),
        "verify resolves the description to a DecisionBrief for the matching decision",
    )?;
    ensure_json_eq(
        &verify["data"]["title"],
        serde_json::json!("Adopt async billing queue"),
        "DecisionBrief leads with the decision's title",
    )?;
    ensure_json_eq(
        &verify["data"]["still_holds"]["held_up"],
        serde_json::json!(true),
        "a fresh, unsuperseded, uncontested decision still holds",
    )?;
    ensure(
        verify["data"]["chosen_option"]["option_id"]
            .as_str()
            .is_some(),
        "DecisionBrief resolves the chosen option",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn query_verify_shows_what_a_decision_rests_on_and_goes_stale_when_the_premise_is_superseded(
) -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-verify-rests-on");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let propose = |title: &str, rationale: &str| {
        run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "human:alice",
            "--hivemind-dir",
            dir,
            "emit",
            "decision.proposed",
            "--title",
            title,
            "--rationale",
            rationale,
            "--topic-keys",
            "grounding",
            "--options",
            "only",
        ]))
    };
    let goal_id = propose(
        "Ship the hosted MVP",
        "The hosted MVP is the goal everything else follows from",
    )?;
    let derived_id = propose(
        "Use Postgres for the MVP",
        "Consistent with shipping the hosted MVP",
    )?;
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
        "--hivemind-dir",
        dir,
        "emit",
        "relation.added",
        "--kind",
        "follows-from",
        "--from",
        derived_id.trim(),
        "--to",
        goal_id.trim(),
    ]))?;

    let verify_json =
        |description: &str| -> std::result::Result<serde_json::Value, Box<dyn std::error::Error>> {
            let output = run(&Cli::parse_from([
                "hivemind",
                "--json",
                "--hivemind-dir",
                dir,
                "query",
                "verify",
                description,
            ]))?;
            Ok(serde_json::from_str(&output)?)
        };

    // The premise is listed with its state and who attributed it: added after capture, since the
    // relation was emitted separately from the proposal.
    let before = verify_json("Use Postgres for the MVP")?;
    ensure_json_eq(
        &before["data"]["grounding_state"],
        serde_json::json!("grounded"),
        "a decision that follows from a prior decision is grounded",
    )?;
    ensure_json_eq(
        &before["data"]["rests_on"][0]["label"],
        serde_json::json!("Ship the hosted MVP"),
        "the premise is named by its title",
    )?;
    ensure_json_eq(
        &before["data"]["rests_on"][0]["state"],
        serde_json::json!("holds"),
        "the premise still stands",
    )?;
    ensure_json_eq(
        &before["data"]["rests_on"][0]["added"]["when"],
        serde_json::json!("later"),
        "a link emitted after the proposal is attributed later, not at capture",
    )?;
    ensure_json_eq(
        &before["data"]["rests_on"][0]["added"]["actor_id"],
        serde_json::json!("human:alice"),
        "and says who attributed it",
    )?;
    ensure_json_eq(
        &before["data"]["still_holds"]["held_up"],
        serde_json::json!(true),
        "a standing premise keeps the decision standing",
    )?;

    // Supersede the premise: the decision that rests on it is stale, and says why.
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:alice",
        "--hivemind-dir",
        dir,
        "supersede",
        "--old",
        goal_id.trim(),
        "--title",
        "Ship the self-hosted MVP",
        "--rationale",
        "Self-hosting replaces the hosted goal for now",
        "--options",
        "self-hosted",
        "--chose",
        "self-hosted",
        "--bet",
    ]))?;
    let after = verify_json("Use Postgres for the MVP")?;
    ensure_json_eq(
        &after["data"]["still_holds"]["held_up"],
        serde_json::json!(false),
        "a superseded premise makes the decision stale",
    )?;
    ensure_json_eq(
        &after["data"]["still_holds"]["reasons"][0]["kind"],
        serde_json::json!("premise_superseded"),
        "the reason names the premise, not a generic staleness",
    )?;
    ensure_json_eq(
        &after["data"]["rests_on"][0]["state"],
        serde_json::json!("superseded"),
        "the premise is shown as superseded",
    )?;

    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "query",
        "--summary",
        "verify",
        "Use Postgres for the MVP",
    ]))?;
    ensure(
        summary.contains("still holds: NO: premise superseded"),
        "the text answer says no, and why",
    )?;
    ensure(
        summary.contains("SUPERSEDED") && summary.contains("Ship the self-hosted MVP"),
        "the text answer names the superseding decision",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn query_compact_view_fluent_resolves_by_description() -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-compact-view-fluent");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:alice",
        "--hivemind-dir",
        dir,
        "emit",
        "decision.proposed",
        "--title",
        "Adopt async billing queue",
        "--rationale",
        "Durability beats latency here",
        "--topic-keys",
        "billing",
        "--options",
        "async",
    ]))?;

    let compact = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir,
        "query",
        "compact-view",
        "async billing queue",
    ]))?;
    let compact: serde_json::Value = serde_json::from_str(&compact)?;
    ensure_json_eq(
        &compact["data"]["decision"]["id"],
        serde_json::json!(decision_id),
        "compact-view resolves the description before rendering the view",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn query_fluent_verb_reports_not_found_for_no_match() -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-fluent-not-found");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "actor:alice",
        "--hivemind-dir",
        dir,
        "emit",
        "decision.proposed",
        "--title",
        "Adopt async billing queue",
        "--rationale",
        "Durability beats latency here",
        "--topic-keys",
        "billing",
        "--options",
        "async",
    ]))?;

    let output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir,
        "query",
        "verify",
        "nonexistent decision about widgets",
    ]))?;
    let output: serde_json::Value = serde_json::from_str(&output)?;
    ensure_json_eq(
        &output["data"]["outcome"],
        serde_json::json!("not_found"),
        "a fluent verb reports not_found rather than erroring on zero matches",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
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
        "Need durable ingestion under heavy load",
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
        // hivemind-zdsh.8: --chose self-accepts by default, so this decision is
        // `accepted`, not `proposed`.
        "accepted",
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
        // hivemind-zdsh.10: option ids are opaque now (no longer a slug of the label), so
        // option.id no longer matches "gateway". The CLI's auto-generated description
        // ("Option generated from CLI value 'gateway'") does, alongside option.label.
        serde_json::json!(["option.description", "option.label"])
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
fn recall_answers_its_own_documented_question_form() -> CliTestResult {
    let hivemind_dir = unique_test_dir("query-recall-question");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    for (title, rationale) in [
        (
            "An agent's personal project is per agent kind and tool, never per session",
            "A session id changes every run, so the project would fragment across sessions",
        ),
        (
            "Project handles are lowercase slugs of at most forty characters",
            "Short stable slugs stay readable in URLs and in the command line",
        ),
    ] {
        run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "human:alice",
            "--hivemind-dir",
            dir,
            "emit",
            "decision.proposed",
            "--title",
            title,
            "--rationale",
            rationale,
            "--topic-keys",
            "projects",
            "--options",
            "Adopt this,Leave it open",
            "--chose",
            "Adopt this",
        ]))?;
    }

    let recall =
        |question: &str, extra: &[&str]| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
            let mut argv = vec![
                "hivemind",
                "--hivemind-dir",
                dir,
                "query",
                "recall",
                question,
            ];
            argv.extend_from_slice(extra);
            Ok(serde_json::from_str(&run(&Cli::parse_from(argv))?)?)
        };

    // recall's own argument-hint: "what did we decide about X".
    let asked = recall(
        "what did we decide about projects",
        &["--topic", "projects"],
    )?;
    ensure_json_eq(
        &asked["result_count"],
        serde_json::json!(2),
        "the question form returns the projects decisions",
    )?;
    ensure_json_eq(
        &asked["data"]["ignored_words"],
        serde_json::json!(["what", "did", "we", "decide", "about"]),
        "the dropped question words are reported, not silently discarded",
    )?;
    ensure_json_eq(
        &asked["data"]["query"],
        serde_json::json!("what did we decide about projects"),
        "the question is echoed as asked",
    )?;

    // Without the topic filter the content word still finds them.
    let bare = recall("what did we decide about projects", &[])?;
    ensure_json_eq(
        &bare["result_count"],
        serde_json::json!(2),
        "content word alone",
    )?;

    // A question made only of question words adds no filter: the topic decides.
    let only_question = recall("what did we decide", &["--topic", "projects"])?;
    ensure_json_eq(
        &only_question["result_count"],
        serde_json::json!(2),
        "a bare question with --topic lists the topic",
    )?;

    // A content word nothing contains still finds nothing, and says which words were ignored.
    let none = recall("what did we decide about billing", &[])?;
    ensure_json_eq(
        &none["result_count"],
        serde_json::json!(0),
        "no decision about billing",
    )?;
    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "query",
        "--summary",
        "recall",
        "what did we decide about billing",
    ]))?;
    ensure(
        summary.contains("ignored question words: what did we decide about"),
        &format!("empty recall names the ignored words, got: {summary}"),
    )?;

    // A query with no question words is unchanged and reports nothing ignored.
    let plain = recall("per agent kind", &[])?;
    ensure_json_eq(&plain["result_count"], serde_json::json!(1), "plain query")?;
    ensure(
        plain["data"].get("ignored_words").is_none(),
        "no ignored_words key when nothing was dropped",
    )?;

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    Ok(())
}

#[test]
fn digest_cli_returns_decisions_in_window() {
    let hivemind_dir = unique_test_dir("digest");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-digest-test",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.proposed",
        "--title",
        "Pick serialization format",
        "--rationale",
        "MessagePack is more compact than JSON for our payloads",
        "--topic-keys",
        "api,serialization",
        "--options",
        "json,msgpack",
        "--chose",
        "msgpack",
    ]))
    .expect("emit decision succeeds");

    // JSON mode: structured digest response
    let out = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "digest",
        "--window",
        "7d",
    ]))
    .expect("digest query succeeds");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid digest json"); // ubs:ignore: test-only; panicking is correct
    let entries = parsed["data"]["entries"].as_array().expect("entries array"); // ubs:ignore: test-only; panicking is correct
    assert!(
        entries
            .iter()
            .any(|e| e["decision_id"].as_str() == Some(decision_id.as_str())),
        "digest must include the emitted decision"
    ); // ubs:ignore: test-only assertion
    let cited = parsed["data"]["cited_decision_ids"]
        .as_array()
        .expect("cited_decision_ids array"); // ubs:ignore: test-only; panicking is correct
    assert!(
        cited
            .iter()
            .any(|id| id.as_str() == Some(decision_id.as_str())),
        "decision_id must appear in cited_decision_ids"
    ); // ubs:ignore: test-only assertion

    // Summary mode: prose output
    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "digest",
        "--window",
        "7d",
        "--summary",
    ]))
    .expect("digest summary succeeds");
    assert!(
        summary.contains("Decision Digest"),
        "summary must contain header"
    ); // ubs:ignore: test-only assertion
    assert!(
        summary.contains("Pick serialization format"),
        "summary must mention the decision title"
    ); // ubs:ignore: test-only assertion
       // hivemind-zdsh.10: option ids are opaque now, so options attached in the same proposal
       // no longer have a stable relative order (previously an accident of the old id scheme
       // embedding the label — sorting by id happened to sort alphabetically by label). Assert
       // content, not a specific order.
    assert!(
        summary.contains("Options:")
            && summary.contains("json")
            && summary.contains("msgpack")
            && summary.contains("Chose: msgpack"),
        "summary must render option labels, not raw option ids (hivemind-zdsh.3): {summary}"
    ); // ubs:ignore: test-only assertion
    assert!(
        !summary.contains("Cited:"),
        "summary must not repeat every decision_id again in a trailing footer \
         (hivemind-zdsh.3) — each already appears in its own bullet: {summary}"
    ); // ubs:ignore: test-only assertion

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_with_decided_by_is_the_acceptance_test_for_ledger_fidelity() {
    // hivemind-zdsh.3 acceptance test: capture Alex's ruling itself — a decision he made,
    // recorded by the capturing agent — with correct attribution, and confirm the digest
    // renders it readably. decided_by=human:alex.knips@gmail.com must appear as who decided;
    // agent:claude:hivemind-crew (the recorder) must appear as who recorded it — neither
    // hidden, both visible in the same "By:" line — and the decision must land as accepted,
    // not stuck at proposed.
    let hivemind_dir = unique_test_dir("ledger-fidelity-acceptance");
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.capture",
        "--bet",
        "--agent-tool",
        "claude",
        "--agent-session",
        "hivemind-crew",
        "--title",
        "Ledger must distinguish who decided from who recorded",
        "--rationale",
        "If the human delegates small decisions to agents, we track that; when the agent \
         asks the human to choose and the human does, the human made the decision. The agent \
         that wrote it down is the recorder, not the decider.",
        "--topic-keys",
        "governance,capture-fidelity",
        "--options",
        "Keep recorder and decider as the same actor,\
         Let capture name decided_by separately from the recording actor",
        "--chose",
        "Let capture name decided_by separately from the recording actor",
        "--decided-by",
        "human:alex.knips@gmail.com",
    ]))
    .expect("decision.capture with decided_by succeeds");

    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "digest",
        "--window",
        "7d",
        "--summary",
    ]))
    .expect("digest summary succeeds");

    assert!(
        summary.contains(&format!("[{decision_id}]")),
        "digest must cite the captured decision: {summary}"
    ); // ubs:ignore: test-only assertion
    assert!(
        summary.contains("(accepted)"),
        "a decision captured with decided_by must land as accepted, not stuck at proposed: {summary}"
    ); // ubs:ignore: test-only assertion
       // hivemind-zdsh.10: option ids are opaque now, so the two options attached in this single
       // proposal no longer have a stable relative order (previously an accident of the old id
       // scheme embedding the label). Assert content, not a specific order.
    assert!(
        summary.contains("Keep recorder and decider as the same actor")
            && summary.contains("Let capture name decided_by separately from the recording actor")
            && summary
                .contains("Chose: Let capture name decided_by separately from the recording actor"),
        "digest must render option titles, not raw option ids: {summary}"
    ); // ubs:ignore: test-only assertion
    assert!(
        summary.contains("By: agent:claude:hivemind-crew, human:alex.knips@gmail.com"),
        "digest must be honest about both who recorded and who decided, in one line: {summary}"
    ); // ubs:ignore: test-only assertion

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

/// The lines of one decision's bullet in a `digest --summary` rendering: from its
/// `• [<id>]` line up to the next bullet or blank line.
fn digest_block<'a>(summary: &'a str, decision_id: &str) -> Vec<&'a str> {
    let marker = format!("• [{decision_id}]");
    let mut block = Vec::new();
    let mut inside = false;
    for line in summary.lines() {
        if line.starts_with("• [") {
            inside = line.starts_with(&marker);
        } else if line.is_empty() {
            inside = false;
        }
        if inside {
            block.push(line);
        }
    }
    block
}

#[test]
fn digest_shows_the_three_attribution_cases_side_by_side() {
    // hivemind-zdsh.6, Alex's attribution ruling: (1) an agent asks and the human chooses ->
    // the human decided; (2) a human delegated a scope and the agent decides within it -> the
    // agent decided, the delegation visible on the record; (3) an agent decides alone -> the
    // agent decided, nothing on the record says a human sanctioned it. All three land as
    // accepted, and the digest has to let a reader tell them apart at a glance.
    let hivemind_dir = unique_test_dir("delegation-three-cases");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let capture = |title: &str, session: &str, extra: &[&str]| -> String {
        let mut args = vec![
            "hivemind",
            "--hivemind-dir",
            dir,
            "emit",
            "decision.capture",
            "--agent-tool",
            "claude",
            "--agent-session",
            session,
            "--title",
            title,
            "--rationale",
            "A stated, self-contained reason that reads without the source conversation",
            "--topic-keys",
            "governance",
            "--options",
            "Ship it,Hold it",
            "--chose",
            "Ship it",
            "--bet",
        ];
        args.extend_from_slice(extra);
        run(&Cli::parse_from(args)).expect("capture succeeds")
    };

    let human_decided = capture(
        "Case 1: agent asked, the human chose",
        "scribe",
        &["--decided-by", "human:alex"],
    );
    let delegated = capture(
        "Case 2: agent decided within a delegated scope",
        "builder",
        &["--delegated-by", "human:alex"],
    );
    let decided_alone = capture("Case 3: agent decided alone", "builder", &[]);

    let summary = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "digest",
        "--window",
        "7d",
        "--summary",
    ]))
    .expect("digest summary succeeds");

    let case1 = digest_block(&summary, &human_decided);
    let case2 = digest_block(&summary, &delegated);
    let case3 = digest_block(&summary, &decided_alone);
    for (name, block) in [("1", &case1), ("2", &case2), ("3", &case3)] {
        assert!(
            block
                .first()
                .is_some_and(|line| line.ends_with("(accepted)")),
            "case {name} must land as accepted: {summary}"
        ); // ubs:ignore: test-only assertion
    }
    // Case 1: the human is one of the deciders, named on the `By:` line; no delegation.
    assert!(
        case1.contains(&"  By: agent:claude:scribe, human:alex"),
        "case 1 names the human who decided: {summary}"
    ); // ubs:ignore: test-only assertion
    assert!(
        !case1.iter().any(|line| line.contains("Delegated by")),
        "case 1 carries no delegation: {summary}"
    ); // ubs:ignore: test-only assertion
       // Case 2: only the agent decided (no human on `By:`), and the delegating human is
       // stated on its own line.
    assert!(
        case2.contains(&"  By: agent:claude:builder"),
        "case 2 is the agent's own decision: {summary}"
    ); // ubs:ignore: test-only assertion
    assert!(
        case2.contains(&"  Delegated by: human:alex"),
        "case 2 shows the delegation: {summary}"
    ); // ubs:ignore: test-only assertion
       // Case 3: the same agent decided the same way, with nothing on the record saying a
       // human sanctioned it.
    assert!(
        case3.contains(&"  By: agent:claude:builder"),
        "case 3 is the agent's own decision: {summary}"
    ); // ubs:ignore: test-only assertion
    assert!(
        !case3.iter().any(|line| line.contains("Delegated by")),
        "case 3 carries no delegation: {summary}"
    ); // ubs:ignore: test-only assertion

    // `verify` (the decision brief) shows the same fact, as text and as JSON.
    let verify_text = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "query",
        "--summary",
        "verify",
        "--id",
        &delegated,
    ]))
    .expect("verify summary succeeds");
    assert!(
        verify_text.contains("decided by: agent:claude:builder")
            && verify_text.contains("delegated by: human:alex"),
        "verify text shows who decided and who delegated: {verify_text}"
    ); // ubs:ignore: test-only assertion
    let verify_json = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "query",
        "verify",
        "--id",
        &delegated,
    ]))
    .expect("verify json succeeds");
    let verify_json: serde_json::Value = serde_json::from_str(&verify_json).expect("valid json");
    assert_eq!(
        verify_json["data"]["decided_by"]["delegated_by"],
        serde_json::json!("human:alex")
    );
    let alone_text = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "query",
        "--summary",
        "verify",
        "--id",
        &decided_alone,
    ]))
    .expect("verify summary succeeds");
    assert!(
        !alone_text.contains("delegated by"),
        "an agent that decided alone has no delegation line: {alone_text}"
    ); // ubs:ignore: test-only assertion

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_refuses_a_delegation_that_is_not_an_agent_deciding_for_itself() {
    // A human recording under their own name is not "an agent deciding within a delegation",
    // and nothing may be written for a refused capture.
    let hivemind_dir = unique_test_dir("delegation-refused");
    let dir = hivemind_dir.to_str().expect("utf-8 temp path");
    let refused = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "human:sam",
        "--hivemind-dir",
        dir,
        "emit",
        "decision.proposed",
        "--title",
        "A human cannot borrow a delegation",
        "--rationale",
        "Delegation only qualifies an agent deciding for itself",
        "--topic-keys",
        "governance",
        "--options",
        "yes,no",
        "--chose",
        "yes",
        "--delegated-by",
        "human:alex",
    ]));
    assert!(refused.is_err(), "a human recorder must be refused");

    let digest = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        dir,
        "digest",
        "--window",
        "7d",
        "--summary",
    ]))
    .expect("digest summary succeeds");
    assert!(
        digest.contains("No decisions found"),
        "the refused capture must leave nothing behind: {digest}"
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
        "Need durable ingestion under heavy load",
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
        // hivemind-zdsh.8: --chose self-accepts by default, so this decision is
        // `accepted`, not `proposed`.
        "accepted",
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
        serde_json::json!("accepted")
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
    assert!(recent_summary.contains("accepted"));

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
    assert_eq!(output["summary"]["events_written"].as_u64(), Some(14));

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
        serde_json::json!("proposed")
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
fn import_documents_cli_lands_as_unreviewed_and_review_flow_works() {
    let hivemind_dir = unique_test_dir("import-documents-unreviewed");
    let scratch_dir = unique_test_dir("import-documents-unreviewed-doc");
    std::fs::create_dir_all(&scratch_dir).expect("scratch dir");
    let doc_path = scratch_dir.join("decision.md");
    std::fs::write(
        &doc_path,
        "Decision:\n  id: unreviewed-test\n  title: Use reviewed import flow\n  status: accepted\n  actor: actor:alice\n  topic_keys: import, review\n  rationale: Importers must not auto-accept; humans review first.\n  options:\n    - require review\n    - auto-accept\n  chose: require review\n",
    )
    .expect("write test document");

    let import_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:test",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "import",
        "documents",
        "--file",
        doc_path.to_str().expect("utf-8 doc path"),
    ]))
    .expect("document import succeeds");
    let import_output: serde_json::Value =
        serde_json::from_str(&import_output).expect("valid import json");
    assert_eq!(
        import_output["summary"]["blocks_imported"],
        serde_json::json!(1)
    );

    // Decision must land as proposed (unreviewed) even though the document says status: accepted.
    let unreviewed_cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:human",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "review",
        "--actor",
        "actor:alice",
        "--since",
        "2000-01-01",
        "--unreviewed-only",
    ]);
    let Command::Review(unreviewed_args) = &unreviewed_cli.command else {
        panic!("expected review command");
    };
    let mut empty_input = std::io::Cursor::new(Vec::<u8>::new());
    let mut prompts = Vec::new();
    let before_review = run_review_session(
        &unreviewed_cli,
        unreviewed_args,
        &mut empty_input,
        &mut prompts,
    )
    .expect("review session succeeds");
    let before_review: serde_json::Value =
        serde_json::from_str(&before_review).expect("valid review json");
    assert_eq!(
        before_review["matched_count"],
        serde_json::json!(1),
        "imported decision must appear as unreviewed"
    );

    // Approve the decision in a review session.
    let review_cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:human",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "review",
        "--actor",
        "actor:alice",
        "--since",
        "2000-01-01",
        "--unreviewed-only",
    ]);
    let Command::Review(review_args) = &review_cli.command else {
        panic!("expected review command");
    };
    let mut approve_input = std::io::Cursor::new("a\n".as_bytes());
    let mut prompts2 = Vec::new();
    let after_approve =
        run_review_session(&review_cli, review_args, &mut approve_input, &mut prompts2)
            .expect("review approval succeeds");
    let after_approve: serde_json::Value =
        serde_json::from_str(&after_approve).expect("valid review json");
    assert_eq!(
        after_approve["reviewed_count"],
        serde_json::json!(1),
        "decision must be accepted by reviewer"
    );

    // After review, must not appear as unreviewed.
    let post_cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:human",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "review",
        "--actor",
        "actor:alice",
        "--since",
        "2000-01-01",
        "--unreviewed-only",
    ]);
    let Command::Review(post_args) = &post_cli.command else {
        panic!("expected review command");
    };
    let mut empty_input2 = std::io::Cursor::new(Vec::<u8>::new());
    let mut prompts3 = Vec::new();
    let post_review = run_review_session(&post_cli, post_args, &mut empty_input2, &mut prompts3)
        .expect("post-review session succeeds");
    let post_review: serde_json::Value =
        serde_json::from_str(&post_review).expect("valid review json");
    assert_eq!(
        post_review["matched_count"],
        serde_json::json!(0),
        "reviewed decision must not appear as unreviewed after accept"
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
    let _ = std::fs::remove_dir_all(&scratch_dir);
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
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: proposed\n  topic_keys: conflict\n  rationale: First rationale for this decision.\n  options:\n    - first option\n",
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
        "Decision:\n  id: conflict-demo\n  title: Changed title\n  status: proposed\n  topic_keys: conflict\n  rationale: Changed rationale after further review.\n  options:\n    - first option\n",
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
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: accepted\n  actor: actor:alice\n  topic_keys: conflict\n  rationale: First rationale for this decision.\n  options:\n    - first option\n  chose: first option\n",
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
        "Decision:\n  id: conflict-demo\n  title: Superseding title\n  status: accepted\n  actor: actor:bob\n  topic_keys: conflict\n  rationale: Replacement rationale for the superseding decision.\n  options:\n    - second option\n  chose: second option\n",
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
    assert_eq!(new_view["data"]["status"], serde_json::json!("proposed"));
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
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: accepted\n  actor: actor:alice\n  topic_keys: conflict\n  rationale: First rationale for this decision.\n  options:\n    - first option\n  chose: first option\n",
    )
    .expect("write initial doc");

    let first_import = run(&Cli::parse_from([
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
    .expect("initial import succeeds");
    let first_import: serde_json::Value =
        serde_json::from_str(&first_import).expect("valid import json");
    let decision_id = first_import["files"][0]["blocks"][0]["decision_id"]
        .as_str()
        .expect("decision id from first import");

    // Explicitly accept the decision so that a contest can produce a "contested" status.
    run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:local",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.accepted",
        "--decision-id",
        decision_id,
    ]))
    .expect("explicit acceptance succeeds");

    std::fs::write(
        &document_path,
        "Decision:\n  id: conflict-demo\n  title: Contesting title\n  status: proposed\n  topic_keys: conflict\n  rationale: This changed import disagrees with the accepted decision.\n  options:\n    - second option\n",
    )
    .expect("write changed doc");

    // Use a different actor to reject (contest) — same actor cannot both accept and reject.
    let output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "dissenter:local",
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
        "Decision:\n  id: conflict-demo\n  title: Keep first title\n  status: proposed\n  topic_keys: conflict\n  rationale: First rationale for this decision.\n  options:\n    - first option\n",
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
        "Decision:\n  id: conflict-demo\n  title: Changed title\n  status: proposed\n  topic_keys: conflict\n  rationale: Changed rationale after further review.\n  options:\n    - first option\n  evidence:\n    - New evidence from re-import.\n  hypotheses:\n    - New assumption from re-import.\n",
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
        "--bet",
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
        "--bet",
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
        "--bet",
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

/// Backend selector for a CLI test invocation. SQLite (the default) only needs
/// `--hivemind-dir`, pointing at a fresh isolated temp directory per test.
/// Postgres additionally needs `--database-url` and a per-test `--tenant` —
/// unlike a temp SQLite dir, the configured Postgres database is shared, so
/// tests must not collide with each other or with prior runs.
struct TestBackend {
    hivemind_dir: PathBuf,
    tenant: Option<String>,
    database_url: Option<String>,
}

impl TestBackend {
    fn sqlite(name: &str) -> Self {
        Self {
            hivemind_dir: unique_test_dir(name),
            tenant: None,
            database_url: None,
        }
    }

    /// `None` when `HIVEMIND_TEST_POSTGRES_URL` is unset — callers skip the test.
    ///
    /// Provisions the per-test tenant before returning: since hivemind-rkbf.1,
    /// an unregistered tenant hard-errors at ledger-open time instead of
    /// silently opening a fresh scope, so every CLI-against-Postgres test
    /// needs a real `hm_tenants` row first.
    fn postgres(name: &str) -> Option<Self> {
        let database_url = std::env::var("HIVEMIND_TEST_POSTGRES_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let tenant = format!("tenant:test:{name}:{nanos}");

        #[cfg(feature = "shared-backend-postgres")]
        {
            let store = crate::ledger::TenantStore::connect(&database_url)
                .expect("TenantStore::connect for CLI test tenant provisioning");
            store
                .provision_tenant(&tenant, "CLI test tenant")
                .expect("provision_tenant for CLI test");
        }

        Some(Self {
            hivemind_dir: unique_test_dir(name),
            tenant: Some(tenant),
            database_url: Some(database_url),
        })
    }

    /// Global flags to splice into a `Cli::parse_from` argument list, right
    /// after the binary name.
    fn global_args(&self) -> Vec<String> {
        let mut args = vec![
            "--hivemind-dir".to_owned(),
            self.hivemind_dir
                .to_str()
                .expect("utf-8 temp path")
                .to_owned(),
        ];
        if let Some(url) = &self.database_url {
            args.push("--database-url".to_owned());
            args.push(url.clone());
        }
        if let Some(tenant) = &self.tenant {
            args.push("--tenant".to_owned());
            args.push(tenant.clone());
        }
        args
    }
}

impl Drop for TestBackend {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.hivemind_dir);
    }
}

/// Builds a full `Cli::parse_from` argument vector: the binary name, this
/// backend's global flags, then `rest` (starting with per-command flags like
/// `--actor` and the subcommand itself).
fn cli_args(backend: &TestBackend, rest: &[&str]) -> Vec<String> {
    let mut args = vec!["hivemind".to_owned()];
    args.extend(backend.global_args());
    args.extend(rest.iter().map(|value| (*value).to_owned()));
    args
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

#[test]
fn classify_queue_list_and_submit_round_trip() {
    use crate::commands::{CommandContext, Commands};
    use crate::events::{EventProvenance, IngestTurn, TenantId};
    use crate::ledger::SqliteEventLedger;

    let hivemind_dir = unique_test_dir("classify-queue");
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(TenantId::local(), EventProvenance::cli()),
    );

    // Seed a batch that needs classification.
    commands
        .record_ingest_batch(
            "agent:test",
            "batch-abc",
            "claude-code",
            "session-1",
            vec![IngestTurn {
                turn_id: "t1".to_owned(),
                role: "user".to_owned(),
                text: "Should we use Postgres or SQLite?".to_owned(),
                truncated: false,
            }],
        )
        .expect("record batch");

    let dir_str = hivemind_dir.to_str().expect("utf-8");

    // list sees the pending batch
    let list_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir_str,
        "classify-queue",
        "list",
    ]))
    .expect("classify-queue list succeeds");
    let list: serde_json::Value = serde_json::from_str(&list_output).expect("valid json");
    assert_eq!(list.as_array().map(|a| a.len()), Some(1));
    assert_eq!(list[0]["batch_id"], serde_json::json!("batch-abc"));
    assert_eq!(list[0]["turn_count"], serde_json::json!(1));

    // submit classification
    let captures_json = serde_json::json!([{
        "kind": "decision",
        "title": "Use Postgres for production",
        "rationale": "Scale and concurrent-write requirements favour Postgres",
        "topic_keys": ["database"],
        "evidence_ids": [],
        "options": null,
        "chosen_option": null,
        "extraction_confidence": 0.9
    }])
    .to_string();

    let submit_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--actor",
        "agent:claude-code:session-1",
        "--hivemind-dir",
        dir_str,
        "classify-queue",
        "submit",
        "--batch-id",
        "batch-abc",
        "--captures",
        &captures_json,
    ]))
    .expect("classify-queue submit succeeds");
    let submit: serde_json::Value = serde_json::from_str(&submit_output).expect("valid json");
    assert_eq!(submit["batch_ids"], serde_json::json!(["batch-abc"]));
    assert_eq!(submit["capture_count"], serde_json::json!(1));

    // list now returns empty (batch is classified)
    let list2_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        dir_str,
        "classify-queue",
        "list",
    ]))
    .expect("classify-queue list after submit succeeds");
    let list2: serde_json::Value = serde_json::from_str(&list2_output).expect("valid json");
    assert_eq!(list2.as_array().map(|a| a.len()), Some(0));
}

fn project_registry_register_link_anchor_list_show_round_trip_body(
    backend: &TestBackend,
) -> CliTestResult {
    // Mirrors the product spec's own worked example (§5): Billing is part of
    // Platform, and Billing depends on Auth.
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "register",
            "platform",
            "--display-name",
            "Platform",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "register",
            "billing",
            "--display-name",
            "Billing",
            "--purpose",
            "Per-seat and per-org pricing decisions",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &["--actor", "human:alice", "project", "register", "auth"],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "link",
            "--from",
            "billing",
            "--to",
            "platform",
            "--kind",
            "part_of",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "link",
            "--from",
            "billing",
            "--to",
            "auth",
            "--kind",
            "depends_on",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "anchor",
            "--handle",
            "billing",
            "--kind",
            "rig",
            "--value",
            "billing",
        ],
    )))?;

    let list_output = run(&Cli::parse_from(cli_args(
        backend,
        &["--json", "project", "list"],
    )))?;
    let mut list_json: serde_json::Value = serde_json::from_str(&list_output)?;
    list_json["latency_ms"] = serde_json::json!(0);
    ensure_json_eq(
        &list_json,
        serde_json::json!({
            "result_count": 3,
            "truncated": false,
            "latency_ms": 0,
            "data": {
                "limit": 25,
                "cursor": null,
                "next_cursor": null,
                "total_matches": 3,
                "items": [
                    {
                        "handle": "auth",
                        "personal": false,
                        "anchors": [],
                        "depends_on": [],
                        "registered_event_origin": 3
                    },
                    {
                        "handle": "billing",
                        "display_name": "Billing",
                        "purpose": "Per-seat and per-org pricing decisions",
                        "personal": false,
                        "anchors": [{"kind": "rig", "value": "billing"}],
                        "part_of": {"to": "platform", "event_origin": 4},
                        "depends_on": [{"to": "auth", "event_origin": 5}],
                        "registered_event_origin": 2
                    },
                    {
                        "handle": "platform",
                        "display_name": "Platform",
                        "personal": false,
                        "anchors": [],
                        "depends_on": [],
                        "registered_event_origin": 1
                    }
                ]
            }
        }),
        "project list golden",
    )?;

    let show_output = run(&Cli::parse_from(cli_args(
        backend,
        &["--json", "project", "show", "billing"],
    )))?;
    let mut show_json: serde_json::Value = serde_json::from_str(&show_output)?;
    show_json["latency_ms"] = serde_json::json!(0);
    ensure_json_eq(
        &show_json,
        serde_json::json!({
            "result_count": 1,
            "truncated": false,
            "latency_ms": 0,
            "data": {
                "outcome": "found",
                "project": {
                    "handle": "billing",
                    "display_name": "Billing",
                    "purpose": "Per-seat and per-org pricing decisions",
                    "personal": false,
                    "anchors": [{"kind": "rig", "value": "billing"}],
                    "part_of": {"to": "platform", "event_origin": 4},
                    "depends_on": [{"to": "auth", "event_origin": 5}],
                    "registered_event_origin": 2
                }
            }
        }),
        "project show golden (registered handle)",
    )?;

    // An unknown handle is a successful envelope with outcome not_found, never an
    // error (Alex's rule, 2026-09-20: a miss is data).
    let missing_output = run(&Cli::parse_from(cli_args(
        backend,
        &["--json", "project", "show", "nonexistent"],
    )))?;
    let mut missing_json: serde_json::Value = serde_json::from_str(&missing_output)?;
    missing_json["latency_ms"] = serde_json::json!(0);
    ensure_json_eq(
        &missing_json,
        serde_json::json!({
            "result_count": 0,
            "truncated": false,
            "latency_ms": 0,
            "data": {"outcome": "not_found"}
        }),
        "project show golden (unknown handle)",
    )?;

    // A personal address always resolves — it's derived from the actor, never
    // registered, and "cannot be a typo" (product spec §1).
    let personal_output = run(&Cli::parse_from(cli_args(
        backend,
        &["--json", "project", "show", "personal:human:alice"],
    )))?;
    let mut personal_json: serde_json::Value = serde_json::from_str(&personal_output)?;
    personal_json["latency_ms"] = serde_json::json!(0);
    ensure_json_eq(
        &personal_json,
        serde_json::json!({
            "result_count": 1,
            "truncated": false,
            "latency_ms": 0,
            "data": {
                "outcome": "found",
                "project": {
                    "handle": "personal:human:alice",
                    "personal": true,
                    "anchors": [],
                    "depends_on": []
                }
            }
        }),
        "project show golden (personal address)",
    )?;

    Ok(())
}

#[test]
fn project_registry_register_link_anchor_list_show_round_trip() -> CliTestResult {
    project_registry_register_link_anchor_list_show_round_trip_body(&TestBackend::sqlite(
        "project-registry",
    ))
}

#[test]
fn project_registry_register_link_anchor_list_show_round_trip_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("project-registry-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    project_registry_register_link_anchor_list_show_round_trip_body(&backend)
}

/// Captures one decision through the CLI and returns its id. `project: None` lands it in the
/// actor's personal project.
fn capture_for_project_list(
    backend: &TestBackend,
    actor_id: &str,
    title: &str,
    project: Option<&str>,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let mut rest = vec![
        "--json",
        "emit",
        "decision.capture",
        "--actor-id",
        actor_id,
        "--title",
        title,
        "--rationale",
        PROJECT_TEST_RATIONALE,
        "--topic-keys",
        "billing",
        "--options",
        "queue,sync",
        "--chose",
        "queue",
        "--bet",
    ];
    if let Some(project) = project {
        rest.extend(["--project", project]);
    }
    let reply: serde_json::Value =
        serde_json::from_str(&run(&Cli::parse_from(cli_args(backend, &rest)))?)?;
    Ok(reply["value"]
        .as_str()
        .ok_or("the capture reply names the decision id in `value`")?
        .to_owned())
}

/// `project decisions ... --json`, with the fields that vary between runs (latency, the
/// ledger offset, the wall-clock time) checked for presence and then removed.
fn project_decisions_json(
    backend: &TestBackend,
    rest: &[&str],
) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut args = vec!["--json", "project", "decisions"];
    args.extend_from_slice(rest);
    let mut json: serde_json::Value =
        serde_json::from_str(&run(&Cli::parse_from(cli_args(backend, &args)))?)?;
    json["latency_ms"] = serde_json::json!(0);
    // `.get_mut()`, not `json["data"]["items"]`: indexing a `Value` with a missing key
    // inserts a `null` at that key as a side effect (serde_json's `IndexMut` impl), which
    // would plant a spurious `"items": null` into the `not_found` case that has no `items`.
    if let Some(items) = json
        .get_mut("data")
        .and_then(|data| data.get_mut("items"))
        .and_then(|items| items.as_array_mut())
    {
        for item in items {
            ensure(
                item["event_origin"].is_i64() && item["occurred_at"].is_string(),
                "every listed decision says when it was recorded",
            )?;
            let item = item.as_object_mut().ok_or("items are objects")?;
            item.remove("event_origin");
            item.remove("occurred_at");
        }
    }
    Ok(json)
}

fn project_decisions_text(
    backend: &TestBackend,
    rest: &[&str],
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let mut args = vec!["project", "decisions"];
    args.extend_from_slice(rest);
    Ok(run(&Cli::parse_from(cli_args(backend, &args)))?)
}

fn project_decisions_lists_shared_and_personal_projects_body(
    backend: &TestBackend,
) -> CliTestResult {
    register_test_project(backend, "billing")?;

    // Two sessions of one agent tool, one session of another, and one filed under billing.
    let stated = capture_for_project_list(
        backend,
        "agent:claude:session-1",
        "Adopt async billing queue",
        Some("billing"),
    )?;
    let first = capture_for_project_list(
        backend,
        "agent:claude:session-1",
        "Retry billing jobs with backoff",
        None,
    )?;
    let second = capture_for_project_list(
        backend,
        "agent:claude:session-2",
        "Export invoices nightly",
        None,
    )?;
    let other_tool = capture_for_project_list(
        backend,
        "agent:codex:session-1",
        "Cache the billing token",
        None,
    )?;

    // A personal address: every session of the tool in one list, each with its session.
    let personal = project_decisions_json(backend, &["personal:agent:claude"])?;
    ensure_json_eq(
        &personal,
        serde_json::json!({
            "result_count": 2,
            "truncated": false,
            "latency_ms": 0,
            "data": {
                "outcome": "found",
                "handle": "personal:agent:claude",
                "personal": true,
                "owner": "agent:claude",
                "limit": 25,
                "cursor": null,
                "next_cursor": null,
                "total_matches": 2,
                "items": [
                    {
                        "decision_id": first,
                        "title": "Retry billing jobs with backoff",
                        "status": "accepted",
                        "proposed_by": "agent:claude:session-1",
                        "session": "session-1",
                        "project_source": "personal_fallback"
                    },
                    {
                        "decision_id": second,
                        "title": "Export invoices nightly",
                        "status": "accepted",
                        "proposed_by": "agent:claude:session-2",
                        "session": "session-2",
                        "project_source": "personal_fallback"
                    }
                ]
            }
        }),
        "project decisions golden (personal address)",
    )?;

    // The text header says whose project it is and how many are still waiting.
    ensure_eq(
        project_decisions_text(backend, &["personal:agent:claude"])?,
        format!(
            "in agent:claude's personal project, not yet shared: 2\n\
             accepted\t{first}\tRetry billing jobs with backoff\tactor=agent:claude:session-1\tsession=session-1\tproject_source=personal_fallback\n\
             accepted\t{second}\tExport invoices nightly\tactor=agent:claude:session-2\tsession=session-2\tproject_source=personal_fallback"
        ),
        "personal project text",
    )?;

    // A limit that cuts the list says so and hands back a cursor; the count stays whole.
    let page = project_decisions_json(backend, &["personal:agent:claude", "--limit", "1"])?;
    ensure_json_eq(
        &page,
        serde_json::json!({
            "result_count": 1,
            "truncated": true,
            "latency_ms": 0,
            "data": {
                "outcome": "found",
                "handle": "personal:agent:claude",
                "personal": true,
                "owner": "agent:claude",
                "limit": 1,
                "cursor": null,
                "next_cursor": "1",
                "total_matches": 2,
                "items": [
                    {
                        "decision_id": first,
                        "title": "Retry billing jobs with backoff",
                        "status": "accepted",
                        "proposed_by": "agent:claude:session-1",
                        "session": "session-1",
                        "project_source": "personal_fallback"
                    }
                ]
            }
        }),
        "project decisions golden (truncated, cursor)",
    )?;
    ensure_eq(
        project_decisions_text(backend, &["personal:agent:claude", "--limit", "1"])?,
        format!(
            "in agent:claude's personal project, not yet shared: 2\n\
             accepted\t{first}\tRetry billing jobs with backoff\tactor=agent:claude:session-1\tsession=session-1\tproject_source=personal_fallback\n\
             truncated=true next_cursor=1"
        ),
        "truncated personal project text",
    )?;
    let rest = project_decisions_json(
        backend,
        &["personal:agent:claude", "--limit", "1", "--cursor", "1"],
    )?;
    ensure_eq(
        rest["truncated"].as_bool(),
        Some(false),
        "the last page is not truncated",
    )?;
    ensure_eq(
        rest["data"]["items"][0]["decision_id"].as_str(),
        Some(second.as_str()),
        "the cursor continues after the first decision",
    )?;

    // A shared handle lists what was filed under it, with how that was determined.
    let shared = project_decisions_json(backend, &["billing"])?;
    ensure_json_eq(
        &shared,
        serde_json::json!({
            "result_count": 1,
            "truncated": false,
            "latency_ms": 0,
            "data": {
                "outcome": "found",
                "handle": "billing",
                "personal": false,
                "limit": 25,
                "cursor": null,
                "next_cursor": null,
                "total_matches": 1,
                "items": [
                    {
                        "decision_id": stated,
                        "title": "Adopt async billing queue",
                        "status": "accepted",
                        "proposed_by": "agent:claude:session-1",
                        "session": "session-1",
                        "project_source": "stated"
                    }
                ]
            }
        }),
        "project decisions golden (shared handle)",
    )?;
    ensure_eq(
        project_decisions_text(backend, &["billing"])?,
        format!(
            "in project billing: 1\n\
             accepted\t{stated}\tAdopt async billing queue\tactor=agent:claude:session-1\tsession=session-1\tproject_source=stated"
        ),
        "shared project text",
    )?;

    // Another tool's personal project is a different list, and a personal address with
    // nothing in it yet is still an answer.
    let codex = project_decisions_json(backend, &["personal:agent:codex"])?;
    ensure_eq(
        codex["data"]["items"][0]["decision_id"].as_str(),
        Some(other_tool.as_str()),
        "another tool's decision lists under its own personal address",
    )?;
    ensure_eq(
        project_decisions_text(backend, &["personal:human:alice"])?,
        "in human:alice's personal project, not yet shared: 0".to_owned(),
        "empty personal project text",
    )?;

    // A wrong handle is never an empty list.
    let missing = project_decisions_json(backend, &["billng"])?;
    ensure_json_eq(
        &missing,
        serde_json::json!({
            "result_count": 0,
            "truncated": false,
            "latency_ms": 0,
            "data": {"outcome": "not_found", "handle": "billng"}
        }),
        "project decisions golden (unknown handle)",
    )?;
    ensure_eq(
        project_decisions_text(backend, &["billng"])?,
        "no project called 'billng': run `hivemind project register billng` to register it"
            .to_owned(),
        "unknown handle text",
    )
}

#[test]
fn project_decisions_lists_shared_and_personal_projects() -> CliTestResult {
    project_decisions_lists_shared_and_personal_projects_body(&TestBackend::sqlite(
        "project-decisions",
    ))
}

#[test]
fn project_decisions_lists_shared_and_personal_projects_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("project-decisions-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    project_decisions_lists_shared_and_personal_projects_body(&backend)
}

fn project_use_and_show_current_round_trip_body(backend: &TestBackend) -> CliTestResult {
    let tenant_label = backend.tenant.clone().unwrap_or_else(|| "local".to_owned());

    run(&Cli::parse_from(cli_args(
        backend,
        &["--actor", "human:alice", "project", "register", "billing"],
    )))?;

    // Nothing set yet: show --current reports no handle, not an error --
    // omission is data (same honesty rule as `show` on an unknown handle).
    let unset_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "human:alice",
            "project",
            "show",
            "--current",
        ],
    )))?;
    ensure_json_eq(
        &serde_json::from_str(&unset_output)?,
        serde_json::json!({"actor": "human:alice", "tenant": tenant_label}),
        "project show --current golden (unset)",
    )?;

    // Setting an unregistered handle is refused with a register hint.
    let refusal = run(&Cli::parse_from(cli_args(
        backend,
        &["--actor", "human:alice", "project", "use", "nonexistent"],
    )))
    .expect_err("unregistered handle is refused");
    assert!(refusal
        .to_string()
        .contains("hivemind project register nonexistent"));

    let use_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "human:alice",
            "project",
            "use",
            "billing",
        ],
    )))?;
    ensure_json_eq(
        &serde_json::from_str(&use_output)?,
        serde_json::json!({"actor": "human:alice", "tenant": tenant_label, "handle": "billing"}),
        "project use golden",
    )?;

    let show_current_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "human:alice",
            "project",
            "show",
            "--current",
        ],
    )))?;
    ensure_json_eq(
        &serde_json::from_str(&show_current_output)?,
        serde_json::json!({"actor": "human:alice", "tenant": tenant_label, "handle": "billing"}),
        "project show --current golden (set)",
    )?;

    // A different actor's setting is independent.
    let other_actor_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "human:bob",
            "project",
            "show",
            "--current",
        ],
    )))?;
    ensure_json_eq(
        &serde_json::from_str(&other_actor_output)?,
        serde_json::json!({"actor": "human:bob", "tenant": tenant_label}),
        "project show --current golden (other actor, unset)",
    )?;

    let clear_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "human:alice",
            "project",
            "use",
            "--clear",
        ],
    )))?;
    ensure_json_eq(
        &serde_json::from_str(&clear_output)?,
        serde_json::json!({"actor": "human:alice", "tenant": tenant_label}),
        "project use --clear golden",
    )?;

    let after_clear_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "human:alice",
            "project",
            "show",
            "--current",
        ],
    )))?;
    ensure_json_eq(
        &serde_json::from_str(&after_clear_output)?,
        serde_json::json!({"actor": "human:alice", "tenant": tenant_label}),
        "project show --current golden (after clear)",
    )?;

    // Mutually exclusive flags are refused, not silently resolved one way.
    let both = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "use",
            "billing",
            "--clear",
        ],
    )));
    assert!(both.is_err());
    let neither = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "show",
            "billing",
            "--current",
        ],
    )));
    assert!(neither.is_err());

    Ok(())
}

#[test]
fn project_use_and_show_current_round_trip() -> CliTestResult {
    project_use_and_show_current_round_trip_body(&TestBackend::sqlite("project-use"))
}

#[test]
fn project_use_and_show_current_round_trip_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("project-use-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    project_use_and_show_current_round_trip_body(&backend)
}

#[test]
fn emit_ingest_batch_classified_plugin_path_round_trip() {
    use crate::events::EventType;
    use crate::ledger::SqliteEventLedger;

    // This test covers the keyless plugin-edge capture path:
    // the plugin spawns a Haiku subagent in-session, gets CaptureItem JSON back,
    // and submits it via `emit ingest.batch_classified` — no server API key needed.
    // The server background classifier is not involved (no IngestBatchReceived event
    // exists for this batch_id, so classify-queue list stays empty).

    let hivemind_dir = unique_test_dir("emit-ingest-batch-classified-plugin");
    let captures_file = unique_test_dir("emit-ingest-batch-classified-captures");
    let captures_path = captures_file.with_extension("json");

    // Write a minimal captures JSON file (as the Haiku subagent would return)
    let captures_json = serde_json::json!([
        {
            "kind": "decision",
            "title": "Use SQLite for local prototype ledger",
            "rationale": "Lighter than Postgres for single-user deployments, sufficient for prototype scale",
            "topic_keys": ["storage", "architecture"],
            "evidence_ids": [],
            "options": ["postgres", "sqlite", "dolt"],
            "chosen_option": "sqlite",
            "extraction_confidence": 0.92,
            "expressed_confidence": null,
            "supersedes_id": null,
            "assumes_ids": [],
            "supports_ids": [],
            "refutes_ids": [],
            "actor_id": null,
            "accepted_by": null,
            "rejected_by": null,
            "blocked_actor_id": null,
            "decision_id": null
        }
    ]);
    std::fs::write(&captures_path, captures_json.to_string()).expect("write captures file");

    // Submit via the new edge-capture command
    let output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "ingest.batch_classified",
        "--captures",
        captures_path.to_str().expect("utf-8 captures path"),
        "--agent-tool",
        "claude",
        "--agent-session",
        "plugin-session-1",
        "--classifier-model",
        "claude-haiku-4-5-20251001",
    ]))
    .expect("edge batch submit succeeds");

    let output: serde_json::Value = serde_json::from_str(&output).expect("valid json output");
    assert_eq!(
        output.get("subcommand").and_then(|v| v.as_str()),
        Some("emit")
    );
    assert_eq!(
        output.get("kind").and_then(|v| v.as_str()),
        Some("batch_id")
    );
    let batch_id = output
        .get("value")
        .and_then(|v| v.as_str())
        .expect("batch_id in output");
    assert!(!batch_id.is_empty());

    // Verify the IngestBatchClassified event landed in the ledger
    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 50).expect("events read");

    let classified_event = events
        .iter()
        .find(|e| e.event_type == EventType::IngestBatchClassified)
        .expect("IngestBatchClassified event written");
    // ubs:ignore: batch IDs are public ledger identifiers
    assert_eq!(
        classified_event
            .payload
            .get("batch_id")
            .and_then(|v| v.as_str()),
        Some(batch_id)
    );
    assert_eq!(classified_event.actor_id, "agent:claude:plugin-session-1");
    assert_eq!(
        classified_event
            .payload
            .get("classifier_model")
            .and_then(|v| v.as_str()),
        Some("claude-haiku-4-5-20251001")
    );

    // No IngestBatchReceived event — server classifier has nothing to process
    assert!(
        events
            .iter()
            .all(|e| e.event_type != EventType::IngestBatchReceived),
        "plugin-edge path must not create a IngestBatchReceived event"
    );

    // The captures project to a queryable decision in the graph
    let search_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "query",
        "search_decisions",
        "--limit",
        "5",
    ]))
    .expect("search succeeds");
    let search: serde_json::Value =
        serde_json::from_str(&search_output).expect("valid search json");
    assert_eq!(
        search["result_count"],
        serde_json::json!(1),
        "one capture projected to graph"
    );
    assert_eq!(
        search["data"]["items"][0]["decision"]["title"],
        serde_json::json!("Use SQLite for local prototype ledger")
    );

    let _ = std::fs::remove_file(&captures_path);
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn emit_decision_scored_plugin_path_round_trip() {
    use crate::events::EventType;
    use crate::ledger::SqliteEventLedger;

    // This test covers the keyless plugin-edge scoring path (hivemind-wi3u):
    // the plugin submits a batch via `emit ingest.batch_classified`, then
    // spawns a Haiku subagent to score the decision capture and submits the
    // result via `emit decision.scored` — no server API key needed, and no
    // caller-constructed `capture:{event_id}:{idx}` node id.

    let hivemind_dir = unique_test_dir("emit-decision-scored-plugin");
    let captures_file = unique_test_dir("emit-decision-scored-captures");
    let captures_path = captures_file.with_extension("json");

    let captures_json = serde_json::json!([
        {
            "kind": "decision",
            "title": "Use Postgres for the shared event ledger",
            "rationale": "Concurrent multi-tenant writes are a day-one requirement; SQLite's single-writer model would bottleneck immediately",
            "topic_keys": ["storage", "architecture"],
            "evidence_ids": [],
            "options": ["sqlite", "postgres"],
            "chosen_option": "postgres",
            "extraction_confidence": 0.9,
            "expressed_confidence": null,
            "supersedes_id": null,
            "assumes_ids": [],
            "supports_ids": [],
            "refutes_ids": [],
            "actor_id": null,
            "accepted_by": null,
            "rejected_by": null,
            "blocked_actor_id": null,
            "decision_id": null
        }
    ]);
    std::fs::write(&captures_path, captures_json.to_string()).expect("write captures file");

    let batch_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "ingest.batch_classified",
        "--captures",
        captures_path.to_str().expect("utf-8 captures path"),
        "--agent-tool",
        "claude",
        "--agent-session",
        "plugin-session-score",
        "--classifier-model",
        "claude-haiku-4-5-20251001",
    ]))
    .expect("edge batch submit succeeds");
    let batch_output: serde_json::Value =
        serde_json::from_str(&batch_output).expect("valid json output");
    let batch_id = batch_output
        .get("value")
        .and_then(|v| v.as_str())
        .expect("batch_id in output")
        .to_owned();

    // Scores JSON as a Haiku subagent would return, with one score
    // deliberately out of range to exercise server-side clamping.
    let scores_file = unique_test_dir("emit-decision-scored-scores");
    let scores_path = scores_file.with_extension("json");
    let scores_json = serde_json::json!({
        "quality_dims": {
            "framing": {"score": 0.8, "explanation": "Framed against a stated concurrency requirement."},
            "alternatives": {"score": 0.7, "explanation": "sqlite and postgres both named and compared."},
            "information": {"score": 1.5, "explanation": "Out-of-range on purpose to exercise clamping."},
            "reasoning": {"score": 0.75, "explanation": "Chosen option follows from the stated requirement."},
            "values_tradeoffs": {"score": 0.5, "explanation": "Operational cost not explicitly weighed."},
            "bias_exposure": {"score": 0.8, "explanation": "No evidence of anchoring."},
            "calibration": {"score": 0.5, "explanation": "No expressed confidence to check against."}
        },
        "importance": {
            "stakes": 8.0,
            "stakes_explanation": "Affects the shared ledger used by every tenant.",
            "irreversibility": 0.6,
            "irreversibility_explanation": "Migrating engines later is possible but costly.",
            "actionability": 1.0,
            "actionability_explanation": "Directly determines what gets built next."
        }
    });
    std::fs::write(&scores_path, scores_json.to_string()).expect("write scores file");

    let score_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.scored",
        "--batch-id",
        &batch_id,
        "--capture-index",
        "0",
        "--scores",
        scores_path.to_str().expect("utf-8 scores path"),
        "--agent-tool",
        "claude",
        "--agent-session",
        "plugin-session-score",
        "--scorer-model",
        "claude-haiku-4-5-20251001",
    ]))
    .expect("edge decision.scored submit succeeds");
    let score_output: serde_json::Value =
        serde_json::from_str(&score_output).expect("valid json output");
    assert_eq!(
        score_output.get("kind").and_then(|v| v.as_str()),
        Some("event_id")
    );

    let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 50).expect("events read");

    let classified_event = events
        .iter()
        .find(|e| e.event_type == EventType::IngestBatchClassified)
        .expect("IngestBatchClassified event written");
    let classified_event_id = classified_event.event_id.expect("event id assigned");

    let scored_event = events
        .iter()
        .find(|e| e.event_type == EventType::DecisionScored)
        .expect("DecisionScored event written");
    assert_eq!(scored_event.actor_id, "agent:claude:plugin-session-score");
    assert_eq!(
        scored_event
            .payload
            .get("capture_node_id")
            .and_then(|v| v.as_str()),
        Some(format!("capture:{classified_event_id}:0").as_str()),
        "capture_node_id derived from batch-id + capture-index, not caller-supplied"
    );
    assert_eq!(
        scored_event.causation_event_id,
        Some(classified_event_id),
        "decision.scored is causally linked to the batch it scores"
    );
    // The out-of-range information score was clamped to 1.0, same as the
    // server-side scorer worker's own validation.
    assert_eq!(
        scored_event
            .payload
            .get("quality_dims")
            .and_then(|v| v.get("information"))
            .and_then(|v| v.get("score"))
            .and_then(|v| v.as_f64()),
        Some(1.0)
    );

    let _ = std::fs::remove_file(&captures_path);
    let _ = std::fs::remove_file(&scores_path);
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn emit_decision_scored_rejects_non_decision_capture() {
    let hivemind_dir = unique_test_dir("emit-decision-scored-non-decision");
    let captures_file = unique_test_dir("emit-decision-scored-non-decision-captures");
    let captures_path = captures_file.with_extension("json");

    let captures_json = serde_json::json!([
        {
            "kind": "evidence",
            "title": "Latency measurement",
            "rationale": "p95 improved after the change",
            "topic_keys": [],
            "evidence_ids": [],
            "options": null,
            "chosen_option": null,
            "extraction_confidence": 0.9,
            "expressed_confidence": null,
            "supersedes_id": null,
            "assumes_ids": [],
            "supports_ids": [],
            "refutes_ids": [],
            "actor_id": null,
            "accepted_by": null,
            "rejected_by": null,
            "blocked_actor_id": null,
            "decision_id": null
        }
    ]);
    std::fs::write(&captures_path, captures_json.to_string()).expect("write captures file");

    let batch_output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "ingest.batch_classified",
        "--captures",
        captures_path.to_str().expect("utf-8 captures path"),
        "--agent-tool",
        "claude",
        "--agent-session",
        "plugin-session-non-decision",
        "--classifier-model",
        "claude-haiku-4-5-20251001",
    ]))
    .expect("edge batch submit succeeds");
    let batch_output: serde_json::Value =
        serde_json::from_str(&batch_output).expect("valid json output");
    let batch_id = batch_output
        .get("value")
        .and_then(|v| v.as_str())
        .expect("batch_id in output")
        .to_owned();

    let scores_file = unique_test_dir("emit-decision-scored-non-decision-scores");
    let scores_path = scores_file.with_extension("json");
    let scores_json = serde_json::json!({
        "quality_dims": {
            "framing": {"score": 0.5, "explanation": "n/a"},
            "alternatives": {"score": 0.5, "explanation": "n/a"},
            "information": {"score": 0.5, "explanation": "n/a"},
            "reasoning": {"score": 0.5, "explanation": "n/a"},
            "values_tradeoffs": {"score": 0.5, "explanation": "n/a"},
            "bias_exposure": {"score": 0.5, "explanation": "n/a"},
            "calibration": {"score": 0.5, "explanation": "n/a"}
        },
        "importance": {
            "stakes": 1.0,
            "stakes_explanation": "n/a",
            "irreversibility": 0.5,
            "irreversibility_explanation": "n/a",
            "actionability": 0.5,
            "actionability_explanation": "n/a"
        }
    });
    std::fs::write(&scores_path, scores_json.to_string()).expect("write scores file");

    let err = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "emit",
        "decision.scored",
        "--batch-id",
        &batch_id,
        "--capture-index",
        "0",
        "--scores",
        scores_path.to_str().expect("utf-8 scores path"),
    ]))
    .expect_err("scoring a non-decision capture must fail");
    assert!(
        err.to_string().contains("not \"decision\""),
        "error should explain the capture kind mismatch: {err}"
    );

    let _ = std::fs::remove_file(&captures_path);
    let _ = std::fs::remove_file(&scores_path);
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn import_documents_cli_prose_extraction_writes_candidates_as_unreviewed() {
    // Prose file with no Decision: blocks → extractor path → UNREVIEWED in ledger.
    let root = unique_test_dir("import-prose-extraction");
    let hivemind_dir = root.join("hive");
    std::fs::create_dir_all(&root).expect("scratch dir");

    let prose = "Engineering memo\nThe team selected Rust as the implementation language for performance reasons. Options considered were Rust and Go. The assumption is that async Rust compile times remain acceptable.\n";
    let document = root.join("memo.txt");
    std::fs::write(&document, prose).expect("write prose document");

    let excerpt = "The team selected Rust as the implementation language for performance reasons.";
    let byte_start = prose.find(excerpt).expect("excerpt start");
    let byte_end = byte_start + excerpt.len();

    let response_path = root.join("response.json");
    std::fs::write(
        &response_path,
        serde_json::json!({
            "candidates": [{
                "file_index": 0,
                "source_span": {
                    "byte_start": byte_start,
                    "byte_end": byte_end,
                    "line_start": 2,
                    "line_end": 2
                },
                "title": "Use Rust as implementation language",
                "status": "proposed",
                "topic_keys": ["lang", "perf"],
                "rationale": "Performance requirements favour Rust.",
                "option_labels": ["Rust", "Go"],
                "chosen_option_label": "Rust",
                "evidence": ["async Rust compile times remain acceptable"],
                "hypotheses": ["Rust compile times will stay acceptable"],
                "explanation": "The excerpt names a clear choice between Rust and Go with a rationale."
            }]
        })
        .to_string(),
    )
    .expect("write LLM response");

    let import_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:test",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 hivemind dir"),
        "import",
        "documents",
        "--file",
        document.to_str().expect("utf-8 document path"),
        "--format",
        "text",
        "--llm-response",
        response_path.to_str().expect("utf-8 response path"),
    ]))
    .expect("prose import succeeds");

    let import_json: serde_json::Value =
        serde_json::from_str(&import_output).expect("valid import json");

    // One candidate proposed via prose extraction.
    assert_eq!(
        import_json["summary"]["prose_candidates_proposed"],
        serde_json::json!(1),
        "prose extraction must record candidates_proposed"
    );
    // The candidate was written (blocks_imported counts both block and prose paths).
    assert_eq!(
        import_json["summary"]["blocks_imported"],
        serde_json::json!(1),
        "prose candidate must land as imported block"
    );
    // The file status reflects prose extraction.
    assert_eq!(
        import_json["files"][0]["status"],
        serde_json::json!("prose_extracted"),
        "file status must be prose_extracted"
    );
    // The decision_id must be present in the block report.
    let decision_id = import_json["files"][0]["blocks"][0]["decision_id"]
        .as_str()
        .expect("decision_id in json output");
    assert!(!decision_id.is_empty(), "decision_id must be non-empty");

    // Verify the source_ref carries the extractor explanation.
    let ledger = crate::ledger::SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
    let events = ledger.read(0, 100).expect("events read");
    let proposal = events
        .iter()
        .find(|e| {
            e.event_type == crate::events::EventType::DecisionProposed
                && e.payload.get("title").and_then(|v| v.as_str())
                    == Some("Use Rust as implementation language")
        })
        .expect("decision proposal event");
    assert_eq!(
        proposal.source,
        crate::events::EventSource::Document,
        "prose candidate must have document source"
    );
    let source_ref: serde_json::Value =
        serde_json::from_str(proposal.source_ref.as_deref().expect("source_ref present"))
            .expect("source_ref is valid json");
    assert_eq!(source_ref["source"], serde_json::json!("document"));
    assert!(
        source_ref["extractor_explanation"]
            .as_str()
            .expect("extractor_explanation in source_ref")
            .contains("choice between Rust and Go"),
        "extractor_explanation must appear in source_ref"
    );
    // The event_type itself proves this is a proposal (no accept/reject = UNREVIEWED).
    assert_eq!(
        proposal.event_type,
        crate::events::EventType::DecisionProposed,
        "prose candidate must be a proposal event (UNREVIEWED)"
    );

    // Must appear in review --unreviewed-only.
    let review_cli = Cli::parse_from([
        "hivemind",
        "--actor",
        "reviewer:human",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 hivemind dir"),
        "review",
        "--since",
        "2000-01-01",
        "--unreviewed-only",
    ]);
    let crate::cli::args::Command::Review(review_args) = &review_cli.command else {
        panic!("expected review command");
    };
    let mut empty_input = std::io::Cursor::new(Vec::<u8>::new());
    let mut prompts = Vec::new();
    let review_result =
        run_review_session(&review_cli, review_args, &mut empty_input, &mut prompts)
            .expect("review session succeeds");
    let review_json: serde_json::Value =
        serde_json::from_str(&review_result).expect("valid review json");
    assert_eq!(
        review_json["matched_count"],
        serde_json::json!(1),
        "prose candidate must appear as unreviewed in review"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn import_documents_cli_blocks_path_used_when_extractor_present() {
    // A file WITH Decision: blocks uses the deterministic path even when extractor args supplied.
    let root = unique_test_dir("import-blocks-with-extractor");
    let hivemind_dir = root.join("hive");
    std::fs::create_dir_all(&root).expect("scratch dir");

    let doc_path = root.join("decision.md");
    std::fs::write(
        &doc_path,
        "Decision:\n  id: lang-choice\n  title: Choose Rust for the CLI\n  status: accepted\n  topic_keys: lang\n  rationale: Performance matters more than familiarity here.\n  options:\n    - Rust\n    - Python\n  chose: Rust\n",
    )
    .expect("write decision block");

    // Supply an extractor response (would be used for prose, but this file has explicit blocks).
    let response_path = root.join("response.json");
    std::fs::write(
        &response_path,
        serde_json::json!({"candidates": []}).to_string(),
    )
    .expect("write empty response");

    let import_output = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "importer:test",
        "--json",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 hivemind dir"),
        "import",
        "documents",
        "--file",
        doc_path.to_str().expect("utf-8 doc path"),
        "--llm-response",
        response_path.to_str().expect("utf-8 response path"),
    ]))
    .expect("block import with extractor args succeeds");

    let import_json: serde_json::Value =
        serde_json::from_str(&import_output).expect("valid import json");
    // Block file → processed via deterministic path, no prose candidates.
    assert_eq!(
        import_json["summary"]["blocks_imported"],
        serde_json::json!(1),
        "block file must use deterministic import path"
    );
    assert_eq!(
        import_json["summary"]["prose_candidates_proposed"],
        serde_json::json!(0),
        "block file must not trigger prose extraction"
    );
    assert_eq!(
        import_json["files"][0]["status"],
        serde_json::json!("processed"),
        "block file status must be processed, not prose_extracted"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// export command: directory writing, pruning, idempotence
// ---------------------------------------------------------------------------

/// Non-recursive listing of the regular files directly in `dir`, sorted by
/// name, with their bytes — enough to compare `INDEX.md`/`decisions/` layers
/// separately without pulling in a general-purpose recursive walker.
fn read_dir_files(dir: &std::path::Path) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(entry.path())?;
            out.push((name, bytes));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn export_writes_prunes_and_is_idempotent_body(backend: &TestBackend) -> CliTestResult {
    let accepted_id = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:alice",
            "emit",
            "decision.proposed",
            "--title",
            "Adopt Postgres session store",
            "--rationale",
            "Durable sessions across restarts",
            "--topic-keys",
            "auth",
            "--options",
            "postgres",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "emit",
            "decision.accepted",
            "--decision-id",
            &accepted_id,
        ],
    )))?;

    let rejected_id = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:alice",
            "emit",
            "decision.proposed",
            "--title",
            "Use bearer tokens for sessions",
            "--rationale",
            "Simpler client integration with fewer moving parts",
            "--topic-keys",
            "auth",
            "--options",
            "bearer",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:bob",
            "emit",
            "decision.rejected",
            "--decision-id",
            &rejected_id,
        ],
    )))?;

    let out_dir = unique_test_dir("export-out");
    let out_str = out_dir.to_str().expect("utf-8 temp path").to_owned();
    // Neither decision states a project, so both land in alice's personal project.
    let decisions_dir = out_dir
        .join("projects")
        .join("personal")
        .join("actor-alice")
        .join("decisions");

    let first_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json", "export", "--format", "markdown", "--out", &out_str,
        ],
    )))?;
    let first_report: serde_json::Value = serde_json::from_str(&first_output)?;
    ensure_eq(
        first_report["outcome"].as_str(),
        Some("exported"),
        "a written export reports outcome=exported",
    )?;
    ensure_eq(
        first_report["files_written"].as_u64(),
        Some(4),
        "first export writes the root INDEX.md, the project's INDEX.md and both decision files",
    )?;
    ensure_eq(
        first_report["files_removed"].as_u64(),
        Some(0),
        "first export into an empty directory removes nothing",
    )?;
    ensure_eq(
        read_dir_files(&decisions_dir)?.len(),
        2,
        "both decisions land in the personal project's decisions/ after the first export",
    )?;

    // Stray files the exporter must never touch: a non-.md file inside
    // decisions/, and a file outside decisions/ entirely.
    std::fs::write(decisions_dir.join("notes.txt"), b"scratch")?;
    std::fs::write(out_dir.join("README.md"), b"not managed by export")?;

    // Idempotence: same filters, unchanged ledger -> byte-identical tree, same file list.
    let root_before = read_dir_files(&out_dir)?;
    let decisions_before = read_dir_files(&decisions_dir)?;
    let second_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json", "export", "--format", "markdown", "--out", &out_str,
        ],
    )))?;
    let second_report: serde_json::Value = serde_json::from_str(&second_output)?;
    ensure_eq(
        second_report["files_removed"].as_u64(),
        Some(0),
        "idempotent re-export removes nothing",
    )?;
    ensure_eq(
        read_dir_files(&out_dir)?,
        root_before,
        "re-export produces byte-identical top-level files (INDEX.md and the stray README.md)",
    )?;
    ensure_eq(
        read_dir_files(&decisions_dir)?,
        decisions_before,
        "re-export produces byte-identical decisions/ files (both decisions and the stray notes.txt)",
    )?;

    // Pruning: a narrower --status drops the excluded decision's file and nothing else.
    let third_output = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json", "export", "--format", "markdown", "--out", &out_str, "--status", "accepted",
        ],
    )))?;
    let third_report: serde_json::Value = serde_json::from_str(&third_output)?;
    ensure_eq(
        third_report["files_written"].as_u64(),
        Some(3),
        "narrower export writes both INDEX.md files plus the one remaining decision file",
    )?;
    ensure_eq(
        third_report["files_removed"].as_u64(),
        Some(1),
        "narrower export prunes the now-excluded rejected decision's file",
    )?;

    let remaining = read_dir_files(&decisions_dir)?;
    ensure_eq(
        remaining.len(),
        2,
        "decisions/ keeps the still-matching decision file plus the untouched stray notes.txt",
    )?;
    ensure(
        remaining
            .iter()
            .filter(|(name, _)| name.ends_with(".md"))
            .count()
            == 1,
        "pruning leaves exactly the still-matching decision file",
    )?;
    ensure_eq(
        std::fs::read(decisions_dir.join("notes.txt"))?,
        b"scratch".to_vec(),
        "stray non-.md file in decisions/ is untouched by pruning",
    )?;
    ensure_eq(
        std::fs::read(out_dir.join("README.md"))?,
        b"not managed by export".to_vec(),
        "stray file outside decisions/ is untouched by pruning",
    )?;

    let _ = std::fs::remove_dir_all(&out_dir);
    Ok(())
}

#[test]
fn export_writes_prunes_and_is_idempotent() -> CliTestResult {
    export_writes_prunes_and_is_idempotent_body(&TestBackend::sqlite("export-tree"))
}

#[test]
fn export_writes_prunes_and_is_idempotent_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("export-tree-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    export_writes_prunes_and_is_idempotent_body(&backend)
}

fn export_per_project_body(backend: &TestBackend) -> CliTestResult {
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "human:alice",
            "project",
            "register",
            "billing",
            "--display-name",
            "Billing",
        ],
    )))?;
    run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--actor",
            "actor:alice",
            "emit",
            "decision.proposed",
            "--title",
            "Adopt Postgres session store",
            "--rationale",
            "Durable sessions across restarts",
            "--topic-keys",
            "auth",
            "--options",
            "postgres",
        ],
    )))?;

    let out_dir = unique_test_dir("export-per-project");
    let out_str = out_dir.to_str().expect("utf-8 temp path").to_owned();
    let personal_dir = out_dir
        .join("projects")
        .join("personal")
        .join("actor-alice");
    let billing_dir = out_dir.join("projects").join("billing");
    let export = |extra: &[&str]| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let mut rest = vec![
            "--json",
            "export",
            "--format",
            "markdown",
            "--out",
            out_str.as_str(),
        ];
        rest.extend_from_slice(extra);
        Ok(serde_json::from_str(&run(&Cli::parse_from(cli_args(
            backend, &rest,
        )))?)?)
    };

    // Full export: the registered-but-empty project and alice's personal project each get
    // their own record, and the root index has one section per project.
    let full = export(&[])?;
    ensure_eq(
        full["files_written"].as_u64(),
        Some(4),
        "root INDEX.md, billing's INDEX.md, alice's INDEX.md and her decision",
    )?;
    ensure(
        billing_dir.join("INDEX.md").is_file(),
        "a registered project with no decisions still gets its record",
    )?;
    ensure_eq(
        read_dir_files(&personal_dir.join("decisions"))?.len(),
        1,
        "the decision lands under the proposer's personal project",
    )?;
    let root_index = std::fs::read_to_string(out_dir.join("INDEX.md"))?;
    ensure(
        root_index.contains("## Billing (billing)")
            && root_index.contains("## Personal project: actor:alice"),
        "the root index has a section per project",
    )?;

    // What the export must leave alone, and what it now owns beyond `projects/`: the
    // layout before per-project grouping wrote `decisions/*.md` at the root.
    std::fs::write(billing_dir.join("notes.txt"), b"scratch")?;
    std::fs::write(out_dir.join("README.md"), b"not managed by export")?;
    std::fs::create_dir_all(out_dir.join("decisions"))?;
    std::fs::write(
        out_dir.join("decisions").join("old.md"),
        b"stale flat layout",
    )?;

    let billing_only = export(&["--project", "billing"])?;
    ensure_eq(
        billing_only["files_written"].as_u64(),
        Some(2),
        "--project billing writes the root INDEX.md and billing's INDEX.md only",
    )?;
    ensure_eq(
        billing_only["files_removed"].as_u64(),
        Some(3),
        "alice's project record and decision, and the stale flat-layout file, are pruned",
    )?;
    ensure(
        !out_dir.join("projects").join("personal").exists(),
        "pruning removes the directories it empties",
    )?;
    ensure(
        !out_dir.join("decisions").exists(),
        "the emptied legacy decisions/ directory goes too",
    )?;
    ensure_eq(
        std::fs::read(billing_dir.join("notes.txt"))?,
        b"scratch".to_vec(),
        "a non-.md file inside a project directory is untouched",
    )?;
    ensure_eq(
        std::fs::read(out_dir.join("README.md"))?,
        b"not managed by export".to_vec(),
        "a file outside the export's trees is untouched",
    )?;

    let personal_only = export(&["--project", "personal:actor:alice"])?;
    ensure_eq(
        personal_only["files_written"].as_u64(),
        Some(3),
        "a personal address resolves without being registered",
    )?;
    ensure_eq(
        personal_only["files_removed"].as_u64(),
        Some(1),
        "billing's INDEX.md is pruned; its notes.txt keeps the directory",
    )?;
    ensure(
        !billing_dir.join("INDEX.md").exists() && billing_dir.join("notes.txt").is_file(),
        "only billing's managed file went",
    )?;

    // A typo is a not_found envelope, never an empty export, and writes nothing.
    let missing_out = unique_test_dir("export-not-found");
    let missing_str = missing_out.to_str().expect("utf-8 temp path").to_owned();
    let json = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "export",
            "--format",
            "markdown",
            "--out",
            &missing_str,
            "--project",
            "billng",
        ],
    )))?;
    ensure_eq(
        serde_json::from_str::<serde_json::Value>(&json)?,
        serde_json::json!({"outcome": "not_found", "project": "billng"}),
        "an unknown handle is a not_found envelope",
    )?;
    let text = run(&Cli::parse_from(cli_args(
        backend,
        &[
            "export",
            "--format",
            "markdown",
            "--out",
            &missing_str,
            "--project",
            "billng",
        ],
    )))?;
    ensure_eq(
        text.as_str(),
        "outcome=not_found project=billng",
        "text form of the not_found envelope",
    )?;
    ensure(
        !missing_out.exists(),
        "an unknown project must not create the output directory",
    )?;

    let _ = std::fs::remove_dir_all(&out_dir);
    Ok(())
}

#[test]
fn export_groups_per_project_and_filters_by_project() -> CliTestResult {
    export_per_project_body(&TestBackend::sqlite("export-per-project"))
}

#[test]
fn export_groups_per_project_and_filters_by_project_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("export-per-project-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    export_per_project_body(&backend)
}

#[test]
fn export_out_pointing_at_file_fails_before_any_write() -> CliTestResult {
    let hivemind_dir = unique_test_dir("export-out-is-file-ledger");
    let out_path = unique_test_dir("export-out-is-file");
    std::fs::write(&out_path, b"not a directory")?;

    let error = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
        "export",
        "--format",
        "markdown",
        "--out",
        out_path.to_str().expect("utf-8 temp path"),
    ]))
    .expect_err("export refuses to treat a file as the output directory");
    ensure(
        error.to_string().contains("is a file"),
        "export --out-is-a-file error names the problem",
    )?;
    ensure(
        std::fs::metadata(&out_path)?.is_file(),
        "export must not touch --out when it already points at a file",
    )?;
    ensure(
        !hivemind_dir.exists(),
        "export must not create the ledger before validating --out",
    )?;

    let _ = std::fs::remove_file(&out_path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Projects on captures (hivemind-s15q.4): `--project` / `--project-source` on the capture
// verbs, and every reply naming the project it landed in.
// ---------------------------------------------------------------------------

const PROJECT_TEST_RATIONALE: &str = "Bounded retries avoid unbounded backlog growth under load";

fn register_test_project(backend: &TestBackend, handle: &str) -> CliTestResult {
    run(&Cli::parse_from(cli_args(
        backend,
        &["--actor", "human:alice", "project", "register", handle],
    )))?;
    Ok(())
}

/// Runs an `emit` command in text mode, returning stdout and what was written to the notices
/// stream (stderr in the real CLI).
fn run_emit_text(
    backend: &TestBackend,
    rest: &[&str],
) -> std::result::Result<(String, String), Box<dyn std::error::Error>> {
    let cli = Cli::parse_from(cli_args(backend, rest));
    let Command::Emit(emit) = &cli.command else {
        return Err("expected an emit command".into());
    };
    let mut notices = Vec::new();
    let stdout = run_emit_with_notices(&cli, emit, &mut notices)?;
    Ok((stdout, String::from_utf8(notices)?))
}

fn run_supersede_text(
    backend: &TestBackend,
    rest: &[&str],
) -> std::result::Result<(String, String), Box<dyn std::error::Error>> {
    let cli = Cli::parse_from(cli_args(backend, rest));
    let Command::Supersede(args) = &cli.command else {
        return Err("expected a supersede command".into());
    };
    let mut notices = Vec::new();
    let stdout = run_supersede_with_notices(&cli, args, &mut notices)?;
    Ok((stdout, String::from_utf8(notices)?))
}

fn capture_args_for<'a>(title: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "emit",
        "decision.capture",
        "--actor-id",
        "agent:claude:session-1",
        "--title",
        title,
        "--rationale",
        PROJECT_TEST_RATIONALE,
        "--topic-keys",
        "billing",
        "--options",
        "queue,sync",
        "--chose",
        "queue",
        "--bet",
    ];
    args.extend_from_slice(extra);
    args
}

/// A capture reply names what the decision rests on; the project tests compare the rest of the
/// envelope, so check the grounding is there (one declared bet, nothing stale) and set it aside.
fn without_bet_grounding(mut reply: serde_json::Value) -> serde_json::Value {
    let object = reply.as_object_mut().expect("the reply is a JSON object");
    let rests_on = object.remove("rests_on").expect("the reply names rests_on");
    assert_eq!(rests_on[0]["kind"], "bet", "{rests_on}");
    assert_eq!(rests_on.as_array().map(Vec::len), Some(1));
    assert_eq!(object.remove("premise_stale"), Some(serde_json::json!([])));
    reply
}

fn emit_capture_names_its_project_in_json_body(backend: &TestBackend) -> CliTestResult {
    register_test_project(backend, "billing")?;

    // Stated: `--project` alone records `stated`.
    let mut rest = vec!["--json"];
    rest.extend(capture_args_for(
        "Adopt async billing queue",
        &["--project", "billing"],
    ));
    let mut reply: serde_json::Value =
        serde_json::from_str(&run(&Cli::parse_from(cli_args(backend, &rest)))?)?;
    ensure(
        reply["value"]
            .as_str()
            .is_some_and(|id| id.starts_with("decision-")),
        "the envelope keeps the decision id in `value`",
    )?;
    reply["value"] = serde_json::json!("<decision-id>");
    ensure_json_eq(
        &without_bet_grounding(reply),
        serde_json::json!({
            "subcommand": "emit",
            "kind": "decision_id",
            "value": "<decision-id>",
            "project": "billing",
            "project_source": "stated",
        }),
        "stated project reply",
    )?;

    // The caller says how it determined the project; it is recorded as told. `emit
    // decision.proposed` takes the same arguments.
    let mut reply: serde_json::Value = serde_json::from_str(&run(&Cli::parse_from(cli_args(
        backend,
        &[
            "--json",
            "--actor",
            "agent:claude:session-1",
            "emit",
            "decision.proposed",
            "--title",
            "Adopt weekly billing exports",
            "--rationale",
            PROJECT_TEST_RATIONALE,
            "--topic-keys",
            "billing",
            "--options",
            "weekly,daily",
            "--project",
            "billing",
            "--project-source",
            "folder_marker",
        ],
    )))?)?;
    reply["value"] = serde_json::json!("<decision-id>");
    ensure_json_eq(
        &reply,
        serde_json::json!({
            "subcommand": "emit",
            "kind": "decision_id",
            "value": "<decision-id>",
            "project": "billing",
            "project_source": "folder_marker",
        }),
        "folder_marker project reply",
    )?;

    // No project: the personal fallback, named and announced.
    let mut rest = vec!["--json"];
    rest.extend(capture_args_for("Adopt nightly billing reports", &[]));
    let mut reply: serde_json::Value =
        serde_json::from_str(&run(&Cli::parse_from(cli_args(backend, &rest)))?)?;
    reply["value"] = serde_json::json!("<decision-id>");
    ensure_json_eq(
        &without_bet_grounding(reply.clone()),
        serde_json::json!({
            "subcommand": "emit",
            "kind": "decision_id",
            "value": "<decision-id>",
            "project": "personal:agent:claude",
            "project_source": "personal_fallback",
            "project_notice": crate::commands::PERSONAL_FALLBACK_NOTICE,
        }),
        "personal fallback reply",
    )?;
    ensure(
        reply["project_notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("saved to your personal project")),
        "the fallback reply says it was saved to the personal project",
    )
}

#[test]
fn emit_capture_names_its_project_in_json() -> CliTestResult {
    emit_capture_names_its_project_in_json_body(&TestBackend::sqlite("emit-project-json"))
}

#[test]
fn emit_capture_names_its_project_in_json_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("emit-project-json-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    emit_capture_names_its_project_in_json_body(&backend)
}

fn emit_capture_text_keeps_stdout_bare_and_announces_the_project_body(
    backend: &TestBackend,
) -> CliTestResult {
    register_test_project(backend, "billing")?;

    let (stdout, notices) = run_emit_text(
        backend,
        &capture_args_for("Adopt async billing queue", &["--project", "billing"]),
    )?;
    ensure(
        stdout.starts_with("decision-") && !stdout.contains(char::is_whitespace),
        "text stdout stays the bare decision id, so `$(hivemind emit ...)` still works",
    )?;
    ensure_eq(
        notices.as_str(),
        "project: billing (stated)\n",
        "a determined project is named, with no fallback sentence",
    )?;

    let (stdout, notices) = run_emit_text(
        backend,
        &capture_args_for("Adopt nightly billing reports", &[]),
    )?;
    ensure(
        stdout.starts_with("decision-") && !stdout.contains(char::is_whitespace),
        "text stdout is still the bare decision id on fallback",
    )?;
    ensure(
        notices.starts_with("project: personal:agent:claude (personal_fallback) — ")
            && notices.contains("saved to your personal project"),
        "the fallback is announced with the personal address and the sentence",
    )?;
    ensure(
        notices.ends_with('\n') && notices.lines().count() == 1,
        "one announcement line",
    )
}

#[test]
fn emit_capture_text_keeps_stdout_bare_and_announces_the_project() -> CliTestResult {
    emit_capture_text_keeps_stdout_bare_and_announces_the_project_body(&TestBackend::sqlite(
        "emit-project-text",
    ))
}

#[test]
fn emit_capture_text_keeps_stdout_bare_and_announces_the_project_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("emit-project-text-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    emit_capture_text_keeps_stdout_bare_and_announces_the_project_body(&backend)
}

#[test]
fn emit_capture_refuses_an_unknown_project_with_the_register_hint() -> CliTestResult {
    let backend = TestBackend::sqlite("emit-project-unknown");
    let error = run_emit_text(
        &backend,
        &capture_args_for(
            "Adopt async billing queue",
            &["--project", "not-registered"],
        ),
    )
    .expect_err("an unregistered handle is refused");
    ensure(
        error
            .to_string()
            .contains("project not registered: not-registered")
            && error
                .to_string()
                .contains("hivemind project register not-registered"),
        "the refusal names the handle and the register command",
    )
}

#[test]
fn emit_capture_project_source_needs_a_project_and_a_caller_claimable_value() -> CliTestResult {
    let dir = unique_test_dir("emit-project-source-args");
    let dir = dir.to_str().expect("utf-8 temp path");
    let parse = |extra: &[&str]| {
        let mut argv = vec!["hivemind", "--hivemind-dir", dir];
        argv.extend(capture_args_for("Adopt async billing queue", extra));
        Cli::try_parse_from(argv)
    };

    ensure(
        parse(&["--project", "billing", "--project-source", "rig"]).is_ok(),
        "a project with its source parses",
    )?;
    ensure(
        parse(&["--project-source", "rig"]).is_err(),
        "--project-source without --project is refused at parse time",
    )?;
    for reserved in ["personal_fallback", "moved"] {
        ensure(
            parse(&["--project", "billing", "--project-source", reserved]).is_err(),
            "HiveMind records personal_fallback and moved itself; a caller cannot claim them",
        )?;
    }
    Ok(())
}

fn supersede_names_its_project_body(backend: &TestBackend) -> CliTestResult {
    register_test_project(backend, "billing")?;
    register_test_project(backend, "payments")?;

    let capture_id =
        |extra: &[&str], title: &str| -> std::result::Result<String, Box<dyn std::error::Error>> {
            let (stdout, _notices) = run_emit_text(backend, &capture_args_for(title, extra))?;
            Ok(stdout)
        };
    let supersede =
        |old: &str,
         title: &str,
         extra: &[&str]|
         -> std::result::Result<(serde_json::Value, String), Box<dyn std::error::Error>> {
            let mut rest = vec![
                "--json",
                "--actor",
                "agent:claude:session-1",
                "supersede",
                "--old",
                old,
                "--title",
                title,
                "--rationale",
                PROJECT_TEST_RATIONALE,
                "--bet",
            ];
            rest.extend_from_slice(extra);
            let reply: serde_json::Value =
                serde_json::from_str(&run(&Cli::parse_from(cli_args(backend, &rest)))?)?;
            let new_id = reply["new_decision_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            Ok((reply, new_id))
        };

    // Not stated: the superseding decision inherits the old project and how it was
    // determined.
    let old = capture_id(
        &["--project", "billing", "--project-source", "rig"],
        "Use shared admin token",
    )?;
    let (reply, new_id) = supersede(&old, "Use scoped service tokens", &[])?;
    ensure_eq(
        reply["project"].as_str(),
        Some("billing"),
        "inherited project",
    )?;
    ensure_eq(
        reply["project_source"].as_str(),
        Some("rig"),
        "inherited project_source",
    )?;
    ensure(
        reply.get("project_notice").is_none(),
        "an inherited project is not a fallback",
    )?;

    // Stated: overrides the inherited project.
    let (reply, _) = supersede(
        &new_id,
        "Move token handling to payments",
        &["--project", "payments"],
    )?;
    ensure_eq(
        reply["project"].as_str(),
        Some("payments"),
        "stated project",
    )?;
    ensure_eq(
        reply["project_source"].as_str(),
        Some("stated"),
        "stated project_source",
    )?;

    // A decision that itself fell back to a personal project stays there, announced.
    let old = capture_id(&[], "Use shared deploy key")?;
    let (reply, new_id) = supersede(&old, "Use per-host deploy keys", &[])?;
    ensure_eq(
        reply["project"].as_str(),
        Some("personal:agent:claude"),
        "personal address",
    )?;
    ensure_eq(
        reply["project_source"].as_str(),
        Some("personal_fallback"),
        "fallback source",
    )?;
    ensure_eq(
        reply["project_notice"].as_str(),
        Some(crate::commands::PERSONAL_FALLBACK_NOTICE),
        "fallback notice",
    )?;

    // Text mode: the key=value stdout line gains the project, the announcement goes to notices.
    let (stdout, notices) = run_supersede_text(
        backend,
        &[
            "--actor",
            "agent:claude:session-1",
            "supersede",
            "--old",
            &new_id,
            "--title",
            "Rotate per-host deploy keys monthly",
            "--rationale",
            PROJECT_TEST_RATIONALE,
            "--bet",
            "--project",
            "billing",
            "--project-source",
            "current_project",
        ],
    )?;
    ensure(
        stdout.contains(" project=billing project_source=current_project")
            && !stdout.contains('\n'),
        "supersede stdout stays one key=value line and carries the project",
    )?;
    ensure_eq(
        notices.as_str(),
        "project: billing (current_project)\n",
        "supersede announcement",
    )?;

    let error = run_supersede_text(
        backend,
        &[
            "--actor",
            "agent:claude:session-1",
            "supersede",
            "--old",
            &old,
            "--title",
            "Use hardware tokens instead",
            "--rationale",
            PROJECT_TEST_RATIONALE,
            "--bet",
            "--project",
            "not-registered",
        ],
    )
    .expect_err("an unregistered handle is refused");
    ensure(
        error
            .to_string()
            .contains("project not registered: not-registered"),
        "supersede refuses an unknown handle with the register hint",
    )
}

#[test]
fn supersede_names_its_project() -> CliTestResult {
    supersede_names_its_project_body(&TestBackend::sqlite("supersede-project"))
}

#[test]
fn supersede_names_its_project_postgres() -> CliTestResult {
    let Some(backend) = TestBackend::postgres("supersede-project-pg") else {
        eprintln!("skipping; set HIVEMIND_TEST_POSTGRES_URL");
        return Ok(());
    };
    supersede_names_its_project_body(&backend)
}

// ---------------------------------------------------------------------------
// Grounded capture (hivemind-gwhr.2): "what does this decision rest on?"
// ---------------------------------------------------------------------------

fn grounding_cli(
    hivemind_dir: &std::path::Path,
    json: bool,
    args: &[&str],
) -> crate::Result<String> {
    let mut argv = vec!["hivemind", "--actor", "agent:claude:grounding-test"];
    if json {
        argv.push("--json");
    }
    argv.extend([
        "--hivemind-dir",
        hivemind_dir.to_str().expect("utf-8 temp path"),
    ]);
    argv.extend(args.iter().copied());
    run(&Cli::parse_from(argv))
}

/// `emit decision.capture` args for a decision titled `title`, with `grounding` appended.
fn grounded_capture_args<'a>(title: &'a str, grounding: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "emit",
        "decision.capture",
        "--agent-tool",
        "claude",
        "--agent-session",
        "grounding-test",
        "--title",
        title,
        "--rationale",
        "Rationale text long enough for the readable floor and then some",
        "--topic-keys",
        "grounding",
        "--options",
        "adopt,skip",
        "--chose",
        "adopt",
    ];
    args.extend(grounding.iter().copied());
    args
}

fn ledger_event_count(hivemind_dir: &std::path::Path) -> usize {
    SqliteEventLedger::open(hivemind_dir)
        .expect("ledger opens")
        .read(0, 500)
        .expect("read events")
        .len()
}

fn seed_decision(hivemind_dir: &std::path::Path, title: &str) -> String {
    grounding_cli(
        hivemind_dir,
        false,
        &[
            "emit",
            "decision.proposed",
            "--title",
            title,
            "--rationale",
            "Rationale text long enough for the readable floor and then some",
            "--topic-keys",
            "grounding",
            "--options",
            "only",
        ],
    )
    .expect("seed decision")
}

#[test]
fn decision_capture_without_grounding_is_refused_with_exit_2_and_writes_nothing() {
    let hivemind_dir = unique_test_dir("capture-no-grounding");

    let error = grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args("Adopt the new queue", &[]),
    )
    .expect_err("a capture that names nothing it rests on is refused");

    assert_eq!(exit_code_for_error(&error).code(), 2);
    let message = error.to_string();
    for flag in [
        "--rests-on-decision",
        "--rests-on-evidence",
        "--rests-on-assumption",
        "--bet",
        "--quote",
    ] {
        assert!(
            message.contains(flag),
            "refusal must name {flag}: {message}"
        );
    }
    assert!(message.contains("nothing was written"), "{message}");
    assert_eq!(ledger_event_count(&hivemind_dir), 0);

    // Raw `emit decision.proposed` never asks the question.
    seed_decision(&hivemind_dir, "A raw decision needs no grounding");

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_records_every_grounding_kind_and_replies_with_rests_on() {
    let hivemind_dir = unique_test_dir("capture-grounded");
    let goal_id = seed_decision(&hivemind_dir, "Keep the ledger append-only");

    let reply = grounding_cli(
        &hivemind_dir,
        true,
        &grounded_capture_args(
            "Adopt the new queue",
            &[
                "--rests-on-decision",
                "Keep the ledger append-only",
                "--rests-on-evidence",
                "p95 latency was 180ms, run 42",
                "--evidence-source",
                "ci run 42",
                "--rests-on-assumption",
                "traffic stays under 1k rps",
                "--bet",
                "the vendor survives the year",
                "--would-change-if",
                "they raise prices",
                "--check-by",
                "2026-12-01",
                "--confidence",
                "high",
            ],
        ),
    )
    .expect("grounded capture succeeds");
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("json reply");
    let decision_id = reply["value"].as_str().expect("decision id").to_owned();

    let rests_on = reply["rests_on"].as_array().expect("rests_on");
    let kinds: Vec<&str> = rests_on
        .iter()
        .map(|item| item["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["decision", "evidence", "assumption", "bet"]);
    assert_eq!(rests_on[0]["id"], goal_id);
    assert_eq!(rests_on[0]["label"], "Keep the ledger append-only");
    assert_eq!(rests_on[1]["label"], "p95 latency was 180ms, run 42");
    assert_eq!(rests_on[3]["label"], "the vendor survives the year");
    assert_eq!(reply["premise_stale"], serde_json::json!([]));

    let events = SqliteEventLedger::open(&hivemind_dir)
        .expect("ledger opens")
        .read(0, 500)
        .expect("read events");
    let proposal = events
        .iter()
        .find(|event| {
            event.event_type == crate::events::EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(|v| v.as_str()) == Some(&decision_id)
        })
        .expect("proposal event");
    assert_eq!(proposal.payload["expressed_confidence"], "high");
    let evidence = events
        .iter()
        .find(|event| event.event_type == crate::events::EventType::EvidenceRecorded)
        .expect("evidence event");
    assert_eq!(evidence.payload["source"], "ci run 42");
    let bet = events
        .iter()
        .find(|event| {
            event.event_type == crate::events::EventType::HypothesisRecorded
                && event.payload.get("kind").and_then(|v| v.as_str()) == Some("bet")
        })
        .expect("bet event");
    assert_eq!(bet.payload["would_change_if"], "they raise prices");
    assert!(bet.payload["check_by"]
        .as_str()
        .expect("check_by")
        .starts_with("2026-12-01"));

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_bet_without_a_statement_records_a_judgement_call() {
    let hivemind_dir = unique_test_dir("capture-bare-bet");

    grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args("Ship the beta", &["--bet"]),
    )
    .expect("a bare bet is a declared grounding");

    let events = SqliteEventLedger::open(&hivemind_dir)
        .expect("ledger opens")
        .read(0, 500)
        .expect("read events");
    let bet = events
        .iter()
        .find(|event| event.event_type == crate::events::EventType::HypothesisRecorded)
        .expect("bet event");
    assert_eq!(bet.payload["statement"], "Judgement call: Ship the beta");
    assert_eq!(bet.payload["kind"], "bet");

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_bet_modifiers_require_a_bet() {
    // `--would-change-if` and `--check-by` describe a bet; without `--bet` clap refuses them
    // while parsing (before anything runs), and with it they parse.
    for modifier in [["--would-change-if", "x"], ["--check-by", "2026-12-01"]] {
        let mut argv = vec![
            "hivemind",
            "emit",
            "decision.capture",
            "--title",
            "t",
            "--rationale",
            "r",
        ];
        argv.extend(modifier);
        assert!(
            Cli::try_parse_from(argv.clone()).is_err(),
            "{modifier:?} without --bet must not parse"
        );
        argv.push("--bet");
        assert!(
            Cli::try_parse_from(argv).is_ok(),
            "{modifier:?} with --bet must parse"
        );
    }
}

#[test]
fn decision_capture_evidence_sources_are_index_aligned_with_evidence() {
    let hivemind_dir = unique_test_dir("capture-evidence-alignment");

    let mismatched = grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args(
            "Adopt the new queue",
            &[
                "--rests-on-evidence",
                "first observation",
                "--rests-on-evidence",
                "second observation",
                "--evidence-source",
                "only one source",
            ],
        ),
    )
    .expect_err("one source for two evidence items is refused");
    assert_eq!(exit_code_for_error(&mismatched).code(), 2);
    assert!(
        mismatched.to_string().contains("index-aligned")
            && mismatched
                .to_string()
                .contains("1 source(s) for 2 evidence item(s)"),
        "{mismatched}"
    );

    let orphan_source = grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args(
            "Adopt the new queue",
            &[
                "--rests-on-assumption",
                "an assumption",
                "--evidence-source",
                "somewhere",
            ],
        ),
    )
    .expect_err("a source with no evidence is refused");
    assert!(
        orphan_source
            .to_string()
            .contains("--evidence-source needs a matching --rests-on-evidence"),
        "{orphan_source}"
    );
    assert_eq!(ledger_event_count(&hivemind_dir), 0);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_with_an_ambiguous_premise_writes_nothing_and_hash_n_resolves_it() {
    let hivemind_dir = unique_test_dir("capture-ambiguous-premise");
    let first_id = seed_decision(&hivemind_dir, "Adopt the queue");
    let second_id = seed_decision(&hivemind_dir, "Adopt the queue");
    let events_before = ledger_event_count(&hivemind_dir);

    let ambiguous = grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args(
            "Ship the queue",
            &[
                "--rests-on-assumption",
                "an assumption",
                "--rests-on-decision",
                "Adopt the queue",
            ],
        ),
    )
    .expect_err("an ambiguous premise refuses the capture");
    assert_eq!(exit_code_for_error(&ambiguous).code(), 2);
    let message = ambiguous.to_string();
    assert!(message.contains("matches 2 decisions"), "{message}");
    assert!(
        message.contains("#1") && message.contains("#2"),
        "{message}"
    );
    assert!(
        message.contains(&first_id) && message.contains(&second_id),
        "{message}"
    );
    assert!(message.contains("--rests-on-decision '#N'"), "{message}");
    assert_eq!(
        ledger_event_count(&hivemind_dir),
        events_before,
        "an ambiguous premise must write nothing — not even the assumption named beside it"
    );

    // The candidate list is on disk: re-running with '#1' resolves the first candidate.
    let reply = grounding_cli(
        &hivemind_dir,
        true,
        &grounded_capture_args(
            "Ship the queue",
            &[
                "--rests-on-assumption",
                "an assumption",
                "--rests-on-decision",
                "#1",
            ],
        ),
    )
    .expect("'#1' resolves against the previous candidate list");
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("json reply");
    let premise = reply["rests_on"]
        .as_array()
        .expect("rests_on")
        .iter()
        .find(|item| item["kind"] == "decision")
        .expect("decision premise");
    assert!(premise["id"] == first_id.as_str() || premise["id"] == second_id.as_str());

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_with_an_unmatched_premise_writes_nothing() {
    let hivemind_dir = unique_test_dir("capture-unmatched-premise");
    seed_decision(&hivemind_dir, "Keep the ledger append-only");
    let events_before = ledger_event_count(&hivemind_dir);

    for premise in ["quantum flux capacitor", "decision-does-not-exist"] {
        let error = grounding_cli(
            &hivemind_dir,
            false,
            &grounded_capture_args(
                "Adopt the new queue",
                &[
                    "--rests-on-evidence",
                    "an observation that must not be stranded",
                    "--rests-on-decision",
                    premise,
                ],
            ),
        )
        .expect_err("an unmatched premise refuses the capture");
        assert_eq!(exit_code_for_error(&error).code(), 2);
        assert!(
            error
                .to_string()
                .contains(&format!("no decision matches '{premise}'")),
            "{error}"
        );
    }
    assert_eq!(ledger_event_count(&hivemind_dir), events_before);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_with_only_close_premise_candidates_says_what_they_lack_and_writes_nothing() {
    let hivemind_dir = unique_test_dir("capture-close-premise");
    let queue_id = seed_decision(&hivemind_dir, "Adopt async queue for billing");
    let events_before = ledger_event_count(&hivemind_dir);

    let error = grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args(
            "Ship the billing queue",
            &["--rests-on-decision", "adopt async queue unicorns"],
        ),
    )
    .expect_err("a close-only premise is not picked for you");
    assert_eq!(exit_code_for_error(&error).code(), 2);
    let message = error.to_string();
    assert!(
        message.contains("no decision matches every word of 'adopt async queue unicorns'"),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "#1 Adopt async queue for billing ({queue_id}) missing: unicorns"
        )),
        "{message}"
    );
    assert!(message.contains("--rests-on-decision '#N'"), "{message}");
    assert_eq!(ledger_event_count(&hivemind_dir), events_before);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn decision_capture_reports_a_stale_premise_in_json_and_text() {
    let hivemind_dir = unique_test_dir("capture-stale-premise");
    let old_id = seed_decision(&hivemind_dir, "Use the shared admin token");
    let new_id = seed_decision(&hivemind_dir, "Use scoped service tokens");
    grounding_cli(
        &hivemind_dir,
        false,
        &[
            "emit",
            "decision.superseded",
            "--old",
            &old_id,
            "--new",
            &new_id,
        ],
    )
    .expect("supersede the old decision");

    let reply = grounding_cli(
        &hivemind_dir,
        true,
        &grounded_capture_args("Rotate the admin token", &["--rests-on-decision", &old_id]),
    )
    .expect("a stale premise is allowed");
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("json reply");
    assert_eq!(reply["premise_stale"], serde_json::json!([old_id]));

    let text = grounding_cli(
        &hivemind_dir,
        false,
        &grounded_capture_args(
            "Rotate the admin token again",
            &["--rests-on-decision", &old_id],
        ),
    )
    .expect("a stale premise is allowed");
    let mut lines = text.lines();
    assert!(lines.next().expect("first line").starts_with("decision-"));
    assert!(
        lines
            .next()
            .expect("stale line")
            .starts_with(&format!("premise_stale: {old_id}")),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

fn grounded_supersede_args<'a>(old_id: &'a str, grounding: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "supersede",
        "--old",
        old_id,
        "--title",
        "Use scoped service tokens",
        "--rationale",
        "Scoped tokens preserve audit boundaries for every caller",
        "--options",
        "scoped-service-tokens",
        "--chose",
        "scoped-service-tokens",
    ];
    args.extend(grounding.iter().copied());
    args
}

#[test]
fn supersede_without_grounding_is_refused_and_with_grounding_records_it() {
    let hivemind_dir = unique_test_dir("supersede-grounded");
    let old_id = seed_decision(&hivemind_dir, "Use shared admin token");
    let goal_id = seed_decision(&hivemind_dir, "Keep audit boundaries");
    let events_before = ledger_event_count(&hivemind_dir);

    let error = grounding_cli(&hivemind_dir, false, &grounded_supersede_args(&old_id, &[]))
        .expect_err("a supersede that names nothing it rests on is refused");
    assert_eq!(exit_code_for_error(&error).code(), 2);
    assert!(error.to_string().contains("--rests-on-decision"), "{error}");
    assert_eq!(ledger_event_count(&hivemind_dir), events_before);

    let grounded = grounded_supersede_args(
        &old_id,
        &[
            "--rests-on-decision",
            "Keep audit boundaries",
            "--rests-on-evidence",
            "the shared token leaked twice",
            "--evidence-source",
            "incident 7",
        ],
    );
    let reply = grounding_cli(&hivemind_dir, true, &grounded).expect("grounded supersede");
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("json reply");
    assert_eq!(reply["old_decision_status"], "superseded");
    let kinds: Vec<&str> = reply["rests_on"]
        .as_array()
        .expect("rests_on")
        .iter()
        .map(|item| item["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["decision", "evidence"]);
    assert_eq!(reply["rests_on"][0]["id"], goal_id);

    // An identical retry is still idempotent: same supersession, nothing appended.
    let events_after_first = ledger_event_count(&hivemind_dir);
    let retry = grounding_cli(&hivemind_dir, true, &grounded).expect("retry succeeds");
    let retry: serde_json::Value = serde_json::from_str(&retry).expect("json reply");
    assert_eq!(retry["new_decision_id"], reply["new_decision_id"]);
    assert_eq!(ledger_event_count(&hivemind_dir), events_after_first);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}
