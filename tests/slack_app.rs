use std::path::Path;

use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::{EventSource, EventType};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::Value;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn slack_app_installs_queues_captures_and_queries_with_citations() -> TestResult<()> {
    let scratch = TempDir::new("slack-app")?;
    let hivemind_dir = scratch.path().join("hivemind");

    let manifest = run_cli_json(
        &hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "manifest".to_owned(),
            "--request-url".to_owned(),
            "https://local.example/slack/interactions".to_owned(),
            "--event-url".to_owned(),
            "https://local.example/slack/events".to_owned(),
            "--redirect-url".to_owned(),
            "https://local.example/slack/oauth".to_owned(),
        ],
    )?;
    assert_eq!(
        manifest["features"]["slash_commands"][0]["command"],
        "/hivemind"
    );
    assert_eq!(
        manifest["settings"]["event_subscriptions"]["bot_events"],
        serde_json::json!(["reaction_added"])
    );

    let bot_token = generated_test_secret("bot");
    let signing_secret = generated_test_secret("signing");
    let install = run_cli_json(
        &hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "install".to_owned(),
            "--team-id".to_owned(),
            "T123".to_owned(),
            "--team-name".to_owned(),
            "Example".to_owned(),
            "--bot-token".to_owned(),
            bot_token,
            "--signing-secret".to_owned(),
            signing_secret,
            "--hivemind-url".to_owned(),
            "http://127.0.0.1:8787".to_owned(),
            "--reaction-emoji".to_owned(),
            "hivemind".to_owned(),
        ],
    )?;
    assert_eq!(install["team_id"], "T123");
    assert_eq!(install["bot_token_stored"], true);
    assert!(hivemind_dir
        .join("slack-app")
        .join("installations.json")
        .exists());

    let capture_modal = run_cli_json(
        &hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "command".to_owned(),
            "--team-id".to_owned(),
            "T123".to_owned(),
            "--user-id".to_owned(),
            "U111".to_owned(),
            "--text".to_owned(),
            "capture".to_owned(),
        ],
    )?;
    assert_eq!(capture_modal["action"], "open_modal");
    assert_eq!(
        capture_modal["modal"]["private_metadata"]["actor_id"],
        "slack:T123:U111"
    );

    let permalink = "https://example.slack.com/archives/C456/p1715970800000100";
    let queued = run_cli_json(
        &hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "enqueue-capture".to_owned(),
            "--team-id".to_owned(),
            "T123".to_owned(),
            "--user-id".to_owned(),
            "U111".to_owned(),
            "--channel-id".to_owned(),
            "C456".to_owned(),
            "--message-ts".to_owned(),
            "1715970800.000100".to_owned(),
            "--permalink".to_owned(),
            permalink.to_owned(),
            "--surface".to_owned(),
            "message_action".to_owned(),
            "--title".to_owned(),
            "Use local Slack app capture".to_owned(),
            "--rationale".to_owned(),
            "The Slack app queues reviewed captures before writing HiveMind events".to_owned(),
            "--topic-keys".to_owned(),
            "slack,integrations".to_owned(),
            "--options".to_owned(),
            "local-first,hosted-service".to_owned(),
            "--chose".to_owned(),
            "local-first".to_owned(),
            "--thread-text".to_owned(),
            "Decision discussed in a Slack thread".to_owned(),
        ],
    )?;
    assert_eq!(queued["attempts"], 0);
    assert_eq!(queued["capture"]["surface"], "message_action");

    let drain = run_cli_json(
        &hivemind_dir,
        vec!["slack-app".to_owned(), "drain".to_owned()],
    )?;
    assert_eq!(drain["processed_count"], 1);
    assert_eq!(drain["queued_after"], 0);
    let decision_id = drain["processed"][0]["decision_id"]
        .as_str()
        .ok_or("decision id missing")?
        .to_owned();

    let query = run_cli_json(
        &hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "command".to_owned(),
            "--team-id".to_owned(),
            "T123".to_owned(),
            "--user-id".to_owned(),
            "U111".to_owned(),
            "--text".to_owned(),
            "query Slack app".to_owned(),
            "--limit".to_owned(),
            "3".to_owned(),
        ],
    )?;
    assert!(query["text"]
        .as_str()
        .ok_or("query text")?
        .contains("HiveMind found 1 decision"));
    assert!(query["blocks"][0]["text"]["text"]
        .as_str()
        .ok_or("query block text")?
        .contains(permalink));

    let show = run_cli_json(
        &hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "command".to_owned(),
            "--team-id".to_owned(),
            "T123".to_owned(),
            "--user-id".to_owned(),
            "U111".to_owned(),
            "--text".to_owned(),
            format!("show {decision_id}"),
        ],
    )?;
    let show_text = show["blocks"][0]["text"]["text"]
        .as_str()
        .ok_or("show block text")?;
    assert!(show_text.contains("options=option-"));
    assert!(show_text.contains("evidence=evidence-"));
    assert!(show_text.contains(permalink));

    let ledger = SqliteEventLedger::open(&hivemind_dir)?;
    let events = ledger.read(0, 100)?;
    let proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(Value::as_str)
                    == Some(decision_id.as_str())
        })
        .ok_or("proposal event missing")?;
    assert_eq!(proposal.actor_id, "slack:T123:U111");
    assert_eq!(proposal.source, EventSource::Slack);
    assert_eq!(proposal.source_ref.as_deref(), Some(permalink));

    let evidence = events
        .iter()
        .filter(|event| event.event_type == EventType::EvidenceRecorded)
        .collect::<Vec<_>>();
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0]
        .payload
        .get("content")
        .and_then(Value::as_str)
        .ok_or("evidence content")?
        .contains(permalink));

    assert!(events
        .iter()
        .all(|event| event.source == EventSource::Slack));

    Ok(())
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

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(name: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn generated_test_secret(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}
