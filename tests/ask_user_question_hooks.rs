//! The capture plugin's AskUserQuestion hooks (hivemind-bbnw.5): a PreToolUse hook records the
//! agent's question as an ask, a PostToolUse hook records the human's answer as a decision that
//! answers the same question, so `why` shows when it was asked and when it was answered.
//!
//! `tests/fixtures/claude_code/ask_user_question/{pre,post}.json` are the hook payloads Claude
//! Code delivered in a recorded interactive session (paths made generic): one call with two
//! questions, a single choice whose label contains a comma, and a multi-select answered with two
//! offered options plus the person's own words. The tests replay them through the real hook
//! script and the real `hivemind` binary.

use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use hivemind::events::{Event, EventType};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::Value;
use tempfile::TempDir;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HOOK: &str = "plugins/hivemind-capture/scripts/ask-hook.sh";
const FIXTURES: &str = "tests/fixtures/claude_code/ask_user_question";
const ASKER: &str = "agent:claude:test/askbot";
const HUMAN: &str = "human:alice";
const DATABASE_QUESTION: &str = "Which database should the demo use?";
const FEATURES_QUESTION: &str = "Which features, if any, should ship first?";

struct Scratch {
    dir: TempDir,
}

impl Scratch {
    fn new() -> TestResult<Self> {
        let scratch = Self {
            dir: tempfile::tempdir()?,
        };
        fs::create_dir_all(scratch.work())?;
        Ok(scratch)
    }

    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn ledger(&self) -> PathBuf {
        self.dir.path().join("ledger")
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The recorded hook payload for `phase` ("pre" or "post"), with `cwd` pointing at a real folder.
fn payload(scratch: &Scratch, phase: &str) -> TestResult<Value> {
    let path = root().join(FIXTURES).join(format!("{phase}.json"));
    let mut payload: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    payload["cwd"] = Value::String(scratch.work().display().to_string());
    Ok(payload)
}

/// Run the hook script for `phase`, feeding it `stdin`, as a Gas City agent slot would: a stable
/// `GC_AGENT`, the ledger and the binary named by environment, nothing else inherited that could
/// change where it writes.
fn run_hook(
    scratch: &Scratch,
    phase: &str,
    stdin: &str,
    envs: &[(&str, &str)],
) -> TestResult<Output> {
    let mut command = Command::new(root().join(HOOK));
    command
        .arg(phase)
        .current_dir(scratch.work())
        .env_remove("GC_ALIAS")
        .env_remove("HIVEMIND_API_URL")
        .env_remove("HIVEMIND_API_KEY")
        .env_remove("HIVEMIND_AGENT_SESSION")
        .env_remove("HIVEMIND_ASK_HOOK_DISABLE")
        .env_remove("HIVEMIND_DATABASE_URL")
        .env_remove("HIVEMIND_PROJECT")
        .env_remove("HIVEMIND_TENANT")
        .env_remove("CLAUDE_SESSION_ID")
        .env("GC_AGENT", "test/askbot")
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", scratch.ledger())
        .env("HIVEMIND_HUMAN_ACTOR", HUMAN)
        .env("CLAUDE_PLUGIN_DATA", scratch.state())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .ok_or("hook stdin")?
        .write_all(stdin.as_bytes())?;
    Ok(child.wait_with_output()?)
}

/// A hook run that must not get in the agent's way: exit 0 and nothing on stdout, whatever
/// happened inside.
fn run_hook_ok(
    scratch: &Scratch,
    phase: &str,
    stdin: &str,
    envs: &[(&str, &str)],
) -> TestResult<Output> {
    let output = run_hook(scratch, phase, stdin, envs)?;
    assert!(
        output.status.success(),
        "the {phase} hook exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "the {phase} hook prints nothing on stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(output)
}

fn events(ledger_dir: &Path) -> TestResult<Vec<Event>> {
    if !ledger_dir.join("ledger.sqlite").exists() {
        return Ok(Vec::new());
    }
    Ok(SqliteEventLedger::open(ledger_dir)?.read(0, 1000)?)
}

fn events_of(ledger_dir: &Path, kind: EventType) -> TestResult<Vec<Event>> {
    Ok(events(ledger_dir)?
        .into_iter()
        .filter(|event| event.event_type == kind)
        .collect())
}

fn cli(ledger_dir: &Path, args: &[&str]) -> TestResult<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(ledger_dir)
        .arg("--json")
        .args(args)
        .output()?;
    assert!(
        output.status.success(),
        "hivemind {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn waiting_texts(ledger_dir: &Path) -> TestResult<Vec<String>> {
    let waiting = cli(ledger_dir, &["query", "get_waiting_requests"])?;
    Ok(waiting["data"]["items"]
        .as_array()
        .ok_or("waiting has an items array")?
        .iter()
        .filter_map(|item| item["text"].as_str().map(str::to_owned))
        .collect())
}

/// `why` for the decision the capture titled `title`: its brief, the `data.root` of the reply.
fn why_for(ledger_dir: &Path, title: &str) -> TestResult<Value> {
    let proposed = events_of(ledger_dir, EventType::DecisionProposed)?
        .into_iter()
        .find(|event| event.payload["title"] == title)
        .ok_or_else(|| format!("no decision titled {title:?}"))?;
    let decision_id = proposed.payload["decision_id"]
        .as_str()
        .ok_or("decision_id")?
        .to_owned();
    let why = cli(ledger_dir, &["query", "why", "--id", &decision_id])?;
    Ok(why["data"]["root"].clone())
}

fn instant(value: &Value) -> TestResult<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value.as_str().ok_or("a timestamp")?)?.with_timezone(&Utc))
}

