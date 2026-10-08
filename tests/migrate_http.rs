//! `hivemind migrate --to-url` against a live cell (hivemind-jawy): a SQLite ledger moves into a
//! cell over HTTP with the cell's admin key, no database password anywhere.
//!
//! The cell here is the real HTTP router served on a loopback port over a SQLite directory the
//! test also reads directly, so what the replay route wrote is checked event by event. The
//! destination already holds events before the move, which is the case the old
//! `migrate --to <postgres>` got wrong: the destination renumbers `event_id`, so a causation id
//! copied verbatim points at an unrelated event.
//!
//! The same rules against Postgres are the `replay_into_a_non_empty_ledger_agrees_with_the_contract`
//! ledger test and `tests/migrate.rs`, which run in CI against a live database.

use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::{Event, EventId, EventSource, EventType, TenantId};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::{json, Value};
use uuid::Uuid;

const ADMIN_KEY: &str = "migrate-http-test-admin-key";

struct Cell {
    base_url: String,
    dir: PathBuf,
    key_file: PathBuf,
}

fn scratch(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-migrate-http-{prefix}-{}", Uuid::new_v4()))
}

/// Serves the real router over a fresh SQLite directory on a loopback port, with the admin key
/// set. The server thread lives until the test process exits.
fn start_cell() -> Cell {
    let dir = scratch("cell");
    std::fs::create_dir_all(&dir).expect("cell dir");
    let config = hivemind::api::ApiConfig {
        hivemind_dir: dir.clone(),
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: None,
        database_url: None,
        admin_key: Some(ADMIN_KEY.to_owned()),
        workos_domain: None,
        workos_issuer: None,
        workos_jwks_url: None,
        workos_audience: None,
        spa_dir: None,
        cors_origins: vec![],
        slack_client_id: None,
        slack_client_secret: None,
        slack_signing_secret: None,
        slack_api_base_url: None,
    };
    let router = hivemind::api::create_router(&config);

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind the cell");
            sender
                .send(listener.local_addr().expect("cell address"))
                .expect("report the cell address");
            axum::serve(listener, router).await.expect("serve the cell");
        });
    });
    let addr = receiver.recv().expect("cell address");

    Cell {
        base_url: format!("http://{addr}"),
        key_file: write_key_file(ADMIN_KEY),
        dir,
    }
}

fn write_key_file(key: &str) -> PathBuf {
    let path = scratch("key");
    // A trailing newline, as an editor or `echo` leaves it, must not become part of the key.
    std::fs::write(&path, format!("{key}\n")).expect("key file");
    path
}

fn event(label: &str, second: u32, cause: Option<EventId>, source: EventSource) -> Event {
    Event {
        tenant_id: TenantId::local(),
        event_id: None,
        event_uuid: Uuid::new_v4(),
        correlation_id: Some(format!("corr-{label}")),
        causation_event_id: cause,
        event_type: EventType::EvidenceRecorded,
        actor_id: format!("agent:claude:{label}"),
        source,
        source_ref: Some(format!("ref-{label}")),
        payload: json!({"evidence_id": label, "content": format!("content of {label}"), "source": "migrate-http"}),
        ts: Some(
            Utc.with_ymd_and_hms(2026, 10, 7, 8, 0, second)
                .single()
                .expect("fixed time"),
        ),
    }
}

/// Six events, four causation links: 2<-1, 3<-2, 5<-3, 6<-5. Event 4 has no cause.
fn source_ledger() -> PathBuf {
    let dir = scratch("source");
    let ledger = SqliteEventLedger::open(&dir).expect("source ledger");
    let causes = [None, Some(1), Some(2), None, Some(3), Some(5)];
    for (index, cause) in causes.into_iter().enumerate() {
        let source = if index % 2 == 0 {
            EventSource::Agent
        } else {
            EventSource::Human
        };
        ledger
            .append(event(
                &format!("source-{}", index + 1),
                index as u32 + 1,
                cause,
                source,
            ))
            .expect("append source event");
    }
    dir
}

