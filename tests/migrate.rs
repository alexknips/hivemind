/// Integration tests for `hivemind migrate --to <postgres-url>` (SQLite → Postgres round-trip).
///
/// Requires the `shared-backend-postgres` feature and a live Postgres instance.
/// Set HIVEMIND_TEST_POSTGRES_URL to run; tests are skipped when unset. The same moves over
/// HTTP (`--to-url`) are `tests/migrate_http.rs`.
#[cfg(feature = "shared-backend-postgres")]
mod migrate_tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use hivemind::cli::{Cli, Command, MigrateArgs};
    use hivemind::events::{Event, EventSource, EventType, TenantId};
    use hivemind::ledger::{EventLedger, PostgresEventLedger, SqliteEventLedger};

    const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

    fn skip_if_no_postgres() -> Option<String> {
        std::env::var(TEST_DATABASE_URL_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
    }

    fn unique_tenant(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("tenant:test:{prefix}:{nanos}:{}", std::process::id())
    }

    fn make_test_event(label: &str) -> Event {
        Event {
            tenant_id: TenantId::local(),
            event_id: None,
            event_uuid: uuid::Uuid::new_v4(),
            correlation_id: None,
            causation_event_id: None,
            event_type: EventType::EvidenceRecorded,
            actor_id: "actor:test".to_owned(),
            source: EventSource::Cli,
            source_ref: None,
            payload: serde_json::json!({
                "evidence_id": label,
                "content": format!("content for {label}"),
                "source": "migrate-test"
            }),
            ts: Some(chrono::Utc::now()),
        }
    }

    fn test_cli(source_dir: std::path::PathBuf, pg_url: String, tenant: String) -> Cli {
        Cli {
            actor: "human:test".to_owned(),
            actor_given: hivemind::cli::ActorGiven(true),
            tenant: "local".to_owned(),
            json: true,
            database_url: None,
            hivemind_dir: source_dir,
            graph_backend: None,
            verbose: 0,
            command: Command::Migrate(MigrateArgs {
                from: None,
                to: Some(pg_url),
                to_url: None,
                admin_key_file: None,
                to_tenant: tenant,
                dry_run: false,
            }),
        }
    }

    #[test]
    fn migrate_round_trip_preserves_all_events() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping migrate test; set {TEST_DATABASE_URL_ENV}");
            return;
        };

        let source_dir = tempfile::tempdir().expect("tempdir");
        let sqlite = SqliteEventLedger::open(source_dir.path()).expect("sqlite open");

        sqlite
            .append(make_test_event("evidence-a"))
            .expect("append a");
        sqlite
            .append(make_test_event("evidence-b"))
            .expect("append b");
        sqlite
            .append(make_test_event("evidence-c"))
            .expect("append c");

        let tenant = unique_tenant("round-trip");
        let cli = test_cli(source_dir.path().to_owned(), pg_url, tenant.clone());
        let output = hivemind::cli::run(&cli).expect("run migrate");

        let report: serde_json::Value =
            serde_json::from_str(&output).expect("parse migrate JSON output");

        assert_eq!(report["dry_run"], false);
        assert_eq!(report["destination"], "postgres");
        assert_eq!(report["source_event_count"], 3);
        assert_eq!(report["new_events"], 3);
        assert_eq!(report["already_present"], 0);
        assert_eq!(report["source_tenant"], "local");
        assert_eq!(report["destination_tenant"], tenant.as_str());
        assert!(
            report["parity_check"]["ok"].as_bool().unwrap_or(false),
            "parity check failed: {report}"
        );
        assert_eq!(report["parity_check"]["present_in_destination"], 3);
        assert_eq!(report["parity_check"]["missing"], 0);
    }

    #[test]
    fn migrate_is_idempotent() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping migrate idempotent test; set {TEST_DATABASE_URL_ENV}");
            return;
        };

        let source_dir = tempfile::tempdir().expect("tempdir");
        let sqlite = SqliteEventLedger::open(source_dir.path()).expect("sqlite open");

        sqlite
            .append(make_test_event("evidence-x"))
            .expect("append x");
        sqlite
            .append(make_test_event("evidence-y"))
            .expect("append y");

        let tenant = unique_tenant("idempotent");

        // First migration.
        let cli1 = test_cli(source_dir.path().to_owned(), pg_url.clone(), tenant.clone());
        hivemind::cli::run(&cli1).expect("first run");

        // Second migration of the same source: should succeed without duplicating events.
        let cli2 = test_cli(source_dir.path().to_owned(), pg_url.clone(), tenant.clone());
        let output2 = hivemind::cli::run(&cli2).expect("second run");
        let report2: serde_json::Value =
            serde_json::from_str(&output2).expect("parse second run output");

        // The second run found both uuids already there and wrote nothing.
        assert_eq!(report2["source_event_count"], 2);
        assert_eq!(report2["new_events"], 0);
        assert_eq!(report2["already_present"], 2);
        assert!(report2["parity_check"]["ok"].as_bool().unwrap_or(false));
        assert_eq!(
            PostgresEventLedger::connect(&pg_url, &tenant)
                .expect("connect")
                .latest_offset()
                .expect("offset"),
            2,
            "second migration should not duplicate events"
        );
    }

    #[test]
    fn migrate_dry_run_reports_count_without_writing() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping migrate dry-run test; set {TEST_DATABASE_URL_ENV}");
            return;
        };

        let source_dir = tempfile::tempdir().expect("tempdir");
        let sqlite = SqliteEventLedger::open(source_dir.path()).expect("sqlite open");

        sqlite
            .append(make_test_event("dry-a"))
            .expect("append dry-a");
        sqlite
            .append(make_test_event("dry-b"))
            .expect("append dry-b");

        let tenant = unique_tenant("dry-run");
        let pg_url_for_check = pg_url.clone();

        let cli = Cli {
            actor: "human:test".to_owned(),
            actor_given: hivemind::cli::ActorGiven(true),
            tenant: "local".to_owned(),
            json: true,
            database_url: None,
            hivemind_dir: source_dir.path().to_owned(),
            graph_backend: None,
            verbose: 0,
            command: Command::Migrate(MigrateArgs {
                from: None,
                to: Some(pg_url),
                to_url: None,
                admin_key_file: None,
                to_tenant: tenant.clone(),
                dry_run: true,
            }),
        };

        let output = hivemind::cli::run(&cli).expect("dry run migrate");
        let report: serde_json::Value =
            serde_json::from_str(&output).expect("parse dry-run output");

        assert_eq!(report["dry_run"], true);
        assert_eq!(report["source_event_count"], 2);
        assert_eq!(report["new_events"], 2);
        assert_eq!(report["already_present"], 0);
        assert!(
            report["parity_check"].is_null(),
            "dry run should have no parity check"
        );
        assert_eq!(
            PostgresEventLedger::connect(&pg_url_for_check, &tenant)
                .expect("connect")
                .latest_offset()
                .expect("offset"),
            0,
            "a dry run writes nothing"
        );
    }

    #[test]
    fn migrate_from_flag_strips_sqlite_prefix() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping migrate --from test; set {TEST_DATABASE_URL_ENV}");
            return;
        };

        let source_dir = tempfile::tempdir().expect("tempdir");
        let sqlite = SqliteEventLedger::open(source_dir.path()).expect("sqlite open");
        sqlite
            .append(make_test_event("from-flag-event"))
            .expect("append");

        let tenant = unique_tenant("from-flag");
        let from_url = format!("sqlite://{}", source_dir.path().display());

        let cli = Cli {
            actor: "human:test".to_owned(),
            actor_given: hivemind::cli::ActorGiven(true),
            tenant: "local".to_owned(),
            json: true,
            database_url: None,
            hivemind_dir: std::path::PathBuf::from("/nonexistent"),
            graph_backend: None,
            verbose: 0,
            command: Command::Migrate(MigrateArgs {
                from: Some(from_url),
                to: Some(pg_url),
                to_url: None,
                admin_key_file: None,
                to_tenant: tenant,
                dry_run: false,
            }),
        };

        let output = hivemind::cli::run(&cli).expect("migrate with --from");
        let report: serde_json::Value = serde_json::from_str(&output).expect("parse output");

        assert_eq!(report["new_events"], 1);
        assert!(report["parity_check"]["ok"].as_bool().unwrap_or(false));
    }

    #[test]
    fn migrate_into_a_non_empty_tenant_renumbers_causation_by_uuid() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping non-empty migrate test; set {TEST_DATABASE_URL_ENV}");
            return;
        };

        let tenant = unique_tenant("non-empty");
        // The destination already holds 3 events, so each migrated event lands at a different
        // number than it had in the source.
        let destination = PostgresEventLedger::connect(&pg_url, &tenant).expect("connect");
        for label in ["resident-a", "resident-b", "resident-c"] {
            destination
                .append(make_test_event(label))
                .expect("resident");
        }

        let source_dir = tempfile::tempdir().expect("tempdir");
        let sqlite = SqliteEventLedger::open(source_dir.path()).expect("sqlite open");
        let mut cause = None;
        for label in ["chain-1", "chain-2", "chain-3"] {
            let mut event = make_test_event(label);
            event.causation_event_id = cause;
            cause = Some(sqlite.append(event).expect("append chain event"));
        }

        let cli = test_cli(source_dir.path().to_owned(), pg_url.clone(), tenant.clone());
        let output = hivemind::cli::run(&cli).expect("run migrate");
        let report: serde_json::Value = serde_json::from_str(&output).expect("parse output");
        assert_eq!(report["new_events"], 3);
        assert_eq!(report["parity_check"]["ok"], true);

        let landed = destination.read(3, 10).expect("read migrated events");
        assert_eq!(landed.len(), 3);
        assert_eq!(landed[0].causation_event_id, None);
        assert_eq!(landed[1].causation_event_id, landed[0].event_id);
        assert_eq!(landed[2].causation_event_id, landed[1].event_id);
        assert_eq!(landed[0].event_id, Some(4), "numbered after the residents");

        // A re-run writes nothing.
        let output = hivemind::cli::run(&cli).expect("re-run migrate");
        let report: serde_json::Value = serde_json::from_str(&output).expect("parse output");
        assert_eq!(report["new_events"], 0);
        assert_eq!(report["already_present"], 3);
        assert_eq!(destination.latest_offset().expect("offset"), 6);
    }
}
