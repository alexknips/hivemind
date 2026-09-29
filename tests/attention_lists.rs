//! The attention lists and the per-decision timeline (hivemind-bbnw.7): what is waiting, what is
//! contested, what changed, and when each thing happened, read through the CLI the way an agent or
//! a person would.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use hivemind::cli::{run, Cli};
use serde_json::Value;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const ASKER: &str = "human:alice";
const CAPTURER: &str = "agent:claude:capturer";
const REVIEWER: &str = "human:bob";
const QUESTION: &str = "Which storage engine should the prototype use?";
const OTHER_QUESTION: &str = "When should the prototype ship?";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let path =
            std::env::temp_dir().join(format!("hivemind-attention-{label}-{}", Uuid::new_v4()));
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

fn json_cli(dir: &Path, actor: &str, args: &[&str]) -> TestResult<Value> {
    Ok(serde_json::from_str(&run_cli(dir, actor, true, args)?)?)
}

fn ask(dir: &Path, text: &str) -> TestResult<String> {
    let asked = json_cli(dir, ASKER, &["ask", text])?;
    Ok(asked["request_id"]
        .as_str()
        .ok_or("ask returns request_id")?
        .to_owned())
}

/// A capture that chose `chose` from fixed options, answering `request_id`'s question.
fn capture_answers(dir: &Path, title: &str, chose: &str, request_id: &str) -> TestResult<String> {
    let captured = json_cli(
        dir,
        CAPTURER,
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
    )?;
    Ok(captured["value"]
        .as_str()
        .ok_or("capture returns the decision id")?
        .to_owned())
}

fn list(dir: &Path, command: &str, extra: &[&str]) -> TestResult<Value> {
    let mut args = vec!["query", command];
    args.extend_from_slice(extra);
    json_cli(dir, ASKER, &args)
}

fn items(list: &Value) -> TestResult<&Vec<Value>> {
    Ok(list["data"]["items"]
        .as_array()
        .ok_or("the list has an items array")?)
}

fn decision_ids(list: &Value) -> TestResult<Vec<String>> {
    Ok(items(list)?
        .iter()
        .map(|item| item["decision_id"].as_str().unwrap_or_default().to_owned())
        .collect())
}