/// Puts events into the cell before the move, so every moved event lands at a different number
/// than it had in the source.
fn fill_cell(cell: &Cell, count: usize) {
    let ledger = SqliteEventLedger::open(&cell.dir).expect("cell ledger");
    for index in 0..count {
        ledger
            .append(event(
                &format!("resident-{index}"),
                30 + index as u32,
                None,
                EventSource::Cli,
            ))
            .expect("append resident event");
    }
}

fn cell_events(cell: &Cell, after: EventId) -> Vec<Event> {
    SqliteEventLedger::open(&cell.dir)
        .expect("cell ledger")
        .read(after, 1000)
        .expect("read the cell")
}

fn cell_offset(cell: &Cell) -> EventId {
    SqliteEventLedger::open(&cell.dir)
        .expect("cell ledger")
        .latest_offset()
        .expect("cell offset")
}

fn migrate(
    source_dir: &Path,
    cell: &Cell,
    key_file: &Path,
    extra: &[&str],
) -> Result<String, String> {
    migrate_into(source_dir, cell, key_file, "local", extra)
}

fn migrate_into(
    source_dir: &Path,
    cell: &Cell,
    key_file: &Path,
    to_tenant: &str,
    extra: &[&str],
) -> Result<String, String> {
    let mut argv = vec![
        "hivemind".to_owned(),
        "--hivemind-dir".to_owned(),
        "/nonexistent-hivemind-dir".to_owned(),
        "migrate".to_owned(),
        "--from".to_owned(),
        source_dir.display().to_string(),
        "--to-url".to_owned(),
        cell.base_url.clone(),
        "--admin-key-file".to_owned(),
        key_file.display().to_string(),
        "--to-tenant".to_owned(),
        to_tenant.to_owned(),
    ];
    argv.extend(extra.iter().map(|arg| (*arg).to_owned()));
    let cli = Cli::parse_from(argv);
    run(&cli).map_err(|error| error.to_string())
}

fn migrate_json(source_dir: &Path, cell: &Cell, extra: &[&str]) -> Value {
    let mut args = vec!["--json"];
    args.extend_from_slice(extra);
    // `--json` is a global flag, so it may follow the subcommand's own flags.
    let output = migrate(source_dir, cell, &cell.key_file, &args).expect("migrate succeeds");
    serde_json::from_str(&output).expect("migrate prints JSON")
}

#[test]
fn a_ledger_moves_into_a_cell_that_already_holds_events_with_causation_renumbered() {
    let cell = start_cell();
    fill_cell(&cell, 4);
    let source_dir = source_ledger();

    let report = migrate_json(&source_dir, &cell, &[]);
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["destination"], cell.base_url.as_str());
    assert_eq!(report["source_event_count"], 6);
    assert_eq!(report["new_events"], 6);
    assert_eq!(report["already_present"], 0);
    assert_eq!(
        report["parity_check"],
        json!({"source_event_count": 6, "present_in_destination": 6, "missing": 0, "ok": true})
    );

    let source = SqliteEventLedger::open(&source_dir)
        .expect("source ledger")
        .read(0, 100)
        .expect("read source");
    let moved = cell_events(&cell, 4);
    assert_eq!(moved.len(), 6, "six events after the four residents");

    for (from, to) in source.iter().zip(&moved) {
        assert_eq!(to.event_uuid, from.event_uuid);
        assert_eq!(to.actor_id, from.actor_id);
        assert_eq!(to.source, from.source);
        assert_eq!(to.source_ref, from.source_ref);
        assert_eq!(to.correlation_id, from.correlation_id);
        assert_eq!(to.payload, from.payload);
        assert_eq!(
            to.ts, from.ts,
            "the original time, not the time of the move"
        );
    }

    // Source event n sat at id n; it now sits at id n+4. Each link must follow its cause.
    let new_id_of = |source_id: EventId| moved[(source_id - 1) as usize].event_id;
    for (index, from) in source.iter().enumerate() {
        let expected = from.causation_event_id.and_then(new_id_of);
        assert_eq!(
            moved[index].causation_event_id,
            expected,
            "causation of source event {}",
            index + 1
        );
    }
    assert_eq!(moved[1].causation_event_id, Some(5), "2 <- 1 is now 6 <- 5");
    assert_eq!(
        moved[5].causation_event_id,
        Some(9),
        "6 <- 5 is now 10 <- 9"
    );
    assert_eq!(cell_offset(&cell), 10);

    // A second run finds every uuid and writes nothing.
    let again = migrate_json(&source_dir, &cell, &[]);
    assert_eq!(again["new_events"], 0);
    assert_eq!(again["already_present"], 6);
    assert_eq!(again["parity_check"]["ok"], true);
    assert_eq!(cell_offset(&cell), 10, "the re-run wrote nothing");
}

