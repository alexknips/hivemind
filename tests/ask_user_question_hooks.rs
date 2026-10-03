//! The capture plugin's AskUserQuestion hooks (hivemind-bbnw.5): a PreToolUse hook records the
//! agent's question as an ask, a PostToolUse hook records the human's answer as a decision that
//! answers the same question, so `why` shows when it was asked and when it was answered.
//!
//! `tests/fixtures/claude_code/ask_user_question/{pre,post}.json` are the hook payloads Claude
//! Code delivered in a recorded interactive session (paths made generic): one call with two
//! questions, a single choice whose label contains a comma, and a multi-select answered with two
//! offered options plus the person's own words. The tests replay them through the real hook
//! script and the real `hivemind` binary.
//!
//! A registered project accepts only the topic keys it declared (hivemind-zywz), so the tests that
//! name a project anchor it to a rig and run the hooks in that rig (`GC_RIG`), the way a Gas City
//! checkout is anchored: the answer must be recorded there too, under the header's key when the
//! project declared it and under the one fixed key otherwise.

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
/// The titles the hook gives the two recorded answers: `<header>: <chosen option>`.
const DATABASE_TITLE: &str = "Database: Postgres, hosted";
const FEATURES_TITLE: &str = "Features: Search + Export";
/// What the hook's fixed rationale says when a person gave neither a note nor words of their own.
const NO_REASONS: &str = "no reasons were given";
/// The rig the registered-project tests anchor their project to and run the hooks in.
const RIG: &str = "testrig";
/// The topic key an answer goes under when its project has not declared the header's key.
const FALLBACK_TOPIC: &str = "claude-code-question";

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
        .env_remove("GC_RIG")
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

/// The topic keys the decision the capture titled `title` was proposed under.
fn topic_keys_of(ledger_dir: &Path, title: &str) -> TestResult<Vec<String>> {
    let proposed = events_of(ledger_dir, EventType::DecisionProposed)?
        .into_iter()
        .find(|event| event.payload["title"] == title)
        .ok_or_else(|| format!("no decision titled {title:?}"))?;
    Ok(proposed.payload["topic_keys"]
        .as_array()
        .ok_or("topic_keys")?
        .iter()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect())
}

/// Register `handle` with `topics` in its vocabulary and anchor it to [`RIG`], as the operator
/// of a rig would, so a hook run with `GC_RIG=testrig` files its writes under it.
fn register_rig_project(ledger_dir: &Path, handle: &str, topics: &[&str]) -> TestResult<()> {
    fs::create_dir_all(ledger_dir)?;
    let registrar = |args: &[&str]| -> TestResult<Value> {
        let mut full = vec!["--actor", "human:test-registrar"];
        full.extend_from_slice(args);
        cli(ledger_dir, &full)
    };
    registrar(&["project", "register", handle])?;
    registrar(&[
        "project", "anchor", "--handle", handle, "--kind", "rig", "--value", RIG,
    ])?;
    for &topic in topics {
        registrar(&["project", "declare-topic", handle, topic])?;
    }
    Ok(())
}

/// Every topic key `handle` declared, in the order the ledger recorded them.
fn declared_topics(ledger_dir: &Path, handle: &str) -> TestResult<Vec<String>> {
    Ok(events_of(ledger_dir, EventType::ProjectTopicDeclared)?
        .iter()
        .filter(|event| event.payload["handle"] == handle)
        .filter_map(|event| event.payload["topic_key"].as_str().map(str::to_owned))
        .collect())
}

/// What the hooks logged, or nothing if they never logged.
fn hook_log(scratch: &Scratch) -> String {
    fs::read_to_string(scratch.state().join("ask-hook").join("hook.log")).unwrap_or_default()
}

fn instant(value: &Value) -> TestResult<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value.as_str().ok_or("a timestamp")?)?.with_timezone(&Utc))
}

fn rationale(brief: &Value) -> TestResult<&str> {
    Ok(brief["rationale"].as_str().ok_or("a rationale")?)
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

/// One question an agent put to the person, as the tool's input lists it.
struct Asked<'a> {
    header: &'a str,
    question: &'a str,
    options: &'a [&'a str],
    multi: bool,
}

