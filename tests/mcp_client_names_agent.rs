//! Which agent a capture is filed under when nothing names it (hivemind-tiu9). A user who adds
//! HiveMind to Claude Code or Cursor without `--agent-tool` used to get every capture filed as
//! `agent:codex:...`. The MCP server now takes the name from the client's own `initialize`
//! handshake; a plain CLI write with nothing to go on gets a neutral name; the flag and the
//! environment still win. Each test runs the real binary in an emptied environment, so the
//! ambient session variables of whoever runs the suite cannot decide the answer.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hivemind::events::{Event, EventType};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::{json, Value};
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-tiu9-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The real binary with no inherited environment but `PATH`: nothing in it names an agent.
fn hivemind(dir: &Path, envs: &BTreeMap<&str, &str>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hivemind"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .envs(envs)
        .arg("--hivemind-dir")
        .arg(dir);
    command
}

fn proposed_actor(dir: &Path) -> TestResult<String> {
    let ledger = SqliteEventLedger::open(dir)?;
    let events: Vec<Event> = ledger.read(0, 100)?;
    let proposed = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .ok_or("no decision.proposed event was written")?;
    Ok(proposed.actor_id.clone())
}

/// Drive a stdio MCP session: `initialize` (carrying `client_info` when given), then a
/// `capture_decision` that names no actor. Returns the actor the decision was filed under.
fn mcp_capture_actor(
    label: &str,
    server_args: &[&str],
    envs: &[(&str, &str)],
    client_info: Option<Value>,
) -> TestResult<String> {
    let scratch = Scratch::new(label)?;
    let envs: BTreeMap<&str, &str> = envs.iter().copied().collect();
    let mut child = hivemind(scratch.path(), &envs)
        .arg("mcp")
        .args(server_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);

    let mut call = |request: Value| -> TestResult<Value> {
        writeln!(stdin, "{request}")?;
        stdin.flush()?;
        let mut line = String::new();
        stdout.read_line(&mut line)?;
        Ok(serde_json::from_str(line.trim())?)
    };

    let params = match client_info {
        Some(info) => {
            json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": info})
        }
        None => json!({"protocolVersion": "2025-03-26", "capabilities": {}}),
    };
    call(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params}))?;
    let captured = call(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "grounding": [{"kind": "bet"}],
                "title": "Name the agent from the client",
                "rationale": "The client already says who it is in its handshake, so the right name needs no flag",
                "topic_keys": ["identity"],
                "options": [{"label": "client"}, {"label": "flag"}],
                "chosen_option_label": "client"
            }
        }
    }))?;
    drop(stdin);
    child.wait()?;
    assert_eq!(
        captured["result"]["isError"],
        Value::Bool(false),
        "{captured}"
    );
    proposed_actor(scratch.path())
}

fn assert_filed_under(actor: &str, tool: &str) {
    assert!(
        actor.starts_with(&format!("agent:{tool}:")),
        "expected a capture filed under agent:{tool}:..., got {actor}"
    ); // ubs:ignore: test-only assertion
}

#[test]
fn claude_code_with_no_flag_is_filed_as_claude() -> TestResult<()> {
    // `claude mcp add hivemind -- hivemind mcp`: no flag, and the server Claude Code starts
    // sees none of the session variables.
    let actor = mcp_capture_actor(
        "claude",
        &[],
        &[],
        Some(json!({"name": "claude-code", "version": "2.0.0"})),
    )?;
    assert_filed_under(&actor, "claude");
    Ok(())
}

#[test]
fn codex_with_no_flag_is_filed_as_codex() -> TestResult<()> {
    let actor = mcp_capture_actor(
        "codex",
        &[],
        &[],
        Some(json!({"name": "codex-mcp-client", "version": "0.1.0"})),
    )?;
    assert_filed_under(&actor, "codex");
    Ok(())
}

#[test]
fn cursor_with_no_flag_is_filed_as_cursor() -> TestResult<()> {
    let actor = mcp_capture_actor(
        "cursor",
        &[],
        &[],
        Some(json!({"name": "Cursor", "version": "1.0.0"})),
    )?;
    assert_filed_under(&actor, "cursor");
    Ok(())
}

#[test]
fn another_client_is_filed_under_its_own_name() -> TestResult<()> {
    let actor = mcp_capture_actor(
        "foo",
        &[],
        &[],
        Some(json!({"name": "foo", "version": "1"})),
    )?;
    assert_filed_under(&actor, "foo");
    Ok(())
}

#[test]
fn a_client_that_names_nothing_is_filed_under_the_neutral_name() -> TestResult<()> {
    assert_filed_under(&mcp_capture_actor("no-info", &[], &[], None)?, "unknown");
    assert_filed_under(
        &mcp_capture_actor("blank-name", &[], &[], Some(json!({"name": "  "})))?,
        "unknown",
    );
    Ok(())
}

#[test]
fn the_flag_wins_over_the_client_handshake() -> TestResult<()> {
    let actor = mcp_capture_actor(
        "flag-wins",
        &["--agent-tool", "cursor"],
        &[],
        Some(json!({"name": "claude-code"})),
    )?;
    assert_filed_under(&actor, "cursor");
    Ok(())
}

#[test]
fn the_environment_wins_over_the_client_handshake() -> TestResult<()> {
    let actor = mcp_capture_actor(
        "env-wins",
        &[],
        &[("HIVEMIND_AGENT_TOOL", "codex")],
        Some(json!({"name": "claude-code"})),
    )?;
    assert_filed_under(&actor, "codex");
    Ok(())
}

#[test]
fn a_plain_cli_agent_capture_with_nothing_to_go_on_is_not_filed_as_codex() -> TestResult<()> {
    let scratch = Scratch::new("cli")?;
    let output = hivemind(scratch.path(), &BTreeMap::new())
        .args([
            "emit",
            "decision.capture",
            "--source",
            "agent",
            "--bet",
            "--title",
            "Name the agent from the client",
            "--rationale",
            "A capture with no tool and no environment must not claim to be codex",
            "--topic-keys",
            "identity",
            "--options",
            "neutral,codex",
            "--chose",
            "neutral",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    ); // ubs:ignore: test-only assertion
    assert_filed_under(&proposed_actor(scratch.path())?, "unknown");
    Ok(())
}

#[test]
fn a_cli_capture_with_the_flag_is_unchanged() -> TestResult<()> {
    let scratch = Scratch::new("cli-flag")?;
    let output = hivemind(scratch.path(), &BTreeMap::new())
        .args([
            "emit",
            "decision.capture",
            "--source",
            "agent",
            "--agent-tool",
            "claude",
            "--bet",
            "--title",
            "Name the agent from the client",
            "--rationale",
            "The flag still names the agent, whatever else is or is not set",
            "--topic-keys",
            "identity",
            "--options",
            "flag,client",
            "--chose",
            "flag",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    ); // ubs:ignore: test-only assertion
    assert_filed_under(&proposed_actor(scratch.path())?, "claude");
    Ok(())
}