fn labels(options: &Value) -> Vec<&str> {
    options
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|option| option["label"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn recorded_session_links_each_answer_to_the_ask_it_answers() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();

    // PreToolUse: one ask per question, by the agent, at the moment the tool was called.
    run_hook_ok(&scratch, "pre", &payload(&scratch, "pre")?.to_string(), &[])?;
    let asks = events_of(&ledger, EventType::QuestionAsked)?;
    assert_eq!(asks.len(), 2, "one ask per question: {asks:?}");
    assert!(
        asks.iter().all(|ask| ask.actor_id == ASKER),
        "the agent asked: {asks:?}"
    );
    let mut waiting = waiting_texts(&ledger)?;
    waiting.sort();
    assert_eq!(
        waiting,
        [DATABASE_QUESTION, FEATURES_QUESTION],
        "asked and not yet answered, so both are waiting"
    );
    assert!(
        events_of(&ledger, EventType::DecisionProposed)?.is_empty(),
        "asking decides nothing"
    );

    // PostToolUse: one decision per answered question, decided by the human.
    run_hook_ok(
        &scratch,
        "post",
        &payload(&scratch, "post")?.to_string(),
        &[],
    )?;
    assert!(
        waiting_texts(&ledger)?.is_empty(),
        "both questions are answered, so nothing is waiting"
    );
    let accepted = events_of(&ledger, EventType::DecisionAccepted)?;
    assert_eq!(accepted.len(), 2, "one decision per answer: {accepted:?}");
    assert!(
        accepted.iter().all(|event| event.actor_id == HUMAN),
        "the human decided: {accepted:?}"
    );

    // A single choice from offered options; the label's own comma survives.
    let database = why_for(&ledger, "Database: Postgres, hosted")?;
    assert_eq!(database["question"], DATABASE_QUESTION);
    assert_eq!(database["chosen_option"]["label"], "Postgres, hosted");
    assert_eq!(labels(&database["rejected_options"]), ["SQLite"]);
    assert_eq!(database["decided_by"]["proposer_id"], ASKER);
    assert_eq!(
        database["decided_by"]["decider_ids"],
        serde_json::json!([HUMAN])
    );
    assert!(
        database["quote"].is_null(),
        "no words of their own: {database}"
    );

    // A multi-select with two offered options and the person's own words: one combined choice,
    // the words quoted, and nothing they picked listed as turned down.
    let features = why_for(&ledger, "Features: Search + Export + Other (own words)")?;
    assert_eq!(features["question"], FEATURES_QUESTION);
    assert_eq!(
        features["chosen_option"]["label"],
        "Search + Export + Other (own words)"
    );
    assert_eq!(features["quote"], "Import from Notion");
    assert!(
        labels(&features["rejected_options"]).is_empty(),
        "what was picked is not rejected: {features}"
    );

    // The ledger holds both times per decision: the ask's own timestamp, then the answer's.
    for (brief, question) in [
        (&database, DATABASE_QUESTION),
        (&features, FEATURES_QUESTION),
    ] {
        let asked_at = instant(&brief["asked_at"])?;
        let answered_at = instant(&brief["occurred_at"])?;
        assert!(asked_at < answered_at, "asked before answered: {brief}");
        let ask = asks
            .iter()
            .find(|ask| ask.payload["text"] == question)
            .ok_or("the ask for this question")?;
        assert_eq!(
            Some(asked_at),
            ask.ts,
            "asked_at is the ask event's own time, never a guess"
        );
    }
    Ok(())
}

#[test]
fn a_question_nobody_answers_stays_waiting() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    run_hook_ok(&scratch, "pre", &payload(&scratch, "pre")?.to_string(), &[])?;

    // The tool call finished with an answer to the first question only.
    let mut post = payload(&scratch, "post")?;
    post["tool_response"]["answers"]
        .as_object_mut()
        .ok_or("answers")?
        .remove(FEATURES_QUESTION);
    run_hook_ok(&scratch, "post", &post.to_string(), &[])?;

    assert_eq!(waiting_texts(&ledger)?, [FEATURES_QUESTION]);
    assert_eq!(events_of(&ledger, EventType::DecisionProposed)?.len(), 1);
    Ok(())
}