impl Asked<'_> {
    fn input(&self) -> Value {
        serde_json::json!({
            "question": self.question,
            "header": self.header,
            "multiSelect": self.multi,
            "options": self
                .options
                .iter()
                .map(|label| serde_json::json!({ "label": label, "description": "" }))
                .collect::<Vec<_>>(),
        })
    }

    /// The PreToolUse payload for this question.
    fn pre(&self, scratch: &Scratch) -> Value {
        serde_json::json!({
            "session_id": "8c2ccf9a-2bb7-4c1b-9539-d20c9bb0c3c0",
            "cwd": scratch.work().display().to_string(),
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": { "questions": [self.input()] },
        })
    }

    /// The PostToolUse payload once the person answered with `answer` as the tool reports it,
    /// plus the note they wrote, if any.
    fn post(&self, scratch: &Scratch, answer: &str, note: Option<&str>) -> Value {
        let mut payload = self.pre(scratch);
        payload["hook_event_name"] = "PostToolUse".into();
        for source in ["tool_input", "tool_response"] {
            payload[source]["questions"] = serde_json::json!([self.input()]);
            payload[source]["answers"] = serde_json::json!({ self.question: answer });
            if let Some(note) = note {
                payload[source]["annotations"] =
                    serde_json::json!({ self.question: { "notes": note } });
            }
        }
        payload
    }
}

/// Every word the ledger recorded, to look for text that must not be there.
fn ledger_text(ledger_dir: &Path) -> TestResult<String> {
    Ok(events(ledger_dir)?
        .iter()
        .map(|event| event.payload.to_string())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The bucket the hook used to record for words it could not match, and the placeholder Claude
/// Code reports for an answer that is only a note: neither is anything a person said or chose.
fn assert_no_placeholders(ledger_dir: &Path) -> TestResult<()> {
    let text = ledger_text(ledger_dir)?;
    for placeholder in ["Other (own words)", "(notes only)"] {
        assert!(
            !text.contains(placeholder),
            "{placeholder:?} is recorded in the ledger: {text}"
        );
    }
    Ok(())
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
    assert!(
        rationale(&database)?.contains(NO_REASONS),
        "no note and no words of their own, so the fixed sentence: {database}"
    );

    // A multi-select with two offered options and the person's own words: the two picks are one
    // combined choice, the words are quoted beside it (no bucket option stands in for them), and
    // nothing they picked is listed as turned down.
    let features = why_for(&ledger, FEATURES_TITLE)?;
    assert_eq!(features["question"], FEATURES_QUESTION);
    assert_eq!(features["chosen_option"]["label"], "Search + Export");
    assert_eq!(features["quote"], "Import from Notion");
    assert!(
        labels(&features["rejected_options"]).is_empty(),
        "what was picked is not rejected: {features}"
    );
    assert!(
        !rationale(&features)?.contains(NO_REASONS),
        "their own words may hold reasons, so none are claimed absent: {features}"
    );
    assert_no_placeholders(&ledger)?;

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
fn a_note_on_the_pick_is_the_rationale_word_for_word() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();

    // The recorded post payload plus the `annotations` Claude Code reports beside `answers` when a
    // person writes a note on their pick: keyed by question, `notes` is their free text. One note
    // reads on its own; the other is too short to be a rationale.
    let note = "SQLite cannot be shared between the two demo hosts, so it has to be a server.";
    let short_note = "and Notion";
    let mut post = payload(&scratch, "post")?;
    for source in ["tool_input", "tool_response"] {
        post[source]["annotations"][DATABASE_QUESTION] = serde_json::json!({ "notes": note });
        post[source]["annotations"][FEATURES_QUESTION] = serde_json::json!({ "notes": short_note });
    }
    run_hook_ok(&scratch, "post", &post.to_string(), &[])?;
    assert_eq!(
        events_of(&ledger, EventType::DecisionAccepted)?.len(),
        2,
        "one decision per answer, the refused first attempt wrote nothing"
    );

    // A note that stands on its own is the rationale, word for word, and no reasons are claimed
    // absent.
    let database = why_for(&ledger, "Database: Postgres, hosted")?;
    assert_eq!(rationale(&database)?, note);
    assert!(database["quote"].is_null(), "{database}");

    // A note the write layer refuses as a rationale is quoted instead, after their own words, and
    // the rationale says a note is attached.
    let features = why_for(&ledger, FEATURES_TITLE)?;
    assert!(
        rationale(&features)?.contains("added a note")
            && !rationale(&features)?.contains(NO_REASONS),
        "{features}"
    );
    assert_eq!(
        features["quote"], "Import from Notion\n\nNote: and Notion",
        "{features}"
    );
    Ok(())
}

#[test]
fn own_words_that_lead_with_an_offered_option_choose_it() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    let retries = Asked {
        header: "Retries",
        question: "How many times should a failed job retry?",
        options: &["Three times", "Five times"],
        multi: false,
    };
    let words = "Five times, because the jobs are cheap and the queue backs off";
    run_hook_ok(
        &scratch,
        "post",
        &retries.post(&scratch, words, None).to_string(),
        &[],
    )?;

    // What they chose is the offered option their words lead with; the other offered option is the
    // one turned down; their words are quoted whole.
    let brief = why_for(&ledger, "Retries: Five times")?;
    assert_eq!(brief["chosen_option"]["label"], "Five times");
    assert_eq!(labels(&brief["rejected_options"]), ["Three times"]);
    assert_eq!(brief["quote"], words);
    assert!(
        rationale(&brief)?.contains("own words") && !rationale(&brief)?.contains(NO_REASONS),
        "their words may hold reasons, so none are claimed absent: {brief}"
    );

    // Words that are only the offered label, in another case with a full stop, are a plain pick:
    // nothing to quote, and the fixed sentence says no reasons were given.
    let database = Asked {
        header: "Database",
        question: DATABASE_QUESTION,
        options: &["SQLite", "Postgres, hosted"],
        multi: false,
    };
    run_hook_ok(
        &scratch,
        "post",
        &database
            .post(&scratch, "postgres, hosted.", None)
            .to_string(),
        &[],
    )?;
    let brief = why_for(&ledger, "Database: Postgres, hosted")?;
    assert_eq!(brief["chosen_option"]["label"], "Postgres, hosted");
    assert_eq!(labels(&brief["rejected_options"]), ["SQLite"]);
    assert!(brief["quote"].is_null(), "{brief}");
    assert!(rationale(&brief)?.contains(NO_REASONS), "{brief}");

    assert_no_placeholders(&ledger)?;
    Ok(())
}

#[test]
fn own_words_that_do_not_name_exactly_one_offered_option_are_the_answer_and_reject_nothing(
) -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    let cases = [
        // Names no offered option.
        (
            Asked {
                header: "Retries",
                question: "How many retries should a failed job get?",
                options: &["Three times", "Five times"],
                multi: false,
            },
            "Back off exponentially and never give up",
        ),
        // Names two, so which one they chose is not for the hook to guess.
        (
            Asked {
                header: "Cache",
                question: "Which cache should the workers share?",
                options: &["Redis", "In-process"],
                multi: false,
            },
            "Redis or In-process, whichever is simpler",
        ),
        // Names one, but turns it down.
        (
            Asked {
                header: "Cache",
                question: "Which cache should the workers use instead?",
                options: &["Redis", "In-process"],
                multi: false,
            },
            "Not Redis, the other one",
        ),
        // A multi-select answered with words only.
        (
            Asked {
                header: "Features",
                question: "Which features should ship first?",
                options: &["Search", "Export"],
                multi: true,
            },
            "Import from Notion",
        ),
    ];
    for (asked, words) in &cases {
        run_hook_ok(
            &scratch,
            "post",
            &asked.post(&scratch, words, None).to_string(),
            &[],
        )?;
        // What they chose is what they said. No offered option is listed beside it, because the
        // record lists an option only to say it was turned down, and the words do not say that.
        let brief = why_for(&ledger, &format!("{}: {words}", asked.header))?;
        assert_eq!(brief["chosen_option"]["label"], *words, "{brief}");
        assert!(
            labels(&brief["rejected_options"]).is_empty(),
            "no offered option is recorded as turned down: {brief}"
        );
        assert_eq!(brief["quote"], *words, "{brief}");
        assert!(
            rationale(&brief)?.contains("none of those options is recorded as turned down"),
            "the record says why no option is listed: {brief}"
        );
        assert_eq!(brief["question"], asked.question);
    }
    assert_no_placeholders(&ledger)?;
    Ok(())
}

