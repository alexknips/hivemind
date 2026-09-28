//! `hivemind ask` and `emit decision.capture --answers` (hivemind-bbnw.4): recording an explicit
//! ask before any decision answers it, linking a later capture to it without repeating the
//! words, and listing open requests as "waiting" until answered.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::{Event, EventType};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::Value;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const ASKER: &str = "human:alice";
const CAPTURER: &str = "agent:claude:capturer";
const QUESTION: &str = "Which storage engine should the prototype use?";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-ask-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_cli(dir: &Path, actor: &str, json: bool, args: &[&str]) -> hivemind::Result<String> {
    let mut argv = vec![
        "hivemind".to_owned(),
        "--hivemind-dir".to_owned(),
        dir.display().to_string(),
        "--actor".to_owned(),
        actor.to_owned(),
    ];
    if json {
        argv.push("--json".to_owned());
    }
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    run(&Cli::parse_from(argv))
}

fn ask(dir: &Path, text: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(&run_cli(
        dir,
        ASKER,
        true,
        &["ask", text],
    )?)?)
}

/// A capture that chose `chose` from fixed options, answering `request_id`'s question.
fn capture_answers(dir: &Path, title: &str, chose: &str, request_id: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &[
            "emit",
            "decision.capture",
            "--bet",
            "--title",
            title,
            "--rationale",
            "Rationale text long enough for the readable floor",
            "--topic-keys",
            "storage",
            "--options",
            "sqlite,postgres",
            "--chose",
            chose,
            "--answers",
            request_id,
        ],
    )?)?)
}

fn events(dir: &Path) -> TestResult<Vec<Event>> {
    Ok(SqliteEventLedger::open(dir)?.read(0, 1000)?)
}

fn waiting_requests(dir: &Path) -> TestResult<Value> {
    Ok(serde_json::from_str(&run_cli(
        dir,
        ASKER,
        true,
        &["query", "get_waiting_requests"],
    )?)?)
}

fn why_json(dir: &Path, decision_id: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &["query", "why", "--id", decision_id],
    )?)?)
}

#[test]
fn ask_writes_question_recorded_and_question_asked_and_is_listed_as_waiting() -> TestResult<()> {
    let scratch = Scratch::new("ask-waiting")?;
    let dir = scratch.path();

    let asked = ask(dir, QUESTION)?;
    let request_id = asked["request_id"]
        .as_str()
        .ok_or("ask returns request_id")?;
    let question_id = asked["question_id"]
        .as_str()
        .ok_or("ask returns question_id")?;
    assert_eq!(asked["reused"], false, "first ask creates the question");

    let recorded = events(dir)?
        .into_iter()
        .filter(|event| event.event_type == EventType::QuestionRecorded)
        .count();
    assert_eq!(recorded, 1, "one question.recorded for a new question");
    let asked_events = events(dir)?
        .into_iter()
        .filter(|event| event.event_type == EventType::QuestionAsked)
        .count();
    assert_eq!(asked_events, 1, "one question.asked for one ask");

    let waiting = waiting_requests(dir)?;
    let items = waiting["data"]["items"]
        .as_array()
        .ok_or("waiting has an items array")?;
    assert_eq!(items.len(), 1, "the unanswered ask is waiting");
    assert_eq!(items[0]["request_id"], request_id);
    assert_eq!(items[0]["question_id"], question_id);
    assert_eq!(items[0]["text"], QUESTION);
    assert_eq!(items[0]["requested_by"], ASKER);

    Ok(())
}

#[test]
fn capture_answers_links_to_the_request_and_it_stops_waiting() -> TestResult<()> {
    let scratch = Scratch::new("answers")?;
    let dir = scratch.path();

    let asked = ask(dir, QUESTION)?;
    let request_id = asked["request_id"]
        .as_str()
        .ok_or("ask returns request_id")?;
    let question_id = asked["question_id"]
        .as_str()
        .ok_or("ask returns question_id")?
        .to_owned();

    let captured = capture_answers(dir, "Prototype storage is SQLite", "sqlite", request_id)?;
    assert_eq!(captured["question_id"], question_id);
    let decision_id = captured["value"]
        .as_str()
        .ok_or("capture returns the decision id")?;

    let waiting = waiting_requests(dir)?;
    let items = waiting["data"]["items"]
        .as_array()
        .ok_or("waiting has an items array")?;
    assert!(
        items.is_empty(),
        "the request is answered, so it is no longer waiting: {waiting}"
    );

    let why = why_json(dir, decision_id)?;
    assert!(
        why["data"]["root"]["asked_at"].is_string(),
        "why shows asked_at for a decision answering an explicit ask: {why}"
    );
    assert!(
        why["data"]["root"]["occurred_at"].is_string(),
        "why shows occurred_at (answered_at in the CLI summary): {why}"
    );

    Ok(())
}

#[test]
fn answers_with_an_unknown_request_id_is_refused_and_writes_nothing() -> TestResult<()> {
    let scratch = Scratch::new("unknown-request")?;
    let dir = scratch.path();

    let before = events(dir)?.len();
    let result = capture_answers(
        dir,
        "Prototype storage is SQLite",
        "sqlite",
        "018f5d8a-03fb-7df0-8e36-64d7410cfe07",
    );
    assert!(result.is_err(), "an unknown request id is refused");
    assert_eq!(
        events(dir)?.len(),
        before,
        "a refused capture writes nothing"
    );

    Ok(())
}

#[test]
fn asking_only_punctuation_is_refused_and_nothing_is_written() -> TestResult<()> {
    let scratch = Scratch::new("punctuation")?;
    let dir = scratch.path();

    let result = ask(dir, "???");
    assert!(result.is_err(), "a question with no words is refused");
    assert!(events(dir)?.is_empty(), "a refused ask writes nothing");

    Ok(())
}

#[test]
fn asking_the_same_question_twice_reuses_the_node_and_both_stop_waiting_once_answered(
) -> TestResult<()> {
    let scratch = Scratch::new("reasked")?;
    let dir = scratch.path();

    let first = ask(dir, QUESTION)?;
    let second = ask(dir, "which storage engine should the PROTOTYPE use")?;
    assert_ne!(
        first["request_id"], second["request_id"],
        "each ask is its own request"
    );
    assert_eq!(
        first["question_id"], second["question_id"],
        "the same normalized text shares one question node"
    );
    assert_eq!(second["reused"], true, "the second ask reuses the node");

    let waiting = waiting_requests(dir)?;
    let items = waiting["data"]["items"]
        .as_array()
        .ok_or("waiting has an items array")?;
    assert_eq!(items.len(), 2, "both asks are outstanding requests");

    let request_id = first["request_id"].as_str().ok_or("request_id")?;
    capture_answers(dir, "Prototype storage is SQLite", "sqlite", request_id)?;

    let waiting = waiting_requests(dir)?;
    let items = waiting["data"]["items"]
        .as_array()
        .ok_or("waiting has an items array")?;
    assert!(
        items.is_empty(),
        "answering the question clears every outstanding ask of it: {waiting}"
    );

    Ok(())
}
