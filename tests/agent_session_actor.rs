//! Who an unnamed CLI write is recorded as (hivemind-jglb7). `emit decision.capture` already
//! files an agent's capture under the agent; the follow-up verbs the docs and the
//! cited-not-linked hint teach (`ground`, `retitle`, `disagree`, `supersede`, `move`) fell back
//! to the git user and so named a person who never made the change. With an agent in the
//! environment and no `--actor`, they record the agent; a plain terminal still records the human.

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

const SEEDER: &str = "human:seeder";
const AGENT: &str = "agent:claude:crew-jglb7";
const GIT_USER: &str = "human:git-user@example.test";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-actor-{label}-{}", Uuid::new_v4()));
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

fn events(dir: &Path) -> TestResult<Vec<Event>> {
    Ok(SqliteEventLedger::open(dir)?.read(0, 1000)?)
}

/// Run one CLI call with no `--actor`, as the agent the environment would have named.
fn run_as_agent(dir: &Path, args: &[&str]) -> hivemind::Result<String> {
    let mut argv = vec![
        "hivemind".to_owned(),
        "--json".to_owned(),
        "--hivemind-dir".to_owned(),
        dir.display().to_string(),
    ];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    let mut cli = Cli::parse_from(argv);
    cli.adopt_ambient_agent(Some(AGENT.to_owned()));
    run(&cli)
}

/// Seed one decision as someone else, with an explicit `--actor`, and return its id.
fn seed(dir: &Path, title: &str) -> TestResult<String> {
    let output = run(&Cli::parse_from([
        "hivemind",
        "--json",
        "--hivemind-dir",
        &dir.display().to_string(),
        "--actor",
        SEEDER,
        "emit",
        "decision.capture",
        "--bet",
        "--title",
        title,
        "--rationale",
        "Rationale text long enough for the readable floor",
        "--topic-keys",
        "actors",
        "--options",
        "adopt,skip",
        "--chose",
        "adopt",
    ]))?;
    let envelope: Value = serde_json::from_str(&output)?;
    Ok(envelope["value"]
        .as_str()
        .ok_or("capture returns the decision id")?
        .to_owned())
}

fn actors_after(dir: &Path, skip: usize) -> TestResult<Vec<String>> {
    Ok(events(dir)?
        .into_iter()
        .skip(skip)
        .map(|event| event.actor_id)
        .collect())
}

#[test]
fn follow_up_verbs_run_by_an_agent_with_no_actor_record_the_agent() -> TestResult<()> {
    let scratch = Scratch::new("verbs")?;
    let dir = scratch.path();
    let target = seed(dir, "Checker summary lists failures first")?;
    let premise = seed(dir, "Checker summary goes out on Mondays")?;

    // The command the cited_not_linked hint prints, verbatim: no --actor.
    let before = events(dir)?.len();
    run_as_agent(
        dir,
        &["ground", "--id", &target, "--rests-on-decision", &premise],
    )?;
    let grounded = actors_after(dir, before)?;
    assert!(!grounded.is_empty(), "ground wrote nothing");
    assert!(
        grounded.iter().all(|actor| actor == AGENT),
        "ground is the agent's: {grounded:?}"
    );

    let before = events(dir)?.len();
    run_as_agent(
        dir,
        &[
            "retitle",
            "--decision",
            &target,
            "--to",
            "Checker summary leads with the failures",
        ],
    )?;
    assert_eq!(actors_after(dir, before)?, vec![AGENT.to_owned()]);

    let before = events(dir)?.len();
    run_as_agent(
        dir,
        &[
            "disagree",
            "--decision",
            &premise,
            "--reason",
            "Fridays suit the team better",
        ],
    )?;
    let disagreed = events(dir)?;
    let rejected = disagreed
        .iter()
        .skip(before)
        .find(|event| event.event_type == EventType::DecisionRejected)
        .ok_or("disagree records a decision.rejected")?;
    assert_eq!(rejected.actor_id, AGENT);
    assert_eq!(
        rejected.source.as_str(),
        "agent",
        "the source is the agent's too"
    );

    let before = events(dir)?.len();
    run_as_agent(
        dir,
        &[
            "supersede",
            "--old",
            &target,
            "--title",
            "Checker summary leads with failures, grouped by rig",
            "--rationale",
            "Witnesses read their own rig, so the failures are grouped by rig",
            "--topic-keys",
            "actors",
            "--options",
            "grouped,flat",
            "--chose",
            "grouped",
            "--bet",
        ],
    )?;
    let superseded = actors_after(dir, before)?;
    assert!(!superseded.is_empty(), "supersede wrote nothing");
    assert!(
        superseded.iter().all(|actor| actor == AGENT),
        "the replacement is proposed, superseded and accepted as the agent: {superseded:?}"
    );

    // `move` is refused here, but the refusal names the acting actor's own personal project,
    // which is the agent's and not a person's.
    let refused = run_as_agent(
        dir,
        &[
            "move",
            "--decision",
            &premise,
            "--to",
            "personal:agent:other",
        ],
    )
    .expect_err("a move to someone else's personal project is refused");
    assert!(
        refused.to_string().contains("personal:agent:claude"),
        "the acting actor is the agent: {refused}"
    );
    Ok(())
}