#[test]
fn a_notes_only_answer_that_leads_with_an_offered_option_chooses_it() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    let queue = Asked {
        header: "Queue",
        question: "Which queue should the scratch service use?",
        options: &["Kafka", "NATS"],
        multi: false,
    };
    // The person picked nothing and wrote a note: Claude Code reports "(notes only)" as the answer.
    let note = "NATS, because we lose nothing if a scratch job is dropped";
    run_hook_ok(
        &scratch,
        "post",
        &queue.post(&scratch, "(notes only)", Some(note)).to_string(),
        &[],
    )?;

    // The note is what they said, so it is the rationale, word for word, and it chose NATS. The
    // placeholder is never quoted as their words.
    let brief = why_for(&ledger, "Queue: NATS")?;
    assert_eq!(brief["chosen_option"]["label"], "NATS");
    assert_eq!(labels(&brief["rejected_options"]), ["Kafka"]);
    assert_eq!(rationale(&brief)?, note);
    assert!(brief["quote"].is_null(), "{brief}");
    assert_no_placeholders(&ledger)?;
    Ok(())
}

#[test]
fn a_notes_only_answer_that_names_no_offered_option_is_the_note() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    let store = Asked {
        header: "Store",
        question: "Which store should the scratch service use?",
        options: &["Kafka", "NATS"],
        multi: false,
    };
    let note = "A plain Postgres table is enough for the scratch service.";
    run_hook_ok(
        &scratch,
        "post",
        &store.post(&scratch, "(notes only)", Some(note)).to_string(),
        &[],
    )?;

    let brief = why_for(
        &ledger,
        "Store: A plain Postgres table is enough for the scratch service",
    )?;
    assert_eq!(brief["chosen_option"]["label"], note);
    assert!(
        labels(&brief["rejected_options"]).is_empty(),
        "no offered option is recorded as turned down: {brief}"
    );
    assert_eq!(rationale(&brief)?, note);
    assert!(brief["quote"].is_null(), "{brief}");
    assert_no_placeholders(&ledger)?;
    Ok(())
}

