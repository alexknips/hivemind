//! `hivemind ground` (hivemind-gwhr.4): give an existing decision what it rests on, after the
//! fact, attributed to whoever runs it. CLI behaviour, the refusals that must write nothing, and
//! the hivemind-context plugin script that wraps the verb.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::{Event, EventType};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::Value;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CAPTURER: &str = "human:alice";
const GROUNDER: &str = "agent:claude:grounder";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-ground-{label}-{}", Uuid::new_v4()));
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

/// Capture a decision the way every pre-grounding decision was captured: the raw verb, which
/// never asks what it rests on.
fn capture_raw(dir: &Path, title: &str) -> TestResult<String> {
    let output = run_cli(
        dir,
        CAPTURER,
        true,
        &[
            "emit",
            "decision.proposed",
            "--title",
            title,
            "--rationale",
            "Rationale text long enough for the readable floor",
            "--topic-keys",
            "grounding",
            "--options",
            "adopt,skip",
            "--chose",
            "adopt",
        ],
    )?;
    let envelope: Value = serde_json::from_str(&output)?;
    Ok(envelope["value"]
        .as_str()
        .ok_or("emit should return the decision id")?
        .to_owned())
}

fn ground(dir: &Path, args: &[&str]) -> hivemind::Result<String> {
    let mut full = vec!["ground"];
    full.extend_from_slice(args);
    run_cli(dir, GROUNDER, true, &full)
}

fn events(dir: &Path) -> TestResult<Vec<Event>> {
    Ok(SqliteEventLedger::open(dir)?.read(0, 1000)?)
}

fn field<'a>(event: &'a Event, key: &str) -> &'a str {
    event
        .payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `(relation, from_id, to_id)` of every `relation.added` event after the first `skip` events.
fn relations_after(dir: &Path, skip: usize) -> TestResult<Vec<(String, String, String)>> {
    Ok(events(dir)?
        .iter()
        .skip(skip)
        .filter(|event| event.event_type == EventType::RelationAdded)
        .map(|event| {
            (
                field(event, "relation").to_owned(),
                field(event, "from_id").to_owned(),
                field(event, "to_id").to_owned(),
            )
        })
        .collect())
}

fn assert_refused_writing_nothing(
    dir: &Path,
    args: &[&str],
    expected: &str,
) -> TestResult<hivemind::HivemindError> {
    let before = events(dir)?.len();
    let error = ground(dir, args).expect_err("the call must be refused");
    assert!(
        error.to_string().contains(expected),
        "expected `{expected}` in: {error}"
    );
    assert_eq!(
        events(dir)?.len(),
        before,
        "a refused ground call writes nothing"
    );
    Ok(error)
}