#[test]
fn an_answer_whose_ask_was_never_recorded_is_still_captured_without_asked_at() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();

    // Only the PostToolUse hook ran (the plugin was installed mid-question).
    run_hook_ok(
        &scratch,
        "post",
        &payload(&scratch, "post")?.to_string(),
        &[],
    )?;

    assert!(events_of(&ledger, EventType::QuestionAsked)?.is_empty());
    let database = why_for(&ledger, "Database: Postgres, hosted")?;
    assert_eq!(database["question"], DATABASE_QUESTION);
    assert!(
        database["asked_at"].is_null(),
        "nobody recorded the ask, so no asked_at is invented: {database}"
    );
    assert!(database["occurred_at"].is_string());
    Ok(())
}

#[test]
fn the_hooks_never_get_in_the_agents_way() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    let pre = payload(&scratch, "pre")?;

    // Not JSON, another tool, an unknown phase, and a tool call with no questions.
    run_hook_ok(&scratch, "pre", "this is not json", &[])?;
    let mut other_tool = pre.clone();
    other_tool["tool_name"] = "Bash".into();
    run_hook_ok(&scratch, "pre", &other_tool.to_string(), &[])?;
    run_hook_ok(&scratch, "sideways", &pre.to_string(), &[])?;
    let mut no_questions = pre.clone();
    no_questions["tool_input"] = serde_json::json!({});
    run_hook_ok(&scratch, "pre", &no_questions.to_string(), &[])?;
    // Switched off.
    run_hook_ok(
        &scratch,
        "pre",
        &pre.to_string(),
        &[("HIVEMIND_ASK_HOOK_DISABLE", "1")],
    )?;
    assert!(events(&ledger)?.is_empty(), "none of those wrote anything");

    // No hivemind binary: the question is still asked, the failure is logged.
    let output = run_hook_ok(
        &scratch,
        "pre",
        &pre.to_string(),
        &[("HIVEMIND_CAPTURE_BIN", "/nonexistent/hivemind")],
    )?;
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not start"),
        "the reason is on stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(scratch.state().join("ask-hook").join("hook.log"))?;
    assert!(
        log.contains("pre:") && log.contains("could not start"),
        "{log}"
    );
    assert!(events(&ledger)?.is_empty());
    Ok(())
}