#[test]
fn a_notes_only_answer_with_no_note_records_nothing_and_the_ask_stays_waiting() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    let queue = Asked {
        header: "Queue",
        question: "Which queue should the scratch service use?",
        options: &["Kafka", "NATS"],
        multi: false,
    };
    run_hook_ok(&scratch, "pre", &queue.pre(&scratch).to_string(), &[])?;
    run_hook_ok(
        &scratch,
        "post",
        &queue.post(&scratch, "(notes only)", None).to_string(),
        &[],
    )?;

    assert!(
        events_of(&ledger, EventType::DecisionProposed)?.is_empty(),
        "there is no answer to record"
    );
    assert_eq!(waiting_texts(&ledger)?, [queue.question]);
    assert!(
        hook_log(&scratch).contains("notes only"),
        "the skip is logged: {}",
        hook_log(&scratch)
    );
    Ok(())
}

#[test]
fn a_header_the_project_never_declared_is_filed_under_the_fixed_key_declared_once() -> TestResult<()>
{
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    // The project declares neither header of the recorded questions ("database", "features").
    register_rig_project(&ledger, "billing", &["invoicing"])?;
    let in_rig = [("GC_RIG", RIG)];

    run_hook_ok(
        &scratch,
        "pre",
        &payload(&scratch, "pre")?.to_string(),
        &in_rig,
    )?;
    assert_eq!(waiting_texts(&ledger)?.len(), 2, "both asks are waiting");
    run_hook_ok(
        &scratch,
        "post",
        &payload(&scratch, "post")?.to_string(),
        &in_rig,
    )?;

    // The person's answers are recorded, not refused, and the asks they answer leave waiting.
    assert_eq!(
        events_of(&ledger, EventType::DecisionAccepted)?.len(),
        2,
        "one decision per answer: {}",
        hook_log(&scratch)
    );
    assert!(
        waiting_texts(&ledger)?.is_empty(),
        "answered, so nothing is waiting"
    );
    for title in [DATABASE_TITLE, FEATURES_TITLE] {
        assert_eq!(
            topic_keys_of(&ledger, title)?,
            [FALLBACK_TOPIC],
            "filed under the one fixed key, not a key per question"
        );
    }
    // The vocabulary grew by exactly that key, once, though both answers used it.
    assert_eq!(
        declared_topics(&ledger, "billing")?,
        ["invoicing", FALLBACK_TOPIC]
    );

    // Both attempts are logged, for each question: the refused header key, then the fixed key.
    let log = hook_log(&scratch);
    for header_key in ["'database'", "'features'"] {
        assert!(
            log.contains(&format!("does not declare topic {header_key}")),
            "the refused attempt is logged: {log}"
        );
    }
    assert_eq!(
        log.matches("under 'claude-code-question'").count(),
        4,
        "each answer logs the retry and its success: {log}"
    );
    Ok(())
}

#[test]
fn a_header_the_project_declared_is_used_as_is_and_nothing_is_declared() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    register_rig_project(&ledger, "billing", &["database", "features"])?;
    let in_rig = [("GC_RIG", RIG)];

    run_hook_ok(
        &scratch,
        "pre",
        &payload(&scratch, "pre")?.to_string(),
        &in_rig,
    )?;
    run_hook_ok(
        &scratch,
        "post",
        &payload(&scratch, "post")?.to_string(),
        &in_rig,
    )?;

    assert_eq!(events_of(&ledger, EventType::DecisionAccepted)?.len(), 2);
    assert!(waiting_texts(&ledger)?.is_empty());
    assert_eq!(topic_keys_of(&ledger, DATABASE_TITLE)?, ["database"]);
    assert_eq!(topic_keys_of(&ledger, FEATURES_TITLE)?, ["features"]);
    assert_eq!(
        declared_topics(&ledger, "billing")?,
        ["database", "features"],
        "the hooks declared nothing: only the operator's two declarations exist"
    );
    assert!(
        !hook_log(&scratch).contains(FALLBACK_TOPIC),
        "no retry happened: {}",
        hook_log(&scratch)
    );
    Ok(())
}