#[test]
fn a_dry_run_says_what_would_move_and_writes_nothing() {
    let cell = start_cell();
    fill_cell(&cell, 2);
    let source_dir = source_ledger();

    let report = migrate_json(&source_dir, &cell, &["--dry-run"]);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["new_events"], 6);
    assert_eq!(report["already_present"], 0);
    assert!(
        report["parity_check"].is_null(),
        "a dry run has nothing to verify"
    );
    assert_eq!(cell_offset(&cell), 2, "nothing was written");

    let text = migrate(&source_dir, &cell, &cell.key_file, &["--dry-run"]).expect("dry run");
    assert!(
        text.starts_with("Dry run: 6 of 6 source events would move (0 already in the destination)"),
        "unexpected output: {text}"
    );

    // After the real move the dry run reports the other side: nothing left to move.
    migrate_json(&source_dir, &cell, &[]);
    let after = migrate_json(&source_dir, &cell, &["--dry-run"]);
    assert_eq!(after["new_events"], 0);
    assert_eq!(after["already_present"], 6);
}

#[test]
fn a_wrong_admin_key_moves_nothing() {
    let cell = start_cell();
    fill_cell(&cell, 1);
    let source_dir = source_ledger();
    let wrong = write_key_file("not-the-admin-key");

    let error = migrate(&source_dir, &cell, &wrong, &["--json"]).expect_err("must be refused");
    assert!(error.contains("401"), "{error}");
    assert!(error.contains("invalid admin key"), "{error}");
    assert_eq!(cell_offset(&cell), 1);
}

#[test]
fn an_unknown_destination_tenant_moves_nothing() {
    let cell = start_cell();
    let source_dir = source_ledger();

    let error = migrate_into(
        &source_dir,
        &cell,
        &cell.key_file,
        "tenant-nobody-registered",
        &["--json"],
    )
    .expect_err("an unregistered tenant must be refused");
    assert!(error.contains("404"), "{error}");
    assert_eq!(cell_offset(&cell), 0);
}

#[test]
fn a_source_whose_link_points_at_nothing_is_refused_before_anything_is_sent() {
    let cell = start_cell();
    let dir = scratch("dangling");
    let ledger = SqliteEventLedger::open(&dir).expect("source ledger");
    ledger
        .append(event("first", 1, None, EventSource::Agent))
        .expect("append");
    ledger
        .append(event("second", 2, Some(99), EventSource::Agent))
        .expect("append");

    let error = migrate(&dir, &cell, &cell.key_file, &["--json"]).expect_err("must be refused");
    assert!(
        error.contains("not an earlier event of the source ledger"),
        "{error}"
    );
    assert_eq!(cell_offset(&cell), 0, "not even the first event was sent");
}

#[test]
fn a_source_with_no_events_is_a_wrong_path_not_a_finished_move() {
    let cell = start_cell();
    let empty = scratch("empty");

    let error = migrate(&empty, &cell, &cell.key_file, &["--json"]).expect_err("must be refused");
    assert!(error.contains("nothing to migrate"), "{error}");
}