#[test]
fn ground_gives_a_decision_captured_with_nothing_declared_what_it_rests_on() -> TestResult<()> {
    let scratch = Scratch::new("end-to-end")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;
    let premise_id = capture_raw(dir, "Keep audit boundaries")?;
    let already = events(dir)?.len();

    let output = ground(
        dir,
        &[
            "shared admin token",
            "--rests-on-decision",
            "audit boundaries",
            "--rests-on-evidence",
            "the shared token leaked twice",
            "--evidence-source",
            "incident 7",
            "--bet",
            "scoped tokens are enough",
            "--would-change-if",
            "a scope leaks",
            "--check-by",
            "2026-12-01",
        ],
    )?;
    let reply: Value = serde_json::from_str(&output)?;
    assert_eq!(reply["decision_id"], target_id.as_str());
    assert_eq!(reply["decision_title"], "Adopt the shared admin token");
    assert_eq!(reply["actor_id"], GROUNDER);
    let rests_on: Vec<(&str, &str)> = reply["rests_on"]
        .as_array()
        .ok_or("rests_on array")?
        .iter()
        .map(|item| {
            (
                item["kind"].as_str().unwrap_or_default(),
                item["label"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        rests_on,
        vec![
            ("decision", "Keep audit boundaries"),
            ("evidence", "the shared token leaked twice"),
            ("bet", "scoped tokens are enough"),
        ]
    );

    // Everything the call wrote names the grounder, not the capturer, and links to nothing that
    // caused it: that is what tells "added later" from "at capture".
    let appended: Vec<Event> = events(dir)?.into_iter().skip(already).collect();
    assert!(appended.iter().all(|event| event.actor_id == GROUNDER));
    assert!(appended
        .iter()
        .all(|event| event.causation_event_id.is_none()));
    let edges = relations_after(dir, already)?;
    assert!(edges.contains(&(
        "FOLLOWS_FROM".to_owned(),
        target_id.clone(),
        premise_id.clone()
    )));
    assert_eq!(
        edges
            .iter()
            .filter(|(relation, from, _)| relation == "BASED_ON" && *from == target_id)
            .count(),
        1
    );
    assert_eq!(
        edges
            .iter()
            .filter(|(relation, _, _)| relation == "ASSUMES")
            .count(),
        1
    );
    let bet = appended
        .iter()
        .find(|event| event.event_type == EventType::HypothesisRecorded)
        .ok_or("the bet is recorded as a hypothesis")?;
    assert_eq!(field(bet, "kind"), "bet");
    assert_eq!(field(bet, "would_change_if"), "a scope leaks");

    // The read side tells the story the events tell: the decision was captured with nothing
    // declared, and every premise is attributed later, by the grounder.
    let verified: Value = serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &["query", "verify", "shared admin token"],
    )?)?;
    assert_eq!(verified["data"]["grounding_state"], "grounded");
    let items = verified["data"]["rests_on"]
        .as_array()
        .ok_or("verify lists what the decision rests on")?;
    assert_eq!(items.len(), 3, "premise, evidence and bet: {items:?}");
    for item in items {
        assert_eq!(item["added"]["when"], "later", "{item}");
        assert_eq!(item["added"]["actor_id"], GROUNDER, "{item}");
    }
    let text = run_cli(
        dir,
        CAPTURER,
        false,
        &["query", "--summary", "verify", "shared admin token"],
    )?;
    assert!(
        text.contains(&format!("attributed later by {GROUNDER} on ")),
        "verify names who attributed the premise and when: {text}"
    );
    Ok(())
}

#[test]
fn ground_text_reply_lists_what_was_added_with_labels() -> TestResult<()> {
    let scratch = Scratch::new("text")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;
    let premise_id = capture_raw(dir, "Keep audit boundaries")?;
    // The capturer self-accepted it, so a different actor rejects it.
    run_cli(
        dir,
        "human:bob",
        false,
        &["emit", "decision.rejected", "--decision-id", &premise_id],
    )?;

    let output = run_cli(
        dir,
        GROUNDER,
        false,
        &[
            "ground",
            "--id",
            &target_id,
            "--rests-on-decision",
            &premise_id,
            "--rests-on-assumption",
            "audit logs stay append-only",
        ],
    )?;

    assert!(
        output.starts_with(&format!(
            "grounded {target_id} \"Adopt the shared admin token\": added 2 thing(s) it rests on, attributed to {GROUNDER}"
        )),
        "{output}"
    );
    assert!(
        output.contains(&format!(
            "decision \"Keep audit boundaries\" ({premise_id})"
        )),
        "{output}"
    );
    assert!(
        output.contains("assumption \"audit logs stay append-only\""),
        "{output}"
    );
    assert!(
        output.contains(&format!("premise_stale: {premise_id}")),
        "a rejected premise is recorded and reported, never silent: {output}"
    );
    Ok(())
}

#[test]
fn ground_takes_existing_evidence_and_hypotheses_by_id() -> TestResult<()> {
    let scratch = Scratch::new("existing-nodes")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;
    let evidence: Value = serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &["emit", "evidence.recorded", "--content", "the token leaked"],
    )?)?;
    let evidence_id = evidence["value"].as_str().ok_or("evidence id")?.to_owned();
    let hypothesis: Value = serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &[
            "emit",
            "hypothesis.recorded",
            "--statement",
            "scoped tokens are enough",
        ],
    )?)?;
    let hypothesis_id = hypothesis["value"]
        .as_str()
        .ok_or("hypothesis id")?
        .to_owned();
    let already = events(dir)?.len();

    ground(
        dir,
        &[
            "--id",
            &target_id,
            "--evidence",
            &evidence_id,
            "--hypotheses",
            &hypothesis_id,
        ],
    )?;

    let edges = relations_after(dir, already)?;
    assert!(edges.contains(&(
        "BASED_ON".to_owned(),
        target_id.clone(),
        evidence_id.clone()
    )));
    assert!(edges
        .iter()
        .any(|(relation, _, to)| relation == "ASSUMES" && *to == hypothesis_id));
    assert_eq!(edges.len(), 2, "no new nodes: only the two links");
    Ok(())
}

