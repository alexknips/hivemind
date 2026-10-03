//! A decision answers a question (hivemind-zdsh.16): `--question` at capture names or creates the
//! question node, decisions that answer one question share it, two accepted answers that choose
//! differently are a visible conflict, and `ground --answers` backfills the question afterwards.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use hivemind::cli::{run, Cli};
use hivemind::events::{Event, EventType};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::Value;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CAPTURER: &str = "human:alice";
const GROUNDER: &str = "agent:claude:grounder";
const QUESTION: &str = "Which storage engine should the prototype use?";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let path =
            std::env::temp_dir().join(format!("hivemind-question-{label}-{}", Uuid::new_v4()));
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

/// A capture that chose `chose` from `options`, optionally naming the question it answers.
fn capture(dir: &Path, title: &str, chose: &str, question: Option<&str>) -> TestResult<Value> {
    let mut args = vec![
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
    ];
    if let Some(question) = question {
        args.extend(["--question", question]);
    }
    Ok(serde_json::from_str(&run_cli(dir, CAPTURER, true, &args)?)?)
}

fn events(dir: &Path) -> TestResult<Vec<Event>> {
    Ok(SqliteEventLedger::open(dir)?.read(0, 1000)?)
}

fn question_events(dir: &Path) -> TestResult<usize> {
    Ok(events(dir)?
        .iter()
        .filter(|event| event.event_type == EventType::QuestionRecorded)
        .count())
}

/// `verify` for one decision, by id: two decisions that answer one question tend to weigh the
/// same options, so a description would not tell them apart.
fn verify_json(dir: &Path, decision_id: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &["query", "verify", "--id", decision_id],
    )?)?)
}

fn verify_text(dir: &Path, decision_id: &str) -> TestResult<String> {
    Ok(run_cli(
        dir,
        CAPTURER,
        false,
        &["query", "--summary", "verify", "--id", decision_id],
    )?)
}

fn id_of(capture: &Value) -> TestResult<&str> {
    Ok(capture["value"]
        .as_str()
        .ok_or("capture returns the decision id")?)
}

