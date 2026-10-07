//! The capture plugin's SessionStart hook (hivemind-wdwg): at the start of a session it hands the
//! agent a short directive as additional context, to check the HiveMind ledger before it acts and
//! to record what is worth keeping. Installed but untold, agents used HiveMind 0 times in 31
//! benchmark sessions; the plugin's other hooks fire on AskUserQuestion, which an autonomous agent
//! never calls. Told only to record "decisions", agents saved a summary of each session and missed
//! the rules that cost them something (hivemind-dd95), so the text says what to save and what not.
//!
//! The hook is a fixed sentence: it reads no ledger and decides nothing. It must name tools the
//! MCP server really serves, stay small (it is paid for in every session) and never get in the
//! way: no binary, or the opt-out variable, means no output and exit 0.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HOOK: &str = "plugins/hivemind-capture/scripts/session-start-hook.sh";
const HOOKS_JSON: &str = "plugins/hivemind-capture/hooks/hooks.json";
/// Every file that quotes the directive, which a test holds equal to the hook's text.
const DIRECTIVE_QUOTED_IN: [&str; 3] = [
    "plugins/hivemind-capture/README.md",
    "docs/AGENT_DECISION_CAPTURE.md",
    "CHANGELOG.md",
];
/// The budget, 150 tokens, counted by a proxy: 4 characters per token. The text the hook shipped
/// before (418 characters) was taken for about 100 tokens, so 4 is the ratio the budget was set
/// against; a tokenizer is not run in the test.
const MAX_DIRECTIVE_TOKENS: usize = 150;
const CHARS_PER_TOKEN: usize = 4;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run the hook the way Claude Code does: a `hivemind` binary reachable through
/// `HIVEMIND_CAPTURE_BIN`, the opt-out variable removed, `envs` applied last.
fn run_hook(envs: &[(&str, &str)]) -> TestResult<Output> {
    let mut command = Command::new(root().join(HOOK));
    command
        .env_remove("HIVEMIND_DIRECTIVE_DISABLE")
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    Ok(command.output()?)
}

/// The directive the hook adds as context: the `additionalContext` of its JSON output.
fn directive(output: &Output) -> TestResult<String> {
    assert!(output.status.success(), "{output:?}");
    let reply: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "SessionStart");
    Ok(reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .ok_or("additionalContext")?
        .to_string())
}

/// The tools `hivemind mcp` serves, with each one's input properties, over its stdio transport.
fn served_tools() -> TestResult<Vec<(String, BTreeSet<String>)>> {
    let dir = TempDir::new()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(dir.path().join("hivemind"))
        .args(["mcp", "--agent-tool", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("mcp stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("mcp stdout")?);
    let mut ask = |request: Value| -> TestResult<Value> {
        writeln!(stdin, "{request}")?;
        stdin.flush()?;
        let mut line = String::new();
        stdout.read_line(&mut line)?;
        Ok(serde_json::from_str(line.trim())?)
    };
    ask(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))?;
    let listed = ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))?;
    let tools = listed["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .map(|tool| {
            let properties = tool["inputSchema"]["properties"]
                .as_object()
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default();
            (
                tool["name"].as_str().unwrap_or_default().to_string(),
                properties,
            )
        })
        .collect();
    drop(stdin);
    child.wait()?;
    Ok(tools)
}

#[test]
fn the_plugin_registers_a_session_start_hook_beside_the_ask_hooks() -> TestResult<()> {
    let hooks: Value = serde_json::from_str(&fs::read_to_string(root().join(HOOKS_JSON))?)?;
    let entries = hooks["hooks"]["SessionStart"]
        .as_array()
        .ok_or("SessionStart hooks")?;
    assert_eq!(entries.len(), 1, "{entries:?}");
    // No matcher: the directive is added on startup, resume, clear and compact alike, because a
    // cleared or compacted session has lost it.
    assert!(entries[0].get("matcher").is_none(), "{entries:?}");
    let hook = &entries[0]["hooks"][0];
    assert_eq!(hook["type"], "command");
    assert_eq!(
        hook["command"],
        "\"${CLAUDE_PLUGIN_ROOT}/scripts/session-start-hook.sh\""
    );
    // The AskUserQuestion hooks are still there.
    for event in ["PreToolUse", "PostToolUse"] {
        assert_eq!(hooks["hooks"][event][0]["matcher"], "AskUserQuestion");
    }
    #[cfg(unix)]
    {
        let mode = fs::metadata(root().join(HOOK))?.permissions().mode();
        assert!(mode & 0o111 != 0, "session-start-hook.sh is executable");
    }
    Ok(())
}

#[test]
fn the_directive_stays_within_its_token_budget() -> TestResult<()> {
    let text = directive(&run_hook(&[])?)?;
    let tokens = text.chars().count().div_ceil(CHARS_PER_TOKEN);
    assert!(
        tokens <= MAX_DIRECTIVE_TOKENS,
        "{tokens} tokens ({} characters at {CHARS_PER_TOKEN} per token) is over the \
         {MAX_DIRECTIVE_TOKENS}-token budget: {text}",
        text.chars().count()
    );
    Ok(())
}