fn timeline_kinds(why: &Value) -> Vec<String> {
    why["data"]["timeline"]["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["kind"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_lists_and_the_timeline_follow_a_decision_through_its_life() -> TestResult<()> {
    let scratch = Scratch::new("life")?;
    let dir = scratch.path();

    // Two asks; only the first is answered.
    let first_request = ask(dir, QUESTION)?;
    let second_request = ask(dir, OTHER_QUESTION)?;
    let sqlite = capture_answers(dir, "Use SQLite", "sqlite", &first_request)?;

    let waiting = list(dir, "get_waiting_requests", &[])?;
    let waiting_items = items(&waiting)?;
    assert_eq!(waiting_items.len(), 1, "only the unanswered ask waits");
    assert_eq!(waiting_items[0]["request_id"], second_request);
    assert_eq!(waiting_items[0]["requested_by"], ASKER);
    assert!(waiting_items[0]["asked_at"].is_string());
    assert!(items(&list(dir, "get_contested_decisions", &[])?)?.is_empty());
    assert!(items(&list(dir, "get_changed_decisions", &[])?)?.is_empty());

    // A second accepted answer that chooses differently: the two conflict, each naming the other.
    let postgres = capture_answers(dir, "Use Postgres", "postgres", &first_request)?;
    let contested = list(dir, "get_contested_decisions", &[])?;
    assert_eq!(
        decision_ids(&contested)?,
        vec![sqlite.clone(), postgres.clone()]
    );
    for (item, other) in items(&contested)?.iter().zip([&postgres, &sqlite]) {
        assert_eq!(item["contest"]["kind"], "conflicting_answers");
        assert_eq!(item["contest"]["question"], QUESTION);
        assert_eq!(item["contest"]["conflicts_with"][0]["decision_id"], *other);
    }

    // Someone rejects the first: it is now a disagreement, and no longer a conflicting answer.
    run_cli(
        dir,
        REVIEWER,
        true,
        &[
            "disagree",
            "--decision",
            &sqlite,
            "--reason",
            "Concurrent writers need row-level locking SQLite does not offer",
        ],
    )?;
    let contested = list(dir, "get_contested_decisions", &[])?;
    assert_eq!(decision_ids(&contested)?, vec![sqlite.clone()]);
    let contest = &items(&contested)?[0]["contest"];
    assert_eq!(contest["kind"], "disagreement");
    assert_eq!(contest["accepted_by"][0], CAPTURER);
    assert_eq!(contest["rejected_by"][0], REVIEWER);

    // A retitle and a supersession: the decision leaves the contested list and shows as changed.
    run_cli(
        dir,
        CAPTURER,
        true,
        &[
            "retitle",
            "--decision",
            &sqlite,
            "--to",
            "Use SQLite for the prototype",
            "--reason",
            "scope",
        ],
    )?;
    run_cli(
        dir,
        CAPTURER,
        true,
        &[
            "supersede",
            "--old",
            &sqlite,
            "--title",
            "Use Postgres for concurrent writers",
            "--rationale",
            "Concurrent writers need row-level locking that SQLite does not offer",
            "--topic-keys",
            "storage",
            "--options",
            "sqlite,postgres",
            "--chose",
            "postgres",
            "--bet",
        ],
    )?;
    assert!(items(&list(dir, "get_contested_decisions", &[])?)?.is_empty());

    let changed = list(dir, "get_changed_decisions", &[])?;
    assert_eq!(decision_ids(&changed)?, vec![sqlite.clone()]);
    assert_eq!(changed["data"]["total_matches"], 1);
    assert_eq!(items(&changed)?[0]["status"], "superseded");
    assert_eq!(items(&changed)?[0]["title"], "Use SQLite for the prototype");
    let change_kinds: Vec<&str> = items(&changed)?[0]["changes"]
        .as_array()
        .ok_or("changes")?
        .iter()
        .filter_map(|change| change["kind"].as_str())
        .collect();
    assert_eq!(change_kinds, vec!["retitled", "superseded"]);
    assert!(
        changed["data"]["since"].is_string(),
        "the window the list used is echoed: {changed}"
    );

    // The same decision's own timeline says the whole story, each entry cited by its event.
    let why = json_cli(dir, CAPTURER, &["query", "why", "--id", &sqlite])?;
    assert_eq!(
        timeline_kinds(&why),
        vec![
            "asked",
            "recorded",
            "accepted",
            "rejected",
            "retitled",
            "superseded"
        ]
    );
    let timeline = &why["data"]["timeline"];
    assert_eq!(timeline["decision_id"], sqlite);
    assert!(timeline["asked_at"].is_string());
    assert!(timeline["decided_at"].is_string());
    assert!(timeline["asked_to_decided_seconds"]
        .as_i64()
        .is_some_and(|s| s >= 0));
    let entries = timeline["entries"].as_array().ok_or("entries")?;
    assert_eq!(entries[0]["actor_id"], ASKER);
    assert_eq!(entries[3]["actor_id"], REVIEWER);
    assert!(entries.iter().all(|entry| entry["citation_id"]
        .as_str()
        .is_some_and(|c| c.starts_with("event:"))));
    Ok(())
}

#[test]
fn a_decision_nobody_asked_about_has_no_ask_and_no_duration() -> TestResult<()> {
    let scratch = Scratch::new("unasked")?;
    let dir = scratch.path();
    let request = ask(dir, QUESTION)?;
    let asked = capture_answers(dir, "Use SQLite", "sqlite", &request)?;
    let captured = json_cli(
        dir,
        CAPTURER,
        &[
            "emit",
            "decision.capture",
            "--bet",
            "--title",
            "Ship in October",
            "--rationale",
            "Rationale text long enough for the readable floor",
            "--topic-keys",
            "release",
            "--options",
            "october,november",
            "--chose",
            "october",
        ],
    )?;
    let unasked = captured["value"].as_str().ok_or("decision id")?;

    let why = json_cli(dir, CAPTURER, &["query", "why", "--id", unasked])?;

    assert_eq!(timeline_kinds(&why), vec!["recorded", "accepted"]);
    assert!(why["data"]["timeline"]["asked_at"].is_null());
    assert!(why["data"]["timeline"]["asked_to_decided_seconds"].is_null());
    assert!(why["data"]["timeline"]["decided_at"].is_string());
    // The answered decision still shows its ask.
    let answered = json_cli(dir, CAPTURER, &["query", "why", "--id", &asked])?;
    assert_eq!(timeline_kinds(&answered)[0], "asked");
    Ok(())
}

#[test]
fn the_changed_list_bounds_its_window_and_pages_without_repeating() -> TestResult<()> {
    let scratch = Scratch::new("paging")?;
    let dir = scratch.path();
    let request = ask(dir, QUESTION)?;
    let first = capture_answers(dir, "Use SQLite", "sqlite", &request)?;
    let second = capture_answers(dir, "Use SQLite too", "sqlite", &request)?;
    for (id, title) in [(&first, "SQLite for now"), (&second, "SQLite for good")] {
        run_cli(
            dir,
            CAPTURER,
            true,
            &["retitle", "--decision", id, "--to", title],
        )?;
    }

    let after = list(dir, "get_changed_decisions", &["--since", "2999-01-01"])?;
    assert!(items(&after)?.is_empty());
    assert_eq!(after["data"]["total_matches"], 0);
    let before = list(dir, "get_changed_decisions", &["--until", "1970-01-01"])?;
    assert!(items(&before)?.is_empty());
    let whole = list(dir, "get_changed_decisions", &["--since", "1970-01-01"])?;
    assert_eq!(items(&whole)?.len(), 2);

    let page_one = list(dir, "get_changed_decisions", &["--limit", "1"])?;
    assert_eq!(page_one["truncated"], true);
    assert_eq!(
        decision_ids(&page_one)?,
        vec![second.clone()],
        "newest change first"
    );
    assert_eq!(page_one["data"]["total_matches"], 2);
    let cursor = page_one["data"]["next_cursor"]
        .as_str()
        .ok_or("a truncated page names its cursor")?
        .to_owned();

    let page_two = list(
        dir,
        "get_changed_decisions",
        &["--limit", "1", "--cursor", &cursor],
    )?;
    assert_eq!(page_two["truncated"], false);
    assert_eq!(decision_ids(&page_two)?, vec![first]);
    assert!(page_two["data"]["next_cursor"].is_null());
    Ok(())
}

#[test]
fn the_summaries_read_as_text() -> TestResult<()> {
    let scratch = Scratch::new("summary")?;
    let dir = scratch.path();
    let request = ask(dir, QUESTION)?;
    ask(dir, OTHER_QUESTION)?;
    let sqlite = capture_answers(dir, "Use SQLite", "sqlite", &request)?;
    run_cli(
        dir,
        REVIEWER,
        true,
        &[
            "disagree",
            "--decision",
            &sqlite,
            "--reason",
            "Not enough for concurrent writers",
        ],
    )?;
    run_cli(
        dir,
        CAPTURER,
        true,
        &["retitle", "--decision", &sqlite, "--to", "SQLite for now"],
    )?;

    let text = |args: &[&str]| run_cli(dir, ASKER, false, args);

    let contested = text(&["query", "--summary", "get_contested_decisions"])?;
    assert!(
        contested.contains(&format!("contested\t{sqlite}")),
        "{contested}"
    );
    assert!(
        contested.contains(&format!("rejected_by={REVIEWER}")),
        "{contested}"
    );

    let changed = text(&["query", "--summary", "get_changed_decisions"])?;
    assert!(changed.contains(&format!("changed\t{sqlite}")), "{changed}");
    assert!(changed.contains("retitled"), "{changed}");

    let waiting = text(&["query", "--summary", "get_waiting_requests"])?;
    assert!(waiting.contains(OTHER_QUESTION), "{waiting}");

    let why = text(&["query", "--summary", "why", "--id", &sqlite])?;
    assert!(why.contains("timeline:"), "{why}");
    assert!(why.contains("asked  by human:alice"), "{why}");
    assert!(why.contains("rejected  by human:bob"), "{why}");
    assert!(why.contains("asked to decided:"), "{why}");

    let empty = Scratch::new("summary-empty")?;
    assert_eq!(
        run_cli(
            empty.path(),
            ASKER,
            false,
            &["query", "--summary", "get_contested_decisions"]
        )?,
        "No contested decisions found"
    );
    assert_eq!(
        run_cli(
            empty.path(),
            ASKER,
            false,
            &["query", "--summary", "get_changed_decisions"]
        )?,
        "No changed decisions found"
    );
    Ok(())
}
