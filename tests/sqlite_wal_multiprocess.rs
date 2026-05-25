use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hivemind::commands::Commands;
use hivemind::events::EventType;
use hivemind::ledger::{EventLedger, SqliteEventLedger};

const WORKER_COUNT: usize = 2;
const DECISIONS_PER_WORKER: usize = 1_000;
const EVENTS_PER_DECISION: usize = 2;
const WORKER_ENV: &str = "HIVEMIND_SQLITE_WAL_WORKER";
const LEDGER_DIR_ENV: &str = "HIVEMIND_SQLITE_WAL_DIR";
const READY_DIR_ENV: &str = "HIVEMIND_SQLITE_WAL_READY_DIR";
const START_FILE_ENV: &str = "HIVEMIND_SQLITE_WAL_START_FILE";
const WORKER_INDEX_ENV: &str = "HIVEMIND_SQLITE_WAL_WORKER_INDEX";

#[test]
fn shared_sqlite_ledger_accepts_concurrent_process_writes() {
    let temp_dir = temp_hivemind_dir("sqlite-wal-multiprocess");
    let result = run_shared_sqlite_ledger_test(&temp_dir);
    let _ = fs::remove_dir_all(&temp_dir);
    result.expect("shared SQLite WAL write test succeeds");
}

#[test]
#[ignore = "helper process spawned by shared_sqlite_ledger_accepts_concurrent_process_writes"]
fn sqlite_wal_worker() {
    if env::var_os(WORKER_ENV).is_none() {
        return;
    }

    run_worker().expect("worker writes decisions");
}

fn run_shared_sqlite_ledger_test(
    temp_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ledger_dir = temp_dir.join("hivemind");
    let ready_dir = temp_dir.join("ready");
    let start_file = temp_dir.join("start");
    fs::create_dir_all(&ready_dir)?;

    let mut workers = Vec::new();
    for worker_index in 0..WORKER_COUNT {
        workers.push((
            worker_index,
            spawn_worker(worker_index, &ledger_dir, &ready_dir, &start_file)?,
        ));
    }

    wait_for(
        "workers to reach the start barrier",
        Duration::from_secs(10),
        || (0..WORKER_COUNT).all(|index| ready_dir.join(format!("{index}.ready")).exists()),
    )?;
    fs::write(&start_file, "start")?;

    for (worker_index, worker) in workers {
        let output = worker.wait_with_output()?;
        assert!(
            output.status.success(),
            "worker {worker_index} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let ledger = SqliteEventLedger::open(&ledger_dir)?;
    let expected_events = WORKER_COUNT * DECISIONS_PER_WORKER * EVENTS_PER_DECISION;
    assert_eq!(ledger.latest_offset()?, expected_events as u64);

    let events = ledger.read(0, expected_events + 1)?;
    assert_eq!(events.len(), expected_events);

    let mut seen_event_ids = BTreeSet::new();
    let mut decision_event_ids = BTreeSet::new();
    let mut decision_count = 0;
    let mut relation_count = 0;

    for (index, event) in events.iter().enumerate() {
        let event_id = event.event_id.expect("stored event has event_id");
        assert!(
            seen_event_ids.insert(event_id),
            "duplicate event_id {event_id}"
        );
        assert_eq!(event_id, (index + 1) as u64);

        match event.event_type {
            EventType::DecisionProposed => {
                decision_count += 1;
                decision_event_ids.insert(event_id);
            }
            EventType::RelationAdded => {
                relation_count += 1;
            }
            other => {
                return Err(
                    format!("unexpected event type in reproduction ledger: {other:?}").into(),
                )
            }
        }
    }

    assert_eq!(decision_count, WORKER_COUNT * DECISIONS_PER_WORKER);
    assert_eq!(relation_count, WORKER_COUNT * DECISIONS_PER_WORKER);

    for event in events
        .iter()
        .filter(|event| event.event_type == EventType::RelationAdded)
    {
        let relation_event_id = event.event_id.expect("stored event has event_id");
        let root_event_id = event
            .causation_event_id
            .expect("proposal relation records causation_event_id");
        assert!(
            root_event_id < relation_event_id,
            "relation {relation_event_id} must follow root {root_event_id}"
        );
        assert!(
            decision_event_ids.contains(&root_event_id),
            "relation {relation_event_id} points to missing decision event {root_event_id}"
        );
    }

    let mut replayed_event_ids = Vec::with_capacity(expected_events);
    ledger.replay_from(0, &mut |event| {
        replayed_event_ids.push(event.event_id.expect("replayed event has event_id"));
        Ok(())
    })?;
    assert_eq!(
        replayed_event_ids,
        (1..=expected_events as u64).collect::<Vec<_>>()
    );

    Ok(())
}

fn spawn_worker(
    worker_index: usize,
    ledger_dir: &Path,
    ready_dir: &Path,
    start_file: &Path,
) -> Result<Child, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Command::new(env::current_exe()?)
        .arg("--ignored")
        .arg("--exact")
        .arg("sqlite_wal_worker")
        .arg("--nocapture")
        .env(WORKER_ENV, "1")
        .env(LEDGER_DIR_ENV, ledger_dir)
        .env(READY_DIR_ENV, ready_dir)
        .env(START_FILE_ENV, start_file)
        .env(WORKER_INDEX_ENV, worker_index.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}

fn run_worker() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ledger_dir = PathBuf::from(env::var(LEDGER_DIR_ENV)?);
    let ready_dir = PathBuf::from(env::var(READY_DIR_ENV)?);
    let start_file = PathBuf::from(env::var(START_FILE_ENV)?);
    let worker_index: usize = env::var(WORKER_INDEX_ENV)?.parse()?;

    fs::write(ready_dir.join(format!("{worker_index}.ready")), "ready")?;
    wait_for("parent start signal", Duration::from_secs(10), || {
        start_file.exists()
    })?;

    let actor_id = format!("agent:test-wal:{worker_index}");
    let ledger = SqliteEventLedger::open(&ledger_dir)?;
    let commands = Commands::new(&ledger);

    for decision_index in 0..DECISIONS_PER_WORKER {
        let option_label = format!("worker-{worker_index}-option-{decision_index}");
        let option_description = format!("Option for concurrent WAL decision {decision_index}");
        let option_id = commands.record_option(&actor_id, &option_label, &option_description)?;
        let topic_key = format!("shared-ledger-worker-{worker_index}");
        let title = format!("Shared ledger write {worker_index}/{decision_index}");

        commands.propose_decision(
            &actor_id,
            &title,
            "Exercise concurrent writes from independent HiveMind processes.",
            &[topic_key],
            &[option_id],
            None,
            &[],
            &[],
        )?;
    }

    Ok(())
}

fn wait_for(
    description: &str,
    timeout: Duration,
    mut predicate: impl FnMut() -> bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    Err(format!("timed out waiting for {description}").into())
}

fn temp_hivemind_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("hivemind-{prefix}-{nanos}-{}", std::process::id()))
}