#[test]
fn the_directive_says_to_recall_first_and_what_is_worth_saving_and_what_is_not() -> TestResult<()> {
    let text = directive(&run_hook(&[])?)?;
    let lower = text.to_lowercase();
    assert!(
        lower.contains("before you act") && lower.contains("earlier decisions"),
        "it says to check the ledger first: {text}"
    );
    assert!(
        lower.contains("rationale"),
        "it says to record the why: {text}"
    );
    // Save: a rule learned from a failure (the first time), a design choice later work must
    // follow, a reversal of an earlier decision.
    for worth_saving in [
        "failed or cost you",
        "the first time",
        "later work must follow",
        "reversal of an earlier decision",
    ] {
        assert!(
            lower.contains(worth_saving),
            "it names what is worth saving ({worth_saving}): {text}"
        );
    }
    // Do not save: routine status, per-session summaries, plans that only hold for this session.
    for not_worth_saving in [
        "routine status",
        "session summaries",
        "only for this session",
    ] {
        assert!(
            lower.contains(not_worth_saving),
            "it names what is not worth saving ({not_worth_saving}): {text}"
        );
    }
    // General: nothing from the benchmarks that measured it.
    for benchmark_word in ["client", "fund", "checkpoint", "month"] {
        assert!(
            !lower.contains(benchmark_word),
            "no benchmark word ({benchmark_word}): {text}"
        );
    }
    Ok(())
}

#[test]
fn the_directive_names_tools_the_mcp_server_serves_and_the_arguments_it_tells_you_to_pass(
) -> TestResult<()> {
    let text = directive(&run_hook(&[])?)?;
    let tools = served_tools()?;
    let properties_of = |name: &str| {
        tools
            .iter()
            .find(|(tool, _)| tool == name)
            .map(|(_, properties)| properties.clone())
    };
    let recall = properties_of("recall_decisions").ok_or("the server serves recall_decisions")?;
    let capture = properties_of("capture_decision").ok_or("the server serves capture_decision")?;
    assert!(text.contains("`recall_decisions`") && text.contains("`capture_decision`"));
    // Every tool it names is served (`hivemind` is the MCP server, not a tool).
    for name in text.split('`').skip(1).step_by(2) {
        if name != "hivemind" {
            assert!(
                tools.iter().any(|(tool, _)| tool == name),
                "the directive names {name}, which the server does not serve: {tools:?}"
            );
        }
    }
    // `q = ...` is recall's text query; the capture fields are the ones the directive lists.
    assert!(text.contains("q =") && recall.contains("q"), "{recall:?}");
    for field in ["title", "rationale", "options", "grounding"] {
        assert!(
            text.contains(field),
            "the directive mentions {field}: {text}"
        );
        assert!(
            capture.contains(field),
            "capture_decision takes {field}: {capture:?}"
        );
    }
    Ok(())
}

/// Text as a reader sees it once markdown wrapping is gone: quote markers, line breaks and runs of
/// spaces collapsed, so a directive wrapped across lines of a blockquote still compares equal.
fn unwrapped(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_start().trim_start_matches('>'))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_plugin_readme_the_docs_and_the_changelog_quote_the_text_the_hook_prints() -> TestResult<()> {
    // The hook script is the one source of truth; there is no way to include it in markdown, so
    // a test keeps every quotation of it equal.
    let text = directive(&run_hook(&[])?)?;
    for file in DIRECTIVE_QUOTED_IN {
        let quoted = unwrapped(&fs::read_to_string(root().join(file))?);
        assert!(
            quoted.contains(&text),
            "{file} does not quote the directive the hook prints, word for word:\n{text}"
        );
    }
    Ok(())
}

#[test]
fn a_ledger_that_does_not_exist_yet_still_gets_the_directive() -> TestResult<()> {
    // The first session in a project is where the directive is needed; the MCP server creates
    // the ledger on the first capture, so a missing directory is a new ledger, not a missing one.
    let dir = TempDir::new()?;
    let missing = dir.path().join("no-ledger-yet");
    let output = run_hook(&[("HIVEMIND_DIR", missing.to_str().ok_or("path")?)])?;
    assert!(directive(&output)?.contains("recall_decisions"));
    assert!(!missing.exists(), "the hook reads and writes no ledger");
    Ok(())
}

#[test]
fn with_the_opt_out_set_or_no_hivemind_binary_it_prints_nothing_and_exits_zero() -> TestResult<()> {
    for envs in [
        vec![("HIVEMIND_DIRECTIVE_DISABLE", "1")],
        vec![("HIVEMIND_CAPTURE_BIN", "/nonexistent/hivemind")],
    ] {
        let output = run_hook(&envs)?;
        assert!(output.status.success(), "{envs:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{envs:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{envs:?}: {output:?}");
    }
    // No override and nothing named `hivemind` on PATH: the same silent no-op.
    let empty = TempDir::new()?;
    let output = Command::new(root().join(HOOK))
        .env_remove("HIVEMIND_CAPTURE_BIN")
        .env_remove("HIVEMIND_DIRECTIVE_DISABLE")
        .env("PATH", empty.path())
        .stdin(Stdio::null())
        .output()?;
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
    Ok(())
}
