use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use hivemind::cli::{run, Cli};
use serde_json::{json, Value};
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn local_slack_and_agent_capture_share_one_query_path() -> TestResult<()> {
    let scratch = TempDir::new("local-capture-demo")?;
    let hivemind_dir = scratch.path().join("hivemind");
    let slack_fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/slack/thread_with_mention.json");

    let slack_decision_id = output_value(run_cli_json(
        &hivemind_dir,
        vec![
            "ingest".to_owned(),
            "slack-thread".to_owned(),
            "--file".to_owned(),
            slack_fixture.display().to_string(),
        ],
    )?)?;

    let codex_decision_id = capture_agent_decision(
        &hivemind_dir,
        "codex",
        "demo-codex",
        "Use local capture smoke tests",
        "The local CLI verifies agent capture without hosted services",
    )?;
    let claude_decision_id = capture_agent_decision(
        &hivemind_dir,
        "claude",
        "demo-claude",
        "Keep the prototype local before Slack hosting",
        "The demo should validate provenance before adding real Slack credentials",
    )?;

    let search = run_cli_json(
        &hivemind_dir,
        vec![
            "query".to_owned(),
            "search_decisions".to_owned(),
            "--topic".to_owned(),
            "integrations".to_owned(),
            "--status".to_owned(),
            "proposed".to_owned(),
            "--source".to_owned(),
            "slack,agent".to_owned(),
            "--limit".to_owned(),
            "10".to_owned(),
        ],
    )?;
    assert_eq!(search["result_count"], 3);
    assert_eq!(search["truncated"], false);

    let search_ids = search["data"]["items"]
        .as_array()
        .ok_or("search items should be an array")?
        .iter()
        .map(|item| {
            item["decision"]["id"]
                .as_str()
                .ok_or("search item decision id should be a string")
                .map(ToOwned::to_owned)
        })
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    assert_eq!(
        search_ids,
        BTreeSet::from([
            slack_decision_id.clone(),
            codex_decision_id.clone(),
            claude_decision_id.clone(),
        ])
    );

    let history = run_cli_json(
        &hivemind_dir,
        vec![
            "query".to_owned(),
            "get_decisions_changed_since".to_owned(),
            "--since-offset".to_owned(),
            "0".to_owned(),
            "--topic".to_owned(),
            "integrations".to_owned(),
            "--status".to_owned(),
            "proposed".to_owned(),
            "--source".to_owned(),
            "slack,agent".to_owned(),
            "--limit".to_owned(),
            "100".to_owned(),
        ],
    )?;

    let provenance_rows = proposal_provenance_rows(&history)?;
    assert_eq!(provenance_rows.len(), 3);
    assert_eq!(
        provenance_rows
            .iter()
            .map(|row| row.decision_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            slack_decision_id.as_str(),
            codex_decision_id.as_str(),
            claude_decision_id.as_str(),
        ])
    );

    let provenance = provenance_rows
        .iter()
        .map(|row| {
            (
                row.source.as_str(),
                row.actor_id.as_str(),
                row.source_ref.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        provenance,
        BTreeSet::from([
            (
                "agent",
                "agent:claude:demo-claude",
                "agent:claude:demo-claude"
            ),
            ("agent", "agent:codex:demo-codex", "agent:codex:demo-codex"),
            (
                "slack",
                "slack:T123:U111",
                "slack://T123/C456/1715970800.000100"
            ),
        ])
    );

    println!(
        "{}",
        serde_json::to_string_pretty(&demo_report(&provenance_rows))?
    );

    Ok(())
}

fn capture_agent_decision(
    hivemind_dir: &Path,
    tool: &str,
    session: &str,
    title: &str,
    rationale: &str,
) -> TestResult<String> {
    output_value(run_cli_json(
        hivemind_dir,
        vec![
            "emit".to_owned(),
            "decision.capture".to_owned(),
            "--agent-tool".to_owned(),
            tool.to_owned(),
            "--agent-session".to_owned(),
            session.to_owned(),
            "--title".to_owned(),
            title.to_owned(),
            "--rationale".to_owned(),
            rationale.to_owned(),
            "--topic-keys".to_owned(),
            "integrations,agents,local-capture".to_owned(),
            "--options".to_owned(),
            "local-test,hosted-service".to_owned(),
            "--chose".to_owned(),
            "local-test".to_owned(),
        ],
    )?)
}

fn run_cli_json(hivemind_dir: &Path, args: Vec<String>) -> TestResult<Value> {
    let mut argv = vec![
        "hivemind".to_owned(),
        "--json".to_owned(),
        "--hivemind-dir".to_owned(),
        hivemind_dir.display().to_string(),
    ];
    argv.extend(args);

    let cli = Cli::parse_from(argv);
    let output = run(&cli)?;
    Ok(serde_json::from_str(&output)?)
}

fn output_value(output: Value) -> TestResult<String> {
    output["value"]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "CLI output envelope should include string value".into())
}

fn proposal_provenance_rows(history: &Value) -> TestResult<Vec<ProvenanceRow>> {
    let mut rows = Vec::new();
    for item in history["data"]["items"]
        .as_array()
        .ok_or("history items should be an array")?
    {
        if item["event_type"].as_str() != Some("decision.proposed") {
            continue;
        }
        let decision_id = item["decision_ids"]
            .as_array()
            .and_then(|ids| ids.first())
            .and_then(Value::as_str)
            .ok_or("decision.proposed row should include a decision id")?;
        rows.push(ProvenanceRow {
            decision_id: decision_id.to_owned(),
            actor_id: required_str(item, "actor_id")?.to_owned(),
            source: required_str(item, "source")?.to_owned(),
            source_ref: required_str(item, "source_ref")?.to_owned(),
        });
    }
    rows.sort_by(|left, right| left.actor_id.cmp(&right.actor_id));
    Ok(rows)
}

fn required_str<'a>(item: &'a Value, key: &str) -> TestResult<&'a str> {
    item[key]
        .as_str()
        .ok_or_else(|| format!("history row should include string {key}").into())
}

fn demo_report(rows: &[ProvenanceRow]) -> Value {
    let entries = rows
        .iter()
        .map(|row| {
            let mut entry = BTreeMap::new();
            entry.insert("actor_id", json!(row.actor_id));
            entry.insert("source", json!(row.source));
            entry.insert("source_ref", json!(row.source_ref));
            entry
        })
        .collect::<Vec<_>>();

    json!({
        "demo": "local slack plus agent capture",
        "command": "cargo test --test local_capture_demo -- --nocapture",
        "query": {
            "topic": "integrations",
            "status": "proposed",
            "sources": ["slack", "agent"]
        },
        "proposal_provenance": entries,
        "next_step": "Replace the fake Slack fixture and temp local directory with a signed Slack app endpoint and shared service/database while preserving the same source, source_ref, actor_id, and query contract."
    })
}

#[derive(Debug)]
struct ProvenanceRow {
    decision_id: String,
    actor_id: String,
    source: String,
    source_ref: String,
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
