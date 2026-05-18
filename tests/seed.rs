use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::EventType;
use serde_json::json;

#[path = "support/seed_data.rs"]
mod seed_data;

use seed_data::{
    canonical_ledger_export, payload_str, reset_seed_dir, seed_events, seed_to_dir,
    unique_temp_dir, TestResult,
};

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
