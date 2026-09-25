use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::ledger::{EventLedger, InMemoryEventLedger};
use hivemind::projector::{memory::MemoryGraph, rebuild_graph};
use hivemind::queries::{export_decision_log, DecisionLogOutcome, DecisionLogRequest};
use serde_json::json;

#[allow(dead_code)]
#[path = "support/seed_data.rs"]
mod seed_data;

#[allow(dead_code)]
#[path = "support/organizational_scenarios.rs"]
mod organizational_scenarios;

#[allow(dead_code)]
#[path = "support/project_layout.rs"]
mod project_layout;

use seed_data::{seed_to_dir, unique_temp_dir, TestResult};

const SNAPSHOT_DIR: &str = "tests/snapshots/golden";
const EXPORT_SNAPSHOT_DIR: &str = "tests/snapshots/golden/export_markdown";

const QUERY_SPECS: &[QuerySpec] = &[
    QuerySpec {
        name: "get_decision",
        snapshot_file: "get_decision.json",
        args: &["query", "get_decision", "--id", "decision-005"],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "get_relevant_decisions",
        snapshot_file: "get_relevant_decisions.json",
        args: &["query", "get_relevant_decisions", "--topic", "architecture"],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "get_supersession_chain",
        snapshot_file: "get_supersession_chain.json",
        args: &["query", "get_supersession_chain", "--id", "decision-002"],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_actor_filter",
        snapshot_file: "search_actor_filter.json",
        args: &[
            "query",
            "search_decisions",
            "--actor-id",
            "actor:bob",
            "--status",
            "contested",
            "--limit",
            "10",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_evidence_text",
        snapshot_file: "search_evidence_text.json",
        args: &[
            "query",
            "search_decisions",
            "--q",
            "packet-capture",
            "--limit",
            "5",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_option_text",
        snapshot_file: "search_option_text.json",
        args: &[
            "query",
            "search_decisions",
            "--q",
            "delta-mirror",
            "--limit",
            "5",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_hypothesis_text",
        snapshot_file: "search_hypothesis_text.json",
        args: &[
            "query",
            "search_decisions",
            "--q",
            "hypothesis text needle",
            "--limit",
            "5",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_empty_results",
        snapshot_file: "search_empty_results.json",
        args: &[
            "query",
            "search_decisions",
            "--q",
            "no-such-search-needle",
            "--topic",
            "architecture",
            "--limit",
            "5",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_empty_page",
        snapshot_file: "search_empty_page.json",
        args: &[
            "query",
            "search_decisions",
            "--topic",
            "slice-one",
            "--limit",
            "5",
            "--cursor",
            "30",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "search_truncated",
        snapshot_file: "search_truncated.json",
        args: &[
            "query",
            "search_decisions",
            "--topic",
            "slice-one",
            "--limit",
            "2",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "recent_decisions",
        snapshot_file: "recent_decisions.json",
        args: &[
            "query",
            "recent_decisions",
            "--since",
            "2026-01-01",
            "--until",
            "2026-01-01T00:00:40Z",
            "--topic",
            "architecture",
            "--status",
            "superseded",
            "--limit",
            "5",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "recent_decisions_empty",
        snapshot_file: "recent_decisions_empty.json",
        args: &[
            "query",
            "recent_decisions",
            "--since",
            "2030-01-01",
            "--limit",
            "5",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "get_decision_neighborhood_refuted",
        snapshot_file: "get_decision_neighborhood_refuted.json",
        args: &["query", "get_decision_neighborhood", "--id", "decision-005"],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "get_decision_neighborhood_branchy_supersession",
        snapshot_file: "get_decision_neighborhood_branchy_supersession.json",
        args: &["query", "get_decision_neighborhood", "--id", "decision-016"],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "get_decision_neighborhood_missing",
        snapshot_file: "get_decision_neighborhood_missing.json",
        args: &[
            "query",
            "get_decision_neighborhood",
            "--id",
            "decision-missing",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "get_decision_neighborhood_invalid_id",
        snapshot_file: "get_decision_neighborhood_invalid_id.json",
        args: &["query", "get_decision_neighborhood", "--id", ""],
        expectation: QueryExpectation::Error,
    },
    QuerySpec {
        name: "get_supersession_chain_branch_error",
        snapshot_file: "get_supersession_chain_branch_error.json",
        args: &["query", "get_supersession_chain", "--id", "decision-016"],
        expectation: QueryExpectation::Error,
    },
    // The seed predates projects, so every decision projects to its recorder's personal
    // project (`actor:planner` -> `personal:actor:planner`).
    QuerySpec {
        name: "project_decisions_personal_truncated",
        snapshot_file: "project_decisions_personal_truncated.json",
        args: &[
            "--json",
            "project",
            "decisions",
            "personal:actor:planner",
            "--limit",
            "2",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "project_decisions_personal_last_page",
        snapshot_file: "project_decisions_personal_last_page.json",
        args: &[
            "--json",
            "project",
            "decisions",
            "personal:actor:planner",
            "--limit",
            "2",
            "--cursor",
            "28",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "project_decisions_not_found",
        snapshot_file: "project_decisions_not_found.json",
        args: &["--json", "project", "decisions", "no-such-project"],
        expectation: QueryExpectation::Success,
    },
];

// `move` writes, so its specs run after every read spec above, in order, against the same seed
// ledger: MOVE_SETUP first (not snapshotted), then each spec. The seed's decisions were recorded
// by `actor:planner`, so they start in `personal:actor:planner`.
const MOVE_SETUP: &[&[&str]] = &[&["--actor", "actor:planner", "project", "register", "pricing"]];

const MOVE_SPECS: &[QuerySpec] = &[
    QuerySpec {
        name: "move_not_found",
        snapshot_file: "move_not_found.json",
        args: &[
            "--actor",
            "actor:planner",
            "--json",
            "move",
            "no-such-decision-needle",
            "--to",
            "pricing",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "move_by_id",
        snapshot_file: "move_by_id.json",
        args: &[
            "--actor",
            "actor:planner",
            "--json",
            "move",
            "--decision",
            "decision-005",
            "--to",
            "pricing",
            "--reason",
            "Pricing owns this decision",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "move_by_id_reversal",
        snapshot_file: "move_by_id_reversal.json",
        args: &[
            "--actor",
            "actor:planner",
            "--json",
            "move",
            "--decision",
            "decision-005",
            "--to",
            "personal:actor:planner",
        ],
        expectation: QueryExpectation::Success,
    },
    QuerySpec {
        name: "move_pick",
        snapshot_file: "move_pick.json",
        args: &[
            "--actor",
            "actor:planner",
            "--json",
            "move",
            "Seed decision",
            "--pick",
            "1",
            "--to",
            "pricing",
        ],
        expectation: QueryExpectation::Success,
    },
];

fn main() {
    if let Err(error) = run_harness() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_harness() -> TestResult<()> {
    let bless = parse_args()?;
    let scratch_root = unique_temp_dir("golden");
    let seed_dir = scratch_root.join("hivemind");

    if scratch_root.exists() {
        fs::remove_dir_all(&scratch_root)?;
    }
    fs::create_dir_all(&scratch_root)?;
    seed_to_dir(&seed_dir)?;

    let actual_outputs = capture_query_outputs(&seed_dir)?;
    let query_result = if bless {
        bless_snapshots(&actual_outputs)
    } else {
        compare_snapshots(&actual_outputs)
    };

    let export_outputs = capture_export_outputs()?;
    let export_result = if bless {
        bless_export_outputs(&export_outputs)
    } else {
        compare_export_outputs(&export_outputs)
    };

    let cleanup_result = fs::remove_dir_all(&scratch_root);
    query_result?;
    export_result?;
    cleanup_result?;
    Ok(())
}

fn parse_args() -> TestResult<bool> {
    let mut bless = false;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bless" => bless = true,
            "--nocapture" | "--include-ignored" | "--ignored" | "--exact" | "--quiet"
            | "--show-output" => {}
            "--test-threads" | "--skip" | "--format" => {
                let _ = args.next();
            }
            "--help" | "-h" => {
                println!("Usage: cargo test --test golden -- [--bless]");
                std::process::exit(0);
            }
            other if other.starts_with("--test-threads=") => {}
            other if other.starts_with("--skip=") => {}
            other if other.starts_with("--format=") => {}
            other if !other.starts_with('-') => {}
            other => return Err(format!("unknown golden test argument: {other}").into()),
        }
    }
    Ok(bless)
}

fn capture_query_outputs(seed_dir: &Path) -> TestResult<Vec<QueryOutput>> {
    let seed_dir = seed_dir.display().to_string();
    let mut outputs = Vec::with_capacity(QUERY_SPECS.len() + MOVE_SPECS.len());

    for (index, spec) in QUERY_SPECS.iter().chain(MOVE_SPECS).enumerate() {
        if index == QUERY_SPECS.len() {
            for setup in MOVE_SETUP {
                let mut argv = vec!["hivemind", "--hivemind-dir", seed_dir.as_str()];
                argv.extend(setup.iter().copied());
                run(&Cli::parse_from(argv))
                    .map_err(|error| format!("golden setup {setup:?} failed: {error}"))?;
            }
        }
        let mut argv = vec!["hivemind", "--hivemind-dir", seed_dir.as_str()];
        argv.extend(spec.args.iter().copied());
        let cli = Cli::parse_from(argv);
        let json = match (spec.expectation, run(&cli)) {
            (QueryExpectation::Success, Ok(raw_json)) => canonical_query_json(&raw_json)?,
            (QueryExpectation::Success, Err(error)) => {
                return Err(format!("{} failed unexpectedly: {error}", spec.name).into());
            }
            (QueryExpectation::Error, Ok(raw_json)) => {
                return Err(format!(
                    "{} succeeded unexpectedly with output: {raw_json}",
                    spec.name
                )
                .into());
            }
            (QueryExpectation::Error, Err(error)) => canonical_error_json(&error.to_string())?,
        };
        outputs.push(QueryOutput { spec, json });
    }

    Ok(outputs)
}

fn canonical_query_json(raw_json: &str) -> TestResult<String> {
    let mut value: serde_json::Value = serde_json::from_str(raw_json)?;
    if let Some(object) = value.as_object_mut() {
        // Only where the reply has one: a write's reply carries no latency to normalise.
        if object.contains_key("latency_ms") {
            object.insert("latency_ms".to_owned(), json!(0));
        }
    }
    Ok(serde_json::to_string_pretty(&value)?)
}

fn canonical_error_json(error: &str) -> TestResult<String> {
    Ok(serde_json::to_string_pretty(&json!({ "error": error }))?)
}

fn bless_snapshots(outputs: &[QueryOutput]) -> TestResult<()> {
    fs::create_dir_all(SNAPSHOT_DIR)?;
    for output in outputs {
        fs::write(output.snapshot_path(), format!("{}\n", output.json))?;
    }
    println!("blessed {} golden query snapshots", outputs.len());
    Ok(())
}

fn compare_snapshots(outputs: &[QueryOutput]) -> TestResult<()> {
    let mut failures = String::new();

    for output in outputs {
        let snapshot_path = output.snapshot_path();
        let expected = match fs::read_to_string(&snapshot_path) {
            Ok(expected) => expected,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                failures.push_str(&format!(
                    "missing snapshot for {}: {}\n",
                    output.spec.name,
                    snapshot_path.display()
                ));
                continue;
            }
            Err(error) => return Err(error.into()),
        };

        if expected.trim_end() != output.json {
            failures.push_str(&unified_diff(
                &snapshot_path.display().to_string(),
                output.spec.name,
                expected.trim_end(),
                &output.json,
            ));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "golden query snapshot mismatch\n{failures}\nRegenerate with: cargo test --test golden -- --bless"
        )
        .into())
    }
}

struct ExportOutput {
    label: &'static str,
    files: std::collections::BTreeMap<String, String>,
}

fn capture_export_outputs() -> TestResult<Vec<ExportOutput>> {
    let mut outputs = Vec::new();
    for (label, events) in [
        ("seed", seed_data::seed_events()),
        (
            "organizational_scenarios",
            organizational_scenarios::scenario_events(),
        ),
        ("project_layout", project_layout::project_layout_events()),
    ] {
        let ledger = InMemoryEventLedger::new();
        for event in events {
            ledger.append(event)?;
        }
        let graph = MemoryGraph::default();
        rebuild_graph(&ledger, &graph)?;
        let DecisionLogOutcome::Exported(export) =
            export_decision_log(&graph, &ledger, &DecisionLogRequest::default())?
        else {
            return Err(
                format!("{label}: an unfiltered export cannot be project_not_found").into(),
            );
        };
        outputs.push(ExportOutput {
            label,
            files: export.files,
        });
    }
    Ok(outputs)
}

fn bless_export_outputs(outputs: &[ExportOutput]) -> TestResult<()> {
    for output in outputs {
        let dir = Path::new(EXPORT_SNAPSHOT_DIR).join(output.label);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        for (relative_path, content) in &output.files {
            let full_path = dir.join(relative_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(full_path, content)?;
        }
    }
    println!(
        "blessed {} export_decision_log golden tree(s)",
        outputs.len()
    );
    Ok(())
}

fn compare_export_outputs(outputs: &[ExportOutput]) -> TestResult<()> {
    let mut failures = String::new();

    for output in outputs {
        let dir = Path::new(EXPORT_SNAPSHOT_DIR).join(output.label);
        let mut expected_paths = BTreeSet::new();
        collect_relative_files(&dir, &dir, &mut expected_paths)?;
        let actual_paths: BTreeSet<String> = output.files.keys().cloned().collect();
        if expected_paths != actual_paths {
            failures.push_str(&format!(
                "{}: exported file set differs from snapshot.\n  only in snapshot: {:?}\n  only in export: {:?}\n",
                output.label,
                expected_paths.difference(&actual_paths).collect::<Vec<_>>(),
                actual_paths.difference(&expected_paths).collect::<Vec<_>>(),
            ));
        }

        for (relative_path, content) in &output.files {
            let full_path = dir.join(relative_path);
            match fs::read_to_string(&full_path) {
                Ok(expected) if expected == *content => {}
                Ok(expected) => {
                    failures.push_str(&unified_diff(
                        &full_path.display().to_string(),
                        &format!("{}/{relative_path}", output.label),
                        &expected,
                        content,
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    failures.push_str(&format!("missing snapshot: {}\n", full_path.display()));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "export_decision_log golden mismatch\n{failures}\nRegenerate with: cargo test --test golden -- --bless"
        )
        .into())
    }
}

fn collect_relative_files(
    root: &Path,
    current: &Path,
    out: &mut BTreeSet<String>,
) -> TestResult<()> {
    if !current.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files(root, &path, out)?;
        } else {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(relative);
        }
    }
    Ok(())
}

fn unified_diff(expected_label: &str, actual_label: &str, expected: &str, actual: &str) -> String {
    let mut diff = format!("--- {expected_label}\n+++ actual/{actual_label}\n@@\n");
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let max_len = expected_lines.len().max(actual_lines.len());

    for index in 0..max_len {
        match (expected_lines.get(index), actual_lines.get(index)) {
            (Some(expected), Some(actual)) if expected == actual => {
                diff.push(' ');
                diff.push_str(expected);
                diff.push('\n');
            }
            (Some(expected), Some(actual)) => {
                diff.push('-');
                diff.push_str(expected);
                diff.push('\n');
                diff.push('+');
                diff.push_str(actual);
                diff.push('\n');
            }
            (Some(expected), None) => {
                diff.push('-');
                diff.push_str(expected);
                diff.push('\n');
            }
            (None, Some(actual)) => {
                diff.push('+');
                diff.push_str(actual);
                diff.push('\n');
            }
            (None, None) => {}
        }
    }

    diff
}

#[derive(Clone, Copy)]
struct QuerySpec {
    name: &'static str,
    snapshot_file: &'static str,
    args: &'static [&'static str],
    expectation: QueryExpectation,
}

#[derive(Clone, Copy)]
enum QueryExpectation {
    Success,
    Error,
}

struct QueryOutput {
    spec: &'static QuerySpec,
    json: String,
}

impl QueryOutput {
    fn snapshot_path(&self) -> PathBuf {
        Path::new(SNAPSHOT_DIR).join(self.spec.snapshot_file)
    }
}