#[test]
fn ground_refuses_a_call_that_names_nothing() -> TestResult<()> {
    let scratch = Scratch::new("nothing")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;

    assert_refused_writing_nothing(dir, &["--id", &target_id], "nothing to ground")?;
    Ok(())
}

#[test]
fn ground_refuses_confidence_because_it_is_the_deciders_words_at_capture() -> TestResult<()> {
    let scratch = Scratch::new("confidence")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;

    assert_refused_writing_nothing(
        dir,
        &["--id", &target_id, "--bet", "--confidence", "high"],
        "cannot be added to a decision that already exists",
    )?;
    Ok(())
}

#[test]
fn ground_ambiguous_target_returns_candidates_and_writes_nothing_until_picked() -> TestResult<()> {
    let scratch = Scratch::new("ambiguous-target")?;
    let dir = scratch.path();
    let first = capture_raw(dir, "Adopt the queue")?;
    let second = capture_raw(dir, "Adopt the queue")?;
    let before = events(dir)?.len();

    let listing: Value = serde_json::from_str(&ground(dir, &["Adopt the queue", "--bet"])?)?;
    assert_eq!(listing["data"]["outcome"], "ambiguous");
    assert_eq!(
        listing["data"]["candidates"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(events(dir)?.len(), before, "ambiguity is data, not a write");

    let picked: Value =
        serde_json::from_str(&ground(dir, &["Adopt the queue", "--bet", "--pick", "1"])?)?;
    let picked_id = picked["decision_id"].as_str().ok_or("decision_id")?;
    assert!(
        picked_id == first || picked_id == second,
        "--pick settles which candidate: {picked_id}"
    );
    assert_eq!(picked["rests_on"][0]["kind"], "bet");
    Ok(())
}

#[test]
fn ground_unmatched_target_is_reported_and_writes_nothing() -> TestResult<()> {
    let scratch = Scratch::new("unmatched-target")?;
    let dir = scratch.path();
    capture_raw(dir, "Adopt the queue")?;
    let before = events(dir)?.len();

    let listing: Value = serde_json::from_str(&ground(dir, &["Zebra migration", "--bet"])?)?;
    assert_eq!(listing["data"]["outcome"], "not_found");
    assert_eq!(events(dir)?.len(), before);
    Ok(())
}

#[test]
fn ground_ambiguous_or_unmatched_premise_refuses_and_a_hash_handle_settles_it() -> TestResult<()> {
    let scratch = Scratch::new("premise")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;
    let first = capture_raw(dir, "Adopt the queue")?;
    let second = capture_raw(dir, "Adopt the queue")?;

    // A new assumption is named alongside: a refusal on the premise must not strand it.
    assert_refused_writing_nothing(
        dir,
        &[
            "--id",
            &target_id,
            "--rests-on-assumption",
            "must not be stranded",
            "--rests-on-decision",
            "Adopt the queue",
        ],
        "none is clearly the one",
    )?;
    assert_refused_writing_nothing(
        dir,
        &[
            "--id",
            &target_id,
            "--rests-on-assumption",
            "must not be stranded",
            "--rests-on-decision",
            "Zebra migration",
        ],
        "no decision matches 'Zebra migration'",
    )?;

    // The ambiguous refusal left its numbered candidates for `#N`; re-listing them first.
    assert_refused_writing_nothing(
        dir,
        &["--id", &target_id, "--rests-on-decision", "Adopt the queue"],
        "re-run with --rests-on-decision '#N'",
    )?;
    let already = events(dir)?.len();
    ground(dir, &["--id", &target_id, "--rests-on-decision", "#2"])?;
    let edges = relations_after(dir, already)?;
    assert_eq!(edges.len(), 1);
    let (relation, from, to) = &edges[0];
    assert_eq!(relation, "FOLLOWS_FROM");
    assert_eq!(*from, target_id);
    assert!(
        *to == first || *to == second,
        "`#2` settled which queue decision: {to}"
    );
    Ok(())
}

#[test]
fn ground_refuses_a_premise_that_would_close_a_loop_or_names_itself() -> TestResult<()> {
    let scratch = Scratch::new("cycle")?;
    let dir = scratch.path();
    let datastore = capture_raw(dir, "Choose the datastore")?;
    let index_layout = capture_raw(dir, "Choose the index layout")?;
    let review = capture_raw(dir, "Review the index layout")?;

    // index layout rests on the datastore, and the review rests on the index layout.
    ground(
        dir,
        &[
            "--id",
            &index_layout,
            "--rests-on-decision",
            "Choose the datastore",
        ],
    )?;
    ground(
        dir,
        &[
            "--id",
            &review,
            "--rests-on-decision",
            "Choose the index layout",
        ],
    )?;

    // The datastore resting on the review — through the index layout — would close a loop.
    let error = assert_refused_writing_nothing(
        dir,
        &[
            "--id",
            &datastore,
            "--rests-on-assumption",
            "must not be stranded",
            "--rests-on-decision",
            "Review the index layout",
        ],
        "would close a loop",
    )?;
    let message = error.to_string();
    assert!(
        message.contains("Choose the index layout") && message.contains("Choose the datastore"),
        "the refusal names the chain: {message}"
    );

    assert_refused_writing_nothing(
        dir,
        &[
            "--id",
            &datastore,
            "--rests-on-decision",
            "Choose the datastore",
        ],
        "cannot be its own premise",
    )?;
    Ok(())
}

#[test]
fn ground_refuses_a_premise_handle_that_a_replaced_candidate_list_would_change() -> TestResult<()> {
    let scratch = Scratch::new("handle")?;
    let dir = scratch.path();
    capture_raw(dir, "Adopt the queue")?;
    capture_raw(dir, "Adopt the queue")?;
    let premise_a = capture_raw(dir, "Adopt the cache")?;
    let premise_b = capture_raw(dir, "Adopt the cache")?;

    // An ambiguous premise leaves candidate list L0; `#2` names its second entry.
    let target_probe = capture_raw(dir, "Probe target")?;
    assert_refused_writing_nothing(
        dir,
        &[
            "--id",
            &target_probe,
            "--rests-on-decision",
            "Adopt the cache",
        ],
        "none is clearly the one",
    )?;

    // Now the *target* is ambiguous too: its listing replaces L0, so a re-run that kept `#2`
    // would silently mean a different decision. The call refuses and names what `#2` was.
    let error = assert_refused_writing_nothing(
        dir,
        &["Adopt the queue", "--rests-on-decision", "#2"],
        "replaced the one your `#N` premise handle(s) pointed at",
    )?;
    let message = error.to_string();
    assert!(
        message.contains(&format!("'#2' = {premise_a}"))
            || message.contains(&format!("'#2' = {premise_b}")),
        "the refusal says which decision `#2` meant: {message}"
    );
    Ok(())
}

#[test]
fn ground_script_wraps_the_verb_and_joins_an_unquoted_description() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = Scratch::new("script")?;
    let dir = scratch.path();
    let target_id = capture_raw(dir, "Adopt the shared admin token")?;
    let premise_id = capture_raw(dir, "Keep audit boundaries")?;
    let already = events(dir)?.len();
    let script = root.join("plugins/hivemind-context/scripts/ground.sh");

    let run_script = |args: &[&str]| -> TestResult<std::process::Output> {
        Ok(Command::new(&script)
            .current_dir(root)
            .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
            .env("HIVEMIND_DIR", dir)
            .env("CLAUDE_PROJECT_DIR", root)
            .env("CLAUDE_SESSION_ID", "plugin-ground-session")
            .env_remove("GC_AGENT")
            .env_remove("GC_ALIAS")
            .args(args)
            .output()?)
    };

    let help = run_script(&["--help"])?;
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stderr).contains("ground.sh"));

    // A description typed as bare words, no quotes: the plugin joins it into one argument.
    let grounded = run_script(&[
        "shared",
        "admin",
        "token",
        "--rests-on-decision",
        "audit boundaries",
    ])?;
    assert!(
        grounded.status.success(),
        "ground.sh failed: {}",
        String::from_utf8_lossy(&grounded.stderr)
    );
    let stdout = String::from_utf8_lossy(&grounded.stdout);
    assert!(
        stdout.starts_with(&format!("grounded {target_id}")),
        "{stdout}"
    );

    let appended: Vec<Event> = events(dir)?.into_iter().skip(already).collect();
    assert!(
        appended
            .iter()
            .all(|event| event.actor_id == "agent:claude:plugin-ground-session"),
        "the script records agent provenance, not the git-derived human default"
    );
    assert!(relations_after(dir, already)?.contains(&(
        "FOLLOWS_FROM".to_owned(),
        target_id.clone(),
        premise_id.clone()
    )));

    // An ambiguity or a refusal is not a success: nothing more is written.
    let before = events(dir)?.len();
    let refused = run_script(&["shared", "admin", "token"])?;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("nothing to ground"));
    assert_eq!(events(dir)?.len(), before);
    Ok(())
}