#[test]
fn two_captures_with_the_same_question_share_one_node_and_a_conflict_is_visible() -> TestResult<()>
{
    let scratch = Scratch::new("conflict")?;
    let dir = scratch.path();

    let sqlite = capture(dir, "Prototype storage is SQLite", "sqlite", Some(QUESTION))?;
    let postgres = capture(
        dir,
        "Prototype storage is Postgres",
        "postgres",
        Some("which storage engine should the PROTOTYPE use"),
    )?;

    let question_id = sqlite["question_id"]
        .as_str()
        .ok_or("capture names the question node")?;
    assert!(question_id.starts_with("question-"), "{question_id}");
    assert_eq!(
        postgres["question_id"], sqlite["question_id"],
        "one question, one node, whatever the spelling"
    );
    assert_eq!(question_events(dir)?, 1);

    let brief = verify_json(dir, id_of(&sqlite)?)?;
    assert_eq!(brief["data"]["question_id"], question_id);
    assert_eq!(brief["data"]["question"], QUESTION);
    let others = brief["data"]["other_answers"]
        .as_array()
        .ok_or("the brief lists the other answers")?;
    assert_eq!(others.len(), 1, "{others:?}");
    assert_eq!(others[0]["decision_id"], postgres["value"]);
    assert_eq!(others[0]["title"], "Prototype storage is Postgres");
    assert_eq!(others[0]["status"], "accepted");
    assert_eq!(others[0]["chosen_option"], "postgres");

    // Two accepted answers that chose differently: visible on both, resolved on neither.
    let reasons = brief["data"]["still_holds"]["reasons"]
        .as_array()
        .ok_or("reasons")?;
    assert!(
        reasons
            .iter()
            .any(|reason| reason["kind"] == "conflicting_answer"
                && reason["other_id"] == postgres["value"]),
        "{reasons:?}"
    );
    assert_eq!(
        brief["data"]["still_holds"]["held_up"], true,
        "a conflict is attention, not staleness"
    );
    let other_side = verify_json(dir, id_of(&postgres)?)?;
    assert!(
        other_side["data"]["still_holds"]["reasons"]
            .as_array()
            .ok_or("reasons")?
            .iter()
            .any(|reason| reason["kind"] == "conflicting_answer"
                && reason["other_id"] == sqlite["value"]),
        "the conflict is reported on both decisions"
    );

    let text = verify_text(dir, id_of(&sqlite)?)?;
    let answers_line = format!("  answers: {QUESTION}\n");
    for expected in [
        answers_line.as_str(),
        "  also answered by: Prototype storage is Postgres [accepted] (chose postgres)\n",
        "conflicting answer: \"Prototype storage is Postgres\" is also accepted",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    Ok(())
}

#[test]
fn situational_shows_a_newer_accepted_answer_to_a_matched_decisions_question() -> TestResult<()> {
    let scratch = Scratch::new("situational")?;
    let dir = scratch.path();
    let sqlite = capture(dir, "Prototype storage is SQLite", "sqlite", Some(QUESTION))?;
    let postgres = capture(
        dir,
        "Prototype storage is Postgres",
        "postgres",
        Some(QUESTION),
    )?;

    let json: Value = serde_json::from_str(&run_cli(
        dir,
        CAPTURER,
        true,
        &["query", "situational", "--paths", "storage/engine.rs"],
    )?)?;

    let matches = json["data"]["matches"].as_array().ok_or("matches")?;
    let older = matches
        .iter()
        .find(|item| item["decision"]["id"] == sqlite["value"])
        .ok_or("the older answer matches")?;
    let newer_ids: Vec<&Value> = older["newer_answers"]
        .as_array()
        .ok_or("the older answer shows the newer one")?
        .iter()
        .map(|answer| &answer["decision_id"])
        .collect();
    assert_eq!(newer_ids, vec![&postgres["value"]]);
    let newest = matches
        .iter()
        .find(|item| item["decision"]["id"] == postgres["value"])
        .ok_or("the newer answer matches")?;
    assert!(
        newest.get("newer_answers").is_none(),
        "nothing answers after the newest answer"
    );

    let text = run_cli(
        dir,
        CAPTURER,
        false,
        &[
            "query",
            "--summary",
            "situational",
            "--paths",
            "storage/engine.rs",
        ],
    )?;
    let postgres_id = postgres["value"].as_str().ok_or("decision id")?;
    assert!(
        text.contains(&format!(
            "answer\tnewer\t{postgres_id}\tPrototype storage is Postgres [accepted] (chose postgres)"
        )),
        "{text}"
    );
    Ok(())
}

#[test]
fn answers_that_choose_the_same_option_are_not_a_conflict() -> TestResult<()> {
    let scratch = Scratch::new("agreement")?;
    let dir = scratch.path();
    let first = capture(dir, "Prototype storage is SQLite", "sqlite", Some(QUESTION))?;
    capture(
        dir,
        "Prototype storage stays SQLite",
        "sqlite",
        Some(QUESTION),
    )?;

    let brief = verify_json(dir, id_of(&first)?)?;

    assert_eq!(
        brief["data"]["other_answers"].as_array().map(Vec::len),
        Some(1),
        "the other answer is still listed"
    );
    assert!(
        brief["data"]["still_holds"]["reasons"]
            .as_array()
            .ok_or("reasons")?
            .iter()
            .all(|reason| reason["kind"] != "conflicting_answer"),
        "agreeing answers are not in conflict"
    );
    Ok(())
}

#[test]
fn a_capture_without_a_question_has_no_question_node() -> TestResult<()> {
    let scratch = Scratch::new("none")?;
    let dir = scratch.path();

    let reply = capture(dir, "Prototype storage is SQLite", "sqlite", None)?;

    assert!(reply.get("question_id").is_none(), "{reply}");
    assert_eq!(question_events(dir)?, 0);
    let brief = verify_json(dir, id_of(&reply)?)?;
    assert!(brief["data"].get("question_id").is_none(), "{brief}");
    assert!(brief["data"].get("other_answers").is_none(), "{brief}");
    Ok(())
}

#[test]
fn ground_answers_backfills_the_question_of_a_decision_captured_without_one() -> TestResult<()> {
    let scratch = Scratch::new("ground")?;
    let dir = scratch.path();
    let decision = capture(dir, "Prototype storage is SQLite", "sqlite", None)?;
    let already = events(dir)?.len();

    let reply: Value = serde_json::from_str(&run_cli(
        dir,
        GROUNDER,
        true,
        &["ground", "storage is sqlite", "--answers", QUESTION],
    )?)?;

    assert_eq!(reply["decision_id"], decision["value"]);
    assert_eq!(reply["answers"]["text"], QUESTION);
    assert_eq!(reply["answers"]["reused"], false);
    let question_id = reply["answers"]["question_id"]
        .as_str()
        .ok_or("question id")?
        .to_owned();
    let appended: Vec<Event> = events(dir)?.into_iter().skip(already).collect();
    assert!(appended.iter().all(|event| event.actor_id == GROUNDER));
    assert!(
        appended
            .iter()
            .all(|event| event.causation_event_id.is_none()),
        "attributed later: no causation link to the proposal"
    );

    let brief = verify_json(dir, id_of(&decision)?)?;
    assert_eq!(brief["data"]["question_id"], question_id.as_str());
    let text = verify_text(dir, id_of(&decision)?)?;
    assert!(
        text.contains(&format!("  answers: {QUESTION}\n")),
        "the backfilled question reads on the brief: {text}"
    );

    // The same question again, spelled differently, writes nothing and says it was reused.
    let before = events(dir)?.len();
    let again: Value = serde_json::from_str(&run_cli(
        dir,
        GROUNDER,
        true,
        &[
            "ground",
            "storage is sqlite",
            "--answers",
            "WHICH storage engine should the prototype use",
        ],
    )?)?;
    assert_eq!(again["answers"]["reused"], true);
    assert_eq!(events(dir)?.len(), before);

    // A decision answers one question: naming a different one is refused, nothing written.
    let refused = run_cli(
        dir,
        GROUNDER,
        true,
        &[
            "ground",
            "storage is sqlite",
            "--answers",
            "When should it ship?",
        ],
    )
    .expect_err("a decision answers one question");
    assert!(
        refused.to_string().contains("already answers question"),
        "{refused}"
    );
    assert_eq!(events(dir)?.len(), before);
    Ok(())
}

#[test]
fn ground_text_reply_names_the_question_when_that_is_all_it_added() -> TestResult<()> {
    let scratch = Scratch::new("ground-text")?;
    let dir = scratch.path();
    capture(dir, "Prototype storage is SQLite", "sqlite", None)?;

    let text = run_cli(
        dir,
        GROUNDER,
        false,
        &["ground", "storage is sqlite", "--answers", QUESTION],
    )?;

    assert!(
        text.contains(&format!(
            "attributed to {GROUNDER}\n  answers \"{QUESTION}\" (new question question-"
        )),
        "{text}"
    );
    assert!(
        !text.contains("added 0 thing(s)"),
        "a question alone is not '0 things it rests on': {text}"
    );
    Ok(())
}

#[test]
fn ground_with_nothing_to_record_names_the_question_as_a_way_to_answer() -> TestResult<()> {
    let scratch = Scratch::new("refusal")?;
    let dir = scratch.path();
    capture(dir, "Prototype storage is SQLite", "sqlite", None)?;
    let before = events(dir)?.len();

    let refused = run_cli(dir, GROUNDER, true, &["ground", "storage is sqlite"])
        .expect_err("nothing to ground");

    let message = refused.to_string();
    assert!(message.contains("nothing to ground"), "{message}");
    assert!(message.contains("--answers"), "{message}");
    assert_eq!(events(dir)?.len(), before);
    Ok(())
}

#[test]
fn a_question_that_is_only_punctuation_is_refused_and_nothing_is_written() -> TestResult<()> {
    let scratch = Scratch::new("punctuation")?;
    let dir = scratch.path();

    let refused =
        capture(dir, "Prototype storage is SQLite", "sqlite", Some("?")).expect_err("no words");

    assert!(
        refused.to_string().contains("must contain words"),
        "{refused}"
    );
    assert!(
        events(dir)?.is_empty(),
        "a refused capture leaves nothing behind"
    );
    Ok(())
}

const RECEIPT_QUESTION: &str = "Which typeface do printed receipts use?";
const HOOK_QUESTION: &str = "Which database should the scratch service use?";

/// A capture whose title and rationale share no word with its question, the way the decisions the
/// ask hooks write do ("<header>: <choice>" with a fixed rationale).
fn capture_answering(
    dir: &Path,
    title: &str,
    rationale: &str,
    question: Option<&str>,
) -> TestResult<String> {
    let mut args = vec![
        "emit",
        "decision.capture",
        "--title",
        title,
        "--rationale",
        rationale,
        "--topic-keys",
        "billing",
        "--options",
        "Inter,Times",
        "--chose",
        "Inter",
        "--rests-on-assumption",
        "Receipts print at 9 pt",
    ];
    if let Some(question) = question {
        args.extend(["--question", question]);
    }
    let capture: Value = serde_json::from_str(&run_cli(dir, CAPTURER, true, &args)?)?;
    Ok(id_of(&capture)?.to_owned())
}

fn ids_of_items(output: &str, path: &[&str]) -> TestResult<Vec<String>> {
    let mut value: Value = serde_json::from_str(output)?;
    for key in path {
        value = value[key].take();
    }
    Ok(value
        .as_array()
        .ok_or("items array")?
        .iter()
        .filter_map(|item| item["decision"]["id"].as_str().map(str::to_owned))
        .collect())
}

fn recall_ids(dir: &Path, question: &str) -> TestResult<Vec<String>> {
    let output = run_cli(dir, CAPTURER, true, &["query", "recall", question])?;
    ids_of_items(&output, &["data", "ranked", "items"])
}

fn search_ids(dir: &Path, words: &str) -> TestResult<Vec<String>> {
    let output = run_cli(dir, CAPTURER, true, &["query", "search", "--q", words])?;
    ids_of_items(&output, &["data", "items"])
}

fn why_summary(dir: &Path, question: &str) -> TestResult<String> {
    Ok(run_cli(
        dir,
        CAPTURER,
        false,
        &["query", "why", question, "--summary"],
    )?)
}

#[test]
fn asking_the_recorded_question_finds_the_decision_that_answers_it() -> TestResult<()> {
    let scratch = Scratch::new("ask-back")?;
    let dir = scratch.path();
    let target = capture_answering(
        dir,
        "Inter for invoice text",
        "Inter stays legible at 9 pt on thermal paper",
        Some(RECEIPT_QUESTION),
    )?;
    // Decisions that share some of the question's words, but do not answer it.
    capture_answering(
        dir,
        "Receipts are emailed as PDF",
        "Printed receipts get lost in a drawer, a PDF is searchable",
        None,
    )?;
    capture_answering(
        dir,
        "Invoice numbers are sequential",
        "Auditors read the numbers in order",
        None,
    )?;

    // As recorded, and as a person types it back: no closing "?", another case.
    for asked in [RECEIPT_QUESTION, "which typeface do printed receipts use"] {
        let summary = why_summary(dir, asked)?;
        assert!(
            summary.contains("decision: Inter for invoice text"),
            "`why {asked}` names the decision: {summary}"
        );
        assert!(
            !summary.contains("close match:"),
            "the whole question was found, nothing is missing: {summary}"
        );
        let recalled = recall_ids(dir, asked)?;
        assert_eq!(
            recalled.first(),
            Some(&target),
            "`recall {asked}` lists the target first: {recalled:?}"
        );
    }

    // Two words that appear only in the question.
    assert_eq!(search_ids(dir, "typeface receipts")?, vec![target]);
    Ok(())
}

#[test]
fn asking_the_question_an_ask_hook_recorded_finds_its_decision() -> TestResult<()> {
    let scratch = Scratch::new("ask-hook")?;
    let dir = scratch.path();
    let target = capture_answering(
        dir,
        "Database: Postgres",
        "Chosen in a Claude Code session",
        Some(HOOK_QUESTION),
    )?;
    capture_answering(
        dir,
        "Pricing: after the trial",
        "Chosen in a Claude Code session",
        Some("When does the service start to charge?"),
    )?;

    let summary = why_summary(dir, HOOK_QUESTION)?;
    assert!(
        summary.contains("decision: Database: Postgres"),
        "{summary}"
    );
    assert_eq!(recall_ids(dir, HOOK_QUESTION)?.first(), Some(&target));
    assert_eq!(search_ids(dir, "scratch service")?, vec![target]);
    Ok(())
}

#[test]
fn a_question_linked_after_the_capture_is_asked_back_like_one_captured_with_it() -> TestResult<()> {
    let scratch = Scratch::new("ask-back-linked")?;
    let dir = scratch.path();
    let target = capture_answering(
        dir,
        "Inter for invoice text",
        "Inter stays legible at 9 pt on thermal paper",
        None,
    )?;
    assert!(search_ids(dir, "typeface receipts")?.is_empty());
    run_cli(
        dir,
        GROUNDER,
        true,
        &[
            "ground",
            "inter for invoice text",
            "--answers",
            RECEIPT_QUESTION,
        ],
    )?;

    assert_eq!(search_ids(dir, "typeface receipts")?, vec![target.clone()]);
    assert_eq!(recall_ids(dir, RECEIPT_QUESTION)?.first(), Some(&target));
    let summary = why_summary(dir, RECEIPT_QUESTION)?;
    assert!(
        summary.contains("decision: Inter for invoice text"),
        "{summary}"
    );
    Ok(())
}

#[test]
fn a_question_answered_twice_is_ambiguous_between_both_answers_never_one_of_them() -> TestResult<()>
{
    let scratch = Scratch::new("ask-back-twice")?;
    let dir = scratch.path();
    let inter = capture_answering(
        dir,
        "Inter for invoice text",
        "Inter stays legible at 9 pt on thermal paper",
        Some(RECEIPT_QUESTION),
    )?;
    let times = capture_answering(
        dir,
        "Times for invoice text",
        "Times matches the printed contract template",
        Some(RECEIPT_QUESTION),
    )?;

    let summary = why_summary(dir, RECEIPT_QUESTION)?;
    assert!(
        summary.contains("Inter for invoice text") && summary.contains("Times for invoice text"),
        "both answers are offered, neither is picked for the asker: {summary}"
    );
    let recalled = recall_ids(dir, RECEIPT_QUESTION)?;
    assert!(
        recalled.contains(&inter) && recalled.contains(&times),
        "{recalled:?}"
    );
    let mut found = search_ids(dir, "typeface receipts")?;
    found.sort();
    let mut both = vec![inter, times];
    both.sort();
    assert_eq!(found, both);
    Ok(())
}

#[test]
fn a_recorded_question_with_a_negation_in_it_still_finds_the_decision_that_answers_it(
) -> TestResult<()> {
    let scratch = Scratch::new("ask-back-negated")?;
    let dir = scratch.path();
    let negated = "Which typeface do printed receipts use without a licence fee?";
    let target = capture_answering(
        dir,
        "Inter for invoice text",
        "Inter is open source and stays legible at 9 pt on thermal paper",
        Some(negated),
    )?;
    capture_answering(
        dir,
        "Times for contract text",
        "Times matches the printed contract template",
        Some("Which typeface do printed contracts use?"),
    )?;

    // "without" is a negation in the asker's words; here it is the recorded question's own.
    let summary = why_summary(dir, negated)?;
    assert!(
        summary.contains("decision: Inter for invoice text"),
        "`why` names the decision, not a list of close candidates: {summary}"
    );
    assert!(!summary.contains("close:"), "{summary}");
    assert_eq!(recall_ids(dir, negated)?.first(), Some(&target));
    assert_eq!(search_ids(dir, "licence fee")?, vec![target]);
    Ok(())
}