#[test]
fn the_destination_is_named_once_and_the_cell_needs_its_key_file() {
    let parse = |args: &[&str]| {
        let mut argv = vec!["hivemind", "migrate", "--to-tenant", "local"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).is_ok()
    };
    assert!(parse(&[
        "--to-url",
        "http://cell",
        "--admin-key-file",
        "key"
    ]));
    assert!(parse(&["--to", "postgres://u@h/db"]));
    assert!(!parse(&[]), "no destination");
    assert!(
        !parse(&["--to-url", "http://cell"]),
        "a cell needs its admin key file"
    );
    assert!(
        !parse(&[
            "--to",
            "postgres://u@h/db",
            "--to-url",
            "http://cell",
            "--admin-key-file",
            "key"
        ]),
        "two destinations"
    );
}

#[test]
fn an_admin_key_file_does_not_go_with_a_database_url() {
    let cli = Cli::parse_from([
        "hivemind",
        "migrate",
        "--to",
        "postgres://u@h/db",
        "--admin-key-file",
        "key",
        "--to-tenant",
        "local",
    ]);
    let error = run(&cli).expect_err("must be refused").to_string();
    assert!(error.contains("goes with --to-url"), "{error}");
}

fn post_replay(cell: &Cell, key: Option<&str>, body: &Value) -> (u16, Value) {
    let client = reqwest::blocking::Client::new();
    let mut request = client
        .post(format!("{}/v1/ledger/replay", cell.base_url))
        .json(body);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().expect("replay request");
    let status = response.status().as_u16();
    (status, response.json().unwrap_or(Value::Null))
}

fn wire_event(uuid: Uuid, cause: Option<Uuid>) -> Value {
    json!({
        "event_uuid": uuid,
        "type": "evidence.recorded",
        "actor_id": "agent:claude:wire",
        "source": "agent",
        "source_ref": "wire-ref",
        "causation_event_uuid": cause,
        "payload": {"evidence_id": "wire", "content": "wire", "source": "test"},
        "ts": "2026-10-07T08:05:56Z",
    })
}

#[test]
fn the_replay_route_wants_the_admin_key_a_known_tenant_and_a_well_formed_batch() {
    let cell = start_cell();
    let body = json!({"tenant_id": "local", "events": [wire_event(Uuid::new_v4(), None)]});

    let (status, _) = post_replay(&cell, None, &body);
    assert_eq!(status, 401, "no key");
    let (status, _) = post_replay(&cell, Some("nope"), &body);
    assert_eq!(status, 401, "wrong key");
    assert_eq!(cell_offset(&cell), 0);

    // An event id at the destination is the destination's to assign.
    let mut with_id = wire_event(Uuid::new_v4(), None);
    with_id["event_id"] = json!(7);
    let (status, _) = post_replay(
        &cell,
        Some(ADMIN_KEY),
        &json!({"tenant_id": "local", "events": [with_id]}),
    );
    assert_eq!(status, 400, "event_id is not part of the wire form");

    // A cause that is nowhere refuses the whole batch, the good event in it included.
    let good = wire_event(Uuid::new_v4(), None);
    let orphan = wire_event(Uuid::new_v4(), Some(Uuid::new_v4()));
    let (status, answer) = post_replay(
        &cell,
        Some(ADMIN_KEY),
        &json!({"tenant_id": "local", "events": [good, orphan]}),
    );
    assert_eq!(status, 400);
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("neither in the destination nor earlier"),
        "{answer}"
    );
    assert_eq!(cell_offset(&cell), 0, "the refused batch wrote nothing");

    // The well-formed batch lands, and its answer says what it did.
    let first = Uuid::new_v4();
    let (status, answer) = post_replay(
        &cell,
        Some(ADMIN_KEY),
        &json!({"tenant_id": "local", "events": [wire_event(first, None), wire_event(Uuid::new_v4(), Some(first))]}),
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["received"], 2);
    assert_eq!(answer["new_events"], 2);
    assert_eq!(answer["already_present"], 0);
    assert_eq!(cell_events(&cell, 0)[1].causation_event_id, Some(1));
}