struct Server {
    child: Child,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `hivemind serve` on a free loopback port with `HIVEMIND_API_KEY` set, once it accepts
/// connections.
fn start_server(ledger: &Path, api_key: &str) -> TestResult<(Server, u16)> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(ledger)
        .args(["serve", "--port", &port.to_string()])
        .env("HIVEMIND_API_KEY", api_key)
        .env_remove("HIVEMIND_DATABASE_URL")
        .env_remove("HIVEMIND_TENANT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let server = Server { child };
    let deadline = Instant::now() + Duration::from_secs(90);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if Instant::now() > deadline {
            return Err("the server did not start listening".into());
        }
        sleep(Duration::from_millis(100));
    }
    Ok((server, port))
}

#[test]
fn with_an_api_url_the_hooks_write_to_that_server_using_its_key() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    fs::create_dir_all(&ledger)?;
    let (_server, port) = start_server(&ledger, "test-key")?;
    let url = format!("http://127.0.0.1:{port}");
    let pre = payload(&scratch, "pre")?.to_string();
    let post = payload(&scratch, "post")?.to_string();

    // The wrong key is refused by the server: nothing is written, the agent is not disturbed.
    let wrong = [
        ("HIVEMIND_API_URL", url.as_str()),
        ("HIVEMIND_API_KEY", "wrong-key"),
    ];
    let output = run_hook_ok(&scratch, "pre", &pre, &wrong)?;
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("401"),
        "the refusal is logged: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(events_of(&ledger, EventType::QuestionAsked)?.is_empty());

    // The right key: the same two hooks, the same result as against a local ledger.
    let right = [
        ("HIVEMIND_API_URL", url.as_str()),
        ("HIVEMIND_API_KEY", "test-key"),
    ];
    run_hook_ok(&scratch, "pre", &pre, &right)?;
    assert_eq!(events_of(&ledger, EventType::QuestionAsked)?.len(), 2);
    run_hook_ok(&scratch, "post", &post, &right)?;
    assert!(waiting_texts(&ledger)?.is_empty());
    let database = why_for(&ledger, "Database: Postgres, hosted")?;
    assert!(database["asked_at"].is_string(), "{database}");
    assert!(database["occurred_at"].is_string(), "{database}");
    Ok(())
}

#[test]
fn the_plugin_registers_the_hooks_for_ask_user_question() -> TestResult<()> {
    let hooks: Value = serde_json::from_str(&fs::read_to_string(
        root().join("plugins/hivemind-capture/hooks/hooks.json"),
    )?)?;
    for (event, phase) in [("PreToolUse", "pre"), ("PostToolUse", "post")] {
        let entries = hooks["hooks"][event]
            .as_array()
            .ok_or_else(|| format!("{event} hooks"))?;
        assert_eq!(entries.len(), 1, "{event}: {entries:?}");
        assert_eq!(entries[0]["matcher"], "AskUserQuestion");
        let command = entries[0]["hooks"][0]["command"]
            .as_str()
            .ok_or("hook command")?;
        assert!(
            command.contains("${CLAUDE_PLUGIN_ROOT}/scripts/ask-hook.sh")
                && command.ends_with(&format!(" {phase}")),
            "{event} runs the script for {phase}: {command}"
        );
        assert_eq!(entries[0]["hooks"][0]["type"], "command");
    }
    #[cfg(unix)]
    {
        for script in ["ask-hook.sh", "ask_hook.py"] {
            let mode = fs::metadata(root().join("plugins/hivemind-capture/scripts").join(script))?
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "{script} is executable");
        }
    }
    Ok(())
}
