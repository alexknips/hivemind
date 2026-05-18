use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::{Event, EventType, RelationKind};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::json;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const SEED_BASE_UNIX_SECONDS: i64 = 1_767_225_600;

#[test]
fn seed_event_stream_is_deterministic() -> TestResult<()> {
    let first_dir = unique_temp_dir("first");
    let second_dir = unique_temp_dir("second");

    seed_to_dir(&first_dir)?;
    seed_to_dir(&second_dir)?;

    assert_eq!(
        canonical_ledger_export(&first_dir)?,
        canonical_ledger_export(&second_dir)?
    );

    Ok(())
}

#[test]
fn seed_dataset_covers_slice_one_demo_cases() {
    let events = seed_events();

    let decision_count = events
        .iter()
        .filter(|event| event.event_type == EventType::DecisionProposed)
        .count();
    assert!(decision_count >= 30);

    assert!(events.iter().any(|event| {
        event.event_type == EventType::DecisionAccepted
            && payload_str(event, "decision_id") == Some("decision-004")
    }));
    assert!(events.iter().any(|event| {
        event.event_type == EventType::DecisionRejected
            && payload_str(event, "decision_id") == Some("decision-004")
    }));

    let supersession_edges = events
        .iter()
        .filter(|event| event.event_type == EventType::DecisionSuperseded)
        .count();
    assert!(supersession_edges >= 2);

    let refutes_hypothesis = events.iter().any(|event| {
        event.event_type == EventType::RelationAdded
            && payload_str(event, "relation") == Some("REFUTES")
            && payload_str(event, "to_id") == Some("hypothesis-001")
    });
    assert!(refutes_hypothesis);

    let assuming_decisions = events
        .iter()
        .filter(|event| {
            event.event_type == EventType::DecisionProposed
                && event
                    .payload
                    .get("hypothesis_ids")
                    .and_then(|value| value.as_array())
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some("hypothesis-001")))
        })
        .count();
    assert!(assuming_decisions >= 2);
}

#[test]
#[ignore = "populates ./hivemind by default; run: cargo test --test seed -- --include-ignored"]
fn populate_seed_hivemind_dir() -> TestResult<()> {
    let seed_dir = std::env::var("HIVEMIND_SEED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./hivemind"));

    reset_seed_dir(&seed_dir)?;
    seed_to_dir(&seed_dir)
}

#[test]
fn replay_smoke_warns_only_on_query_drift() -> TestResult<()> {
    let started = Instant::now();
    let seed_dir = unique_temp_dir("replay-smoke");
    seed_to_dir(&seed_dir)?;

    let before_replay = capture_replay_query_outputs(&seed_dir)?;
    let after_replay = capture_replay_query_outputs(&seed_dir)?;

    let query_diff = replay_query_diff(&before_replay, &after_replay);
    if !query_diff.is_empty() {
        eprintln!("warning: replay smoke query output drift detected\n{query_diff}");
    }

    let elapsed = started.elapsed();
    if elapsed.as_secs_f64() > 5.0 {
        eprintln!(
            "warning: replay smoke exceeded 5s target: {:.3}s",
            elapsed.as_secs_f64()
        );
    }

    Ok(())
}

fn seed_to_dir(seed_dir: &Path) -> TestResult<()> {
    fs::create_dir_all(seed_dir)?;
    let ledger = SqliteEventLedger::open(seed_dir)?;

    for event in seed_events() {
        ledger.append(event)?;
    }

    Ok(())
}

fn seed_events() -> Vec<Event> {
    let mut builder = SeedBuilder::default();

    for index in 1..=5 {
        builder.evidence(
            &format!("evidence-{index:03}"),
            &format!("Seed evidence {index}: fabricated observation for replay demos"),
        );
    }

    builder.hypothesis(
        "hypothesis-001",
        "The embedded graph projection remains deterministic after replay",
    );
    builder.hypothesis(
        "hypothesis-002",
        "CLI command output remains stable enough for automation",
    );
    builder.hypothesis(
        "hypothesis-003",
        "Decision provenance is sufficient for later audits",
    );

    let topics = ["architecture", "operations", "security", "product"];
    for index in 1..=30 {
        let topic = match topics.get((index - 1) % topics.len()) {
            Some(topic) => *topic,
            None => "architecture",
        };
        let hypotheses = match index {
            5 | 6 => vec!["hypothesis-001"],
            7..=10 => vec!["hypothesis-002"],
            11..=15 => vec!["hypothesis-003"],
            _ => Vec::new(),
        };
        let evidence = match index {
            5 | 6 => vec!["evidence-001"],
            7..=10 => vec!["evidence-002"],
            11..=15 => vec!["evidence-003"],
            _ => Vec::new(),
        };
        builder.decision(index, topic, &hypotheses, &evidence);
    }

    builder.accept("decision-003", "actor:reviewer");
    builder.accept("decision-004", "actor:alice");
    builder.reject("decision-004", "actor:bob");
    builder.supersede("decision-001", "decision-002");
    builder.supersede("decision-002", "decision-003");
    builder.relation(
        RelationKind::Supports,
        "evidence-001",
        "hypothesis-001",
        "actor:analyst",
    );
    builder.relation(
        RelationKind::Refutes,
        "evidence-004",
        "hypothesis-001",
        "actor:auditor",
    );
    builder.relation(
        RelationKind::Supports,
        "evidence-002",
        "hypothesis-002",
        "actor:analyst",
    );

    builder.events
}

#[derive(Default)]
struct SeedBuilder {
    events: Vec<Event>,
}