/// The real binary, with the environment an agent session has (or does not have) and a git
/// identity for the person who owns the machine.
fn hivemind_bin(dir: &Path, home: &Path, agent_env: bool) -> Command {
    let gitconfig = home.join("gitconfig");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hivemind"));
    command
        // Not a git checkout, so the repository's own user.email cannot answer.
        .current_dir(home)
        .env("GIT_CONFIG_GLOBAL", &gitconfig)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env_remove("HIVEMIND_ACTOR")
        .env_remove("HIVEMIND_AGENT_TOOL")
        .env_remove("HIVEMIND_TOOL")
        .env_remove("HIVEMIND_AGENT_SESSION")
        .env_remove("HIVEMIND_CODEX_SESSION")
        .env_remove("HIVEMIND_CLAUDE_SESSION")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_TASK_ID")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args(["--json", "--hivemind-dir"])
        .arg(dir);
    if agent_env {
        command
            .env("HIVEMIND_AGENT_TOOL", "claude")
            .env("GC_AGENT", "crew-jglb7");
    }
    command
}

fn disagree_with_binary(
    dir: &Path,
    home: &Path,
    agent_env: bool,
    decision_id: &str,
) -> TestResult<Event> {
    let before = events(dir)?.len();
    let output = hivemind_bin(dir, home, agent_env)
        .args([
            "disagree",
            "--decision",
            decision_id,
            "--reason",
            "Fridays suit the team better",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "disagree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    events(dir)?
        .into_iter()
        .skip(before)
        .find(|event| event.event_type == EventType::DecisionRejected)
        .ok_or_else(|| "disagree records a decision.rejected".into())
}

#[test]
fn the_binary_records_the_agent_in_an_agent_session_and_the_person_at_a_plain_terminal(
) -> TestResult<()> {
    let home = Scratch::new("home")?;
    fs::write(
        home.path().join("gitconfig"),
        "[user]\n\temail = git-user@example.test\n\tname = Git User\n",
    )?;
    let scratch = Scratch::new("binary")?;
    let dir = scratch.path();
    let first = seed(dir, "Checker summary goes out on Mondays")?;
    let second = seed(dir, "Checker summary lists failures first")?;

    let by_agent = disagree_with_binary(dir, home.path(), true, &first)?;
    assert_eq!(
        by_agent.actor_id, "agent:claude:crew-jglb7",
        "an agent session with no --actor is the agent, not the git user"
    );

    let by_person = disagree_with_binary(dir, home.path(), false, &second)?;
    assert_eq!(
        by_person.actor_id, GIT_USER,
        "a plain terminal with no agent in the environment is still the person"
    );

    // An explicit --actor wins over what the environment shows.
    let third = seed(dir, "Checker summary is grouped by rig")?;
    let before = events(dir)?.len();
    let output = hivemind_bin(dir, home.path(), true)
        .args([
            "--actor",
            "human:alice",
            "disagree",
            "--decision",
            &third,
            "--reason",
            "Fridays suit the team better",
        ])
        .output()?;
    assert!(output.status.success());
    assert!(
        actors_after(dir, before)?
            .iter()
            .all(|actor| actor == "human:alice"),
        "a typed --actor is believed"
    );
    Ok(())
}

/// The `--actor` entry of `hivemind --help` as the binary prints it in this environment,
/// whitespace folded.
fn actor_help_entry(dir: &Path, home: &Path, agent_env: bool) -> TestResult<String> {
    let output = hivemind_bin(dir, home, agent_env).arg("--help").output()?;
    assert!(output.status.success(), "--help failed");
    let help = String::from_utf8(output.stdout)?;
    let start = help.find("--actor <ACTOR>").ok_or("--help lists --actor")?;
    let len = help[start..]
        .find("--tenant")
        .ok_or("--actor is followed by --tenant")?;
    Ok(help[start..start + len]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

#[test]
fn help_names_the_actor_an_untyped_write_is_recorded_as_in_that_environment() -> TestResult<()> {
    // hivemind-iynfm: `--help` named the git user as the `--actor` default in an agent session,
    // where an untyped write is recorded as the agent. Help and the recorded actor come from
    // one rule; this compares what the binary says with what the binary writes.
    let home = Scratch::new("help-home")?;
    fs::write(
        home.path().join("gitconfig"),
        "[user]\n\temail = git-user@example.test\n\tname = Git User\n",
    )?;
    let scratch = Scratch::new("help")?;
    let dir = scratch.path();
    let first = seed(dir, "Checker summary goes out on Mondays")?;
    let second = seed(dir, "Checker summary lists failures first")?;

    let agent_help = actor_help_entry(dir, home.path(), true)?;
    let agent_recorded = disagree_with_binary(dir, home.path(), true, &first)?.actor_id;
    assert!(
        agent_help.contains(&format!("Here that is the agent, {agent_recorded};")),
        "an agent session's help names the agent it records: {agent_help}"
    );
    assert!(
        !agent_help.contains("[default:"),
        "no git-user default is advertised as the default: {agent_help}"
    );

    let person_help = actor_help_entry(dir, home.path(), false)?;
    let person_recorded = disagree_with_binary(dir, home.path(), false, &second)?.actor_id;
    assert!(
        person_help.contains(&format!(
            "recorded as when --actor is not typed: {person_recorded},"
        )),
        "a plain terminal's help still names the git user it records: {person_help}"
    );
    assert!(
        !person_help.contains("Here that is the agent"),
        "no agent is claimed where none is in the environment: {person_help}"
    );

    // The write verbs that name `--actor` say what an untyped one is, in either environment.
    for verb in ["ground", "disagree"] {
        for agent_env in [true, false] {
            let output = hivemind_bin(dir, home.path(), agent_env)
                .args([verb, "--help"])
                .output()?;
            let help = String::from_utf8(output.stdout)?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                help.contains(
                    "untyped, the agent in an agent session and the git user at a plain terminal"
                ),
                "{verb} --help says what an untyped --actor is: {help}"
            );
        }
    }
    Ok(())
}