#[test]
fn a_checkout_no_project_is_anchored_to_files_under_the_header_and_declares_nothing(
) -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    // The ledger has a registered project with a vocabulary, but this checkout is not in its rig:
    // the writes fall back to the personal project, which has no vocabulary.
    register_rig_project(&ledger, "billing", &["invoicing"])?;

    run_hook_ok(&scratch, "pre", &payload(&scratch, "pre")?.to_string(), &[])?;
    run_hook_ok(
        &scratch,
        "post",
        &payload(&scratch, "post")?.to_string(),
        &[],
    )?;

    assert_eq!(events_of(&ledger, EventType::DecisionAccepted)?.len(), 2);
    assert!(waiting_texts(&ledger)?.is_empty());
    assert_eq!(topic_keys_of(&ledger, DATABASE_TITLE)?, ["database"]);
    assert_eq!(topic_keys_of(&ledger, FEATURES_TITLE)?, ["features"]);
    assert_eq!(
        declared_topics(&ledger, "billing")?,
        ["invoicing"],
        "nothing was declared, here or anywhere"
    );
    assert!(
        !hook_log(&scratch).contains(FALLBACK_TOPIC),
        "no retry happened: {}",
        hook_log(&scratch)
    );
    Ok(())
}

#[test]
fn a_refused_note_and_an_undeclared_header_are_each_retried_once_without_a_duplicate(
) -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    register_rig_project(&ledger, "billing", &["invoicing"])?;

    // One answer with a note that reads on its own, one with a note too short to be a rationale:
    // the second is refused twice (the note, then the header's key) and still lands once.
    let note = "SQLite cannot be shared between the two demo hosts, so it has to be a server.";
    let mut post = payload(&scratch, "post")?;
    for source in ["tool_input", "tool_response"] {
        post[source]["annotations"][DATABASE_QUESTION] = serde_json::json!({ "notes": note });
        post[source]["annotations"][FEATURES_QUESTION] =
            serde_json::json!({ "notes": "and Notion" });
    }
    run_hook_ok(&scratch, "post", &post.to_string(), &[("GC_RIG", RIG)])?;

    assert_eq!(
        events_of(&ledger, EventType::DecisionAccepted)?.len(),
        2,
        "one decision per answer, however many refused attempts came first: {}",
        hook_log(&scratch)
    );
    let database = why_for(&ledger, DATABASE_TITLE)?;
    assert_eq!(rationale(&database)?, note);
    let features = why_for(&ledger, FEATURES_TITLE)?;
    assert_eq!(
        features["quote"], "Import from Notion\n\nNote: and Notion",
        "{features}"
    );
    for title in [DATABASE_TITLE, FEATURES_TITLE] {
        assert_eq!(topic_keys_of(&ledger, title)?, [FALLBACK_TOPIC]);
    }
    assert_eq!(
        declared_topics(&ledger, "billing")?,
        ["invoicing", FALLBACK_TOPIC]
    );
    Ok(())
}

#[test]
fn a_refusal_that_is_not_about_topics_is_not_retried() -> TestResult<()> {
    let scratch = Scratch::new()?;
    let ledger = scratch.ledger();
    // The ledger's project is anchored to another rig, so a capture from this one is the write
    // layer's wrong-ledger refusal: nothing to do with the topic key, so no second attempt.
    register_rig_project(&ledger, "billing", &["invoicing"])?;

    run_hook_ok(
        &scratch,
        "post",
        &payload(&scratch, "post")?.to_string(),
        &[("GC_RIG", "some-other-rig")],
    )?;

    assert!(events_of(&ledger, EventType::DecisionProposed)?.is_empty());
    assert_eq!(declared_topics(&ledger, "billing")?, ["invoicing"]);
    let log = hook_log(&scratch);
    assert!(
        log.contains("post: could not record the answer") && log.contains("some-other-rig"),
        "the refusal is logged: {log}"
    );
    assert!(
        !log.contains(FALLBACK_TOPIC),
        "it was not retried under the fixed key: {log}"
    );
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