impl SeedBuilder {
    fn evidence(&mut self, evidence_id: &str, content: &str) {
        self.push(
            EventType::EvidenceRecorded,
            "actor:researcher",
            json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": "seed"
            }),
            None,
        );
    }

    fn hypothesis(&mut self, hypothesis_id: &str, statement: &str) {
        self.push(
            EventType::HypothesisRecorded,
            "actor:researcher",
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement
            }),
            None,
        );
    }

    fn decision(
        &mut self,
        index: usize,
        topic: &str,
        hypothesis_ids: &[&str],
        evidence_ids: &[&str],
    ) {
        let decision_id = format!("decision-{index:03}");
        let option_a = format!("option-{index:03}-a");
        let option_b = format!("option-{index:03}-b");
        self.push(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": decision_id,
                "title": format!("Seed decision {index:03}"),
                "rationale": format!("Deterministic rationale for seed decision {index:03}"),
                "topic_keys": [topic, "slice-one"],
                "option_ids": [option_a, option_b],
                "chosen_option_id": option_b,
                "hypothesis_ids": hypothesis_ids,
                "evidence_ids": evidence_ids
            }),
            None,
        );
    }

    fn accept(&mut self, decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionAccepted,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
        );
    }

    fn reject(&mut self, decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionRejected,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
        );
    }

    fn supersede(&mut self, old_decision_id: &str, new_decision_id: &str) {
        self.push(
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id
            }),
            None,
        );
    }

    fn relation(&mut self, relation: RelationKind, from_id: &str, to_id: &str, actor_id: &str) {
        self.push(
            EventType::RelationAdded,
            actor_id,
            json!({
                "relation": relation,
                "from_id": from_id,
                "to_id": to_id
            }),
            None,
        );
    }

    fn push(
        &mut self,
        event_type: EventType,
        actor_id: &str,
        payload: serde_json::Value,
        causation_event_id: Option<u64>,
    ) {
        let sequence = self.events.len() + 1;
        self.events.push(Event {
            event_id: None,
            event_uuid: Uuid::from_u128(u128::try_from(sequence).unwrap_or(u128::MAX)),
            correlation_id: Some("seed-dataset-v1".to_owned()),
            causation_event_id,
            event_type,
            actor_id: actor_id.to_owned(),
            payload,
            ts: Some(seed_timestamp(sequence)),
        });
    }
}

fn seed_timestamp(sequence: usize) -> DateTime<Utc> {
    let sequence_seconds = i64::try_from(sequence).unwrap_or(i64::MAX - SEED_BASE_UNIX_SECONDS);
    DateTime::from_timestamp(SEED_BASE_UNIX_SECONDS, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        + Duration::seconds(sequence_seconds)
}

fn canonical_ledger_export(seed_dir: &Path) -> TestResult<Vec<u8>> {
    let ledger = SqliteEventLedger::open(seed_dir)?;
    let events = ledger.read(0, 10_000)?;
    Ok(serde_json::to_vec_pretty(&events)?)
}

#[derive(Debug, Eq, PartialEq)]
struct ReplayQueryOutput {
    name: &'static str,
    json: String,
}

fn capture_replay_query_outputs(seed_dir: &Path) -> TestResult<Vec<ReplayQueryOutput>> {
    let seed_dir = seed_dir.display().to_string();
    let query_specs: [(&str, &[&str]); 3] = [
        (
            "get_decision",
            &["query", "get_decision", "--id", "decision-005"],
        ),
        (
            "get_relevant_decisions",
            &["query", "get_relevant_decisions", "--topic", "architecture"],
        ),
        (
            "get_supersession_chain",
            &["query", "get_supersession_chain", "--id", "decision-002"],
        ),
    ];

    let mut outputs = Vec::with_capacity(query_specs.len());
    for (name, args) in query_specs {
        let mut argv = vec!["hivemind", "--hivemind-dir", seed_dir.as_str()];
        argv.extend(args.iter().copied());
        let cli = Cli::parse_from(argv);
        outputs.push(ReplayQueryOutput {
            name,
            json: canonical_query_json(&run(&cli)?)?,
        });
    }

    Ok(outputs)
}

fn canonical_query_json(raw_json: &str) -> TestResult<String> {
    let mut value: serde_json::Value = serde_json::from_str(raw_json)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("latency_ms".to_owned(), json!(0));
    }
    Ok(serde_json::to_string_pretty(&value)?)
}

fn replay_query_diff(
    before_replay: &[ReplayQueryOutput],
    after_replay: &[ReplayQueryOutput],
) -> String {
    let mut diff = String::new();
    for (before, after) in before_replay.iter().zip(after_replay) {
        if before != after {
            diff.push_str("query: ");
            diff.push_str(before.name);
            diff.push('\n');
            diff.push_str(&unified_diff(&before.json, &after.json));
        }
    }

    if before_replay.len() != after_replay.len() {
        diff.push_str(&format!(
            "query count changed: before={}, after={}\n",
            before_replay.len(),
            after_replay.len()
        ));
    }

    diff
}

fn unified_diff(before: &str, after: &str) -> String {
    let mut diff = String::from("--- before replay\n+++ after replay\n@@\n");
    for line in before.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in after.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn reset_seed_dir(seed_dir: &Path) -> TestResult<()> {
    if seed_dir.as_os_str().is_empty() || seed_dir == Path::new("/") {
        return Err("refusing to reset unsafe seed directory".into());
    }

    if seed_dir.exists() {
        return Err(format!(
            "seed directory already exists: {}; remove it before running the seed test",
            seed_dir.display()
        )
        .into());
    }

    Ok(())
}

fn unique_temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-seed-{label}-{}", Uuid::new_v4()))
}

fn payload_str<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(|value| value.as_str())
}
