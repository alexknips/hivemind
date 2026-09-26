use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::Value;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn codex_capture_plugin_bundle_is_installable_and_points_at_cli_capture() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let marketplace = read_json(root.join(".agents/plugins/marketplace.json"))?;
    assert_eq!(marketplace["name"], "hivemind-plugins");
    assert_eq!(marketplace["interface"]["displayName"], "HiveMind Plugins");

    let plugin = marketplace["plugins"]
        .as_array()
        .and_then(|plugins| {
            plugins
                .iter()
                .find(|plugin| plugin["name"] == "hivemind-capture")
        })
        .expect("hivemind-capture appears in marketplace");
    assert_eq!(plugin["source"]["source"], "local");
    assert_eq!(plugin["source"]["path"], "./plugins/hivemind-capture");
    assert_eq!(plugin["policy"]["installation"], "AVAILABLE");
    assert_eq!(plugin["policy"]["authentication"], "ON_INSTALL");

    let manifest = read_json(root.join("plugins/hivemind-capture/.codex-plugin/plugin.json"))?;
    assert_eq!(manifest["name"], "hivemind-capture");
    assert_eq!(manifest["skills"], "./skills/");
    assert_eq!(manifest["mcpServers"], "./.mcp.json");
    assert!(manifest["interface"]["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|capability| capability == "MCP"));
    assert_no_todos("plugin manifest", &manifest.to_string());

    let mcp = read_json(root.join("plugins/hivemind-capture/.mcp.json"))?;
    assert_mcp_pins_shared_ledger(&mcp);

    let skill =
        fs::read_to_string(root.join("plugins/hivemind-capture/skills/hivemind-capture/SKILL.md"))?;
    assert_no_todos("skill", &skill);
    assert!(skill.contains("decision.capture"));
    assert!(skill.contains("--agent-tool codex"));
    // hivemind-zdsh.9: the skill now documents actor construction generically
    // (agent:<tool>:<name>, a stable identity, not a raw session id) rather
    // than repeating a per-tool "agent:codex:<session>" example.
    assert!(skill.contains("agent:<tool>:<name>"));
    assert!(skill.contains("Automatic Capture Triggers"));
    assert!(skill.contains("CODEX_THREAD_ID"));
    assert!(skill.contains("Capture immediately after the decision is made"));
    assert!(skill.contains("HIVEMIND_DIR"));
    // hivemind-s15q.15: decision captures work their project out from where the agent runs, and
    // the skill tells the agent what it will see and to relay the reminder.
    assert!(skill.contains("--project-from-context"));
    assert!(skill.contains("Which Project A Capture Lands In"));
    assert!(skill.contains(".hivemind-project"));
    assert!(skill.contains("saved to your personal project"));
    Ok(())
}

#[test]
fn claude_code_capture_command_writes_human_decision() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let settings = read_json(root.join(".claude/settings.json"))?;
    assert_eq!(settings["env"]["HIVEMIND_DIR"], "./hivemind/");
    assert_eq!(
        settings["extraKnownMarketplaces"]["hivemind"]["source"]["source"],
        "github"
    );
    assert_eq!(
        settings["extraKnownMarketplaces"]["hivemind"]["source"]["repo"],
        "alexknips/hivemind"
    );
    assert_eq!(
        settings["enabledPlugins"]["hivemind-capture@hivemind"],
        true
    );
    assert!(settings["permissions"]["allow"]
        .as_array()
        .expect("allow list")
        .iter()
        .any(|permission| permission == "Bash(.claude/scripts/capture-decision.sh:*)"));

    let command = fs::read_to_string(root.join(".claude/commands/capture-decision.md"))?;
    assert!(command.contains("/capture-decision"));
    assert!(command.contains("--source human"));
    assert!(command.contains("--source agent"));

    let script = root.join(".claude/scripts/capture-decision.sh");
    let unique = uuid::Uuid::new_v4().to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-claude-capture-{unique}"));

    let output = Command::new(&script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .args([
            "--source",
            "human",
            "--actor-id",
            "human:test-user",
            "--title",
            "Capture Claude Code slash command decisions",
            "--rationale",
            "Project-local Claude Code commands should write manual decisions with human provenance",
            "--topic-keys",
            "claude,capture",
            "--options",
            "repo-command,manual-shell",
            "--chose",
            "repo-command",
            "--bet",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "capture script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let query = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .args([
            "query",
            "search_decisions",
            "--q",
            "slash command",
            "--actor-id",
            "human:test-user",
            "--source",
            "human",
            "--limit",
            "5",
        ])
        .output()?;
    assert!(
        query.status.success(),
        "query failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query: Value = serde_json::from_slice(&query.stdout)?;
    assert_eq!(query["result_count"], 1);
    assert_eq!(
        query["data"]["items"][0]["graph_context"]["actor_ids"][0],
        "human:test-user"
    );

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn claude_code_plugin_bundle_is_installable_and_wires_cli_mcp() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let repo_mcp = read_json(root.join(".mcp.json"))?;
    assert_mcp_pins_shared_ledger(&repo_mcp);

    let marketplace = read_json(root.join(".claude-plugin/marketplace.json"))?;
    assert_eq!(marketplace["name"], "hivemind");
    assert_eq!(marketplace["owner"]["name"], "HiveMind contributors");

    let plugin = marketplace["plugins"]
        .as_array()
        .and_then(|plugins| {
            plugins
                .iter()
                .find(|plugin| plugin["name"] == "hivemind-capture")
        })
        .expect("hivemind-capture appears in Claude marketplace");
    assert_eq!(plugin["source"], "./plugins/hivemind-capture");
    assert_eq!(plugin["displayName"], "HiveMind Capture");
    assert!(plugin["tags"]
        .as_array()
        .expect("tags")
        .iter()
        .any(|tag| tag == "mcp"));

    let manifest = read_json(root.join("plugins/hivemind-capture/.claude-plugin/plugin.json"))?;
    assert_eq!(manifest["name"], "hivemind-capture");
    assert_eq!(manifest["commands"], "./commands/");
    assert_eq!(manifest["skills"], "./skills/");
    assert_eq!(manifest["mcpServers"], "./.mcp.json");
    assert_eq!(
        manifest["userConfig"]["hivemind_dir"]["default"],
        "./hivemind/"
    );
    assert_no_todos("Claude plugin manifest", &manifest.to_string());

    let mcp = read_json(root.join("plugins/hivemind-capture/.mcp.json"))?;
    assert_eq!(mcp["mcpServers"]["hivemind"]["command"], "hivemind");
    assert!(mcp["mcpServers"]["hivemind"]["args"]
        .as_array()
        .expect("mcp args")
        .iter()
        .any(|arg| arg == "mcp"));
    assert!(mcp["mcpServers"]["hivemind"]["args"]
        .as_array()
        .expect("mcp args")
        .windows(2)
        .any(|pair| pair[0] == "--agent-tool" && pair[1] == "claude"));
    // hivemind-s15q.15: the bundled stdio server works a capture's project out from its own
    // working directory when the call names none.
    assert!(mcp["mcpServers"]["hivemind"]["args"]
        .as_array()
        .expect("mcp args")
        .iter()
        .any(|arg| arg == "--project-from-context"));
    assert_eq!(
        mcp["mcpServers"]["hivemind"]["env"]["HIVEMIND_DIR"],
        "./hivemind/"
    );
    assert_mcp_pins_shared_ledger(&mcp);

    let capture_command =
        fs::read_to_string(root.join("plugins/hivemind-capture/commands/capture.md"))?;
    assert_no_todos("Claude unified capture command", &capture_command);
    require_contains(
        &capture_command,
        "${CLAUDE_PLUGIN_ROOT}/scripts/capture.sh",
        "unified capture command invokes capture.sh",
    )?;
    require_contains(
        &capture_command,
        "--kind evidence",
        "unified capture command documents evidence kind",
    )?;
    require_contains(
        &capture_command,
        "hivemind-classifier",
        "unified capture command documents classifier delegation",
    )?;

    let capture_decision_command =
        fs::read_to_string(root.join("plugins/hivemind-capture/commands/capture-decision.md"))?;
    assert_no_todos("Claude capture decision command", &capture_decision_command);
    require_contains(
        &capture_decision_command,
        "/hivemind-capture:capture",
        "decision command points to primary capture command",
    )?;
    require_contains(
        &capture_decision_command,
        "/hivemind-capture:query-decisions",
        "decision command prints query follow-up",
    )?;
    require_contains(
        &capture_decision_command,
        "agent:claude:<name>",
        "decision command documents Claude actor provenance",
    )?;
    require_contains(
        &capture_decision_command,
        "${CLAUDE_PLUGIN_ROOT}/scripts/capture-decision.sh",
        "decision command invokes compatibility wrapper",
    )?;

    let query_command =
        fs::read_to_string(root.join("plugins/hivemind-capture/commands/query-decisions.md"))?;
    assert_no_todos("Claude query command", &query_command);
    assert!(query_command.contains("query recall"));
    assert!(query_command.contains("truncated"));

    let readme = fs::read_to_string(root.join("plugins/hivemind-capture/README.md"))?;
    assert_no_todos("Claude plugin README", &readme);
    assert!(readme.contains("/plugin install hivemind-capture@hivemind"));
    assert!(readme.contains("/plugin uninstall hivemind-capture@hivemind"));
    require_contains(
        &readme,
        "/hivemind-capture:capture",
        "README documents primary capture command",
    )?;
    require_contains(
        &readme,
        "/hivemind-capture:capture-decision",
        "README documents compatibility decision command",
    )?;
    require_contains(
        &readme,
        "install-active-capture-skill.sh",
        "README documents active capture skill installer",
    )?;

    let active_skill = fs::read_to_string(
        root.join("plugins/hivemind-capture/.claude-plugin/skills/active-capture.md"),
    )?;
    verify_active_capture_skill("Claude active capture skill", &active_skill)?;

    let rig_skill = fs::read_to_string(root.join(".claude/skills/active-capture.md"))?;
    if rig_skill != active_skill {
        return Err("rig-local active capture skill differs from plugin source".into());
    }

    assert_executable(root.join("plugins/hivemind-capture/scripts/capture.sh"))?;
    assert_executable(root.join("plugins/hivemind-capture/scripts/capture-decision.sh"))?;
    assert_executable(root.join("plugins/hivemind-capture/scripts/query-decisions.sh"))?;
    assert_executable(
        root.join("plugins/hivemind-capture/scripts/install-active-capture-skill.sh"),
    )?;
    Ok(())
}

#[test]
fn claude_active_capture_skill_installer_copies_to_project_skills() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project_dir = unique_temp_dir("hivemind-active-capture-project")?;
    let installer = root.join("plugins/hivemind-capture/scripts/install-active-capture-skill.sh");

    let output = Command::new(&installer)
        .current_dir(root)
        .arg("--project-dir")
        .arg(&project_dir)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "active capture skill installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let source = fs::read_to_string(
        root.join("plugins/hivemind-capture/.claude-plugin/skills/active-capture.md"),
    )?;
    let installed = fs::read_to_string(project_dir.join(".claude/skills/active-capture.md"))?;
    if installed != source {
        return Err("installed active capture skill differs from plugin source".into());
    }
    verify_active_capture_skill("installed active capture skill", &installed)?;

    let _ = fs::remove_dir_all(project_dir);
    Ok(())
}

#[test]
fn claude_code_plugin_capture_and_query_scripts_write_agent_decision() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = uuid::Uuid::new_v4().to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-claude-plugin-{unique}"));
    let actor_id = "agent:claude:plugin-test-session";

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    let output = Command::new(&capture_script)
        .current_dir(markerless_cwd())
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "plugin-test-session")
        // A Gas City-hosted test run (or this very repo's own dev polecats) may have
        // GC_AGENT/GC_ALIAS ambient; this test isolates CLAUDE_SESSION_ID-derived
        // identity specifically, so the stable-identity-first resolution
        // (identity.rs::stable_agent_identity, hivemind-zdsh.9) must not see them.
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args([
            "--title",
            "Capture Claude plugin decisions",
            "--rationale",
            "The Claude plugin should write agent decisions with session provenance",
            "--topic-keys",
            "claude,plugin,capture",
            "--options",
            "plugin-command,manual-shell",
            "--chose",
            "plugin-command",
            "--bet",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "plugin capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Captured HiveMind decision decision-"));
    assert!(stdout.contains("/hivemind-capture:query-decisions"));
    assert!(stdout.contains(actor_id));

    let query_script = root.join("plugins/hivemind-capture/scripts/query-decisions.sh");
    let query = Command::new(&query_script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "plugin-test-session")
        .args(["--q", "plugin decisions", "--limit", "5"])
        .output()?;
    assert!(
        query.status.success(),
        "plugin query failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query: Value = serde_json::from_slice(&query.stdout)?;
    assert_eq!(query["result_count"], 1);
    assert_eq!(
        query["data"]["ranked"]["items"][0]["graph_context"]["actor_ids"][0],
        actor_id
    );

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn query_decisions_script_finds_decisions_captured_by_a_different_session() -> TestResult<()> {
    // Regression test for hivemind-tenv.4's reopen finding: the query
    // script used to silently inject --actor-id/--source for the
    // QUERYING session's own identity, so "what did we decide about X"
    // from a fresh session found nothing a prior session had captured.
    // Capture and query here run as two distinct sessions/actors on
    // purpose — the bug was invisible when both used the same session
    // (see claude_code_plugin_capture_and_query_scripts_write_agent_decision).
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = uuid::Uuid::new_v4().to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-cross-session-query-{unique}"));
    let capturer_actor_id = "agent:claude:capturer-session";

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    let output = Command::new(&capture_script)
        .current_dir(markerless_cwd())
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "capturer-session")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args([
            "--title",
            "Recall smoke test fixture across sessions",
            "--rationale",
            "A fresh querying session must find decisions captured by other sessions",
            "--topic-keys",
            "claude,plugin,recall",
            "--options",
            "cross-session,same-session",
            "--chose",
            "cross-session",
            "--bet",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "plugin capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let query_script = root.join("plugins/hivemind-capture/scripts/query-decisions.sh");
    let query = Command::new(&query_script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "querier-session")
        .args(["--q", "recall smoke test fixture", "--limit", "5"])
        .output()?;
    assert!(
        query.status.success(),
        "plugin query failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query: Value = serde_json::from_slice(&query.stdout)?;
    assert_eq!(
        query["result_count"], 1,
        "a query from a different session with no --actor-id/--source must still find the decision"
    );
    assert_eq!(
        query["data"]["ranked"]["items"][0]["graph_context"]["actor_ids"][0],
        capturer_actor_id
    );

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

/// Captures one decision through the plugin's own capture script so the
/// free-text query tests below have exactly one thing to find.
fn capture_joined_query_fixture(root: &Path, hivemind_dir: &Path) -> TestResult<()> {
    let output = Command::new(root.join("plugins/hivemind-capture/scripts/capture-decision.sh"))
        .current_dir(markerless_cwd())
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "join-fixture-session")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args([
            "--title",
            "Recall smoke test fixture for joined free text",
            "--rationale",
            "A free-text query typed with or without quotes must reach recall as one description",
            "--topic-keys",
            "plugin,recall",
            "--options",
            "join-words,require-quotes",
            "--chose",
            "join-words",
            "--bet",
        ])
        .output()?;
    require(
        output.status.success(),
        format!(
            "plugin capture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn require_one_recall_hit(output: &std::process::Output, what: &str) -> TestResult<()> {
    require(
        output.status.success(),
        format!(
            "{what}: query failed ({:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    let result: Value = serde_json::from_slice(&output.stdout)?;
    require_eq(&result["result_count"], &Value::from(1), what)
}

#[test]
fn query_decisions_script_joins_unquoted_free_text_into_one_query() -> TestResult<()> {
    // hivemind-f4ng: the CLI's positional query is ONE shell token, so an unquoted
    // `recall smoke test fixture` used to reach it as four and fail with
    // "unexpected argument 'smoke' found". The script now joins the leading
    // non-flag words itself; a query already quoted into one word is unchanged.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let hivemind_dir = unique_temp_dir("hivemind-query-join")?;
    capture_joined_query_fixture(root, &hivemind_dir)?;

    let query_script = root.join("plugins/hivemind-capture/scripts/query-decisions.sh");
    let typed_forms: [&[&str]; 4] = [
        &["recall smoke test fixture", "--limit", "5"],
        &["recall", "smoke", "test", "fixture", "--limit", "5"],
        &["recall", "smoke", "test", "fixture"],
        &["--q", "recall smoke test fixture", "--limit", "5"],
    ];
    for args in typed_forms {
        let output = Command::new(&query_script)
            .current_dir(root)
            .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
            .env("HIVEMIND_DIR", &hivemind_dir)
            .env("CLAUDE_PROJECT_DIR", root)
            .args(args)
            .output()?;
        require_one_recall_hit(&output, &format!("query-decisions.sh {args:?}"))?;
    }

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn query_decisions_command_runs_both_typed_forms_of_its_own_argument_hint() -> TestResult<()> {
    // hivemind-f4ng: the command markdown wrapped `$ARGUMENTS` in one more pair of
    // quotes, so the quoted form its argument-hint documents expanded to `""a b""`
    // and split into two words. Run the very line the markdown hands the model,
    // with the typed arguments substituted the way Claude Code does.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin_root = root.join("plugins/hivemind-capture");
    let command_md = fs::read_to_string(plugin_root.join("commands/query-decisions.md"))?;
    let invocation = command_md
        .lines()
        .find(|line| line.starts_with("${CLAUDE_PLUGIN_ROOT}/scripts/query-decisions.sh"))
        .ok_or("query-decisions.md should show the helper invocation")?;
    require_eq(
        invocation,
        "${CLAUDE_PLUGIN_ROOT}/scripts/query-decisions.sh $ARGUMENTS",
        "the helper takes the typed arguments bare, never wrapped in more quotes",
    )?;

    let hivemind_dir = unique_temp_dir("hivemind-query-command")?;
    capture_joined_query_fixture(root, &hivemind_dir)?;

    for typed in [
        r#""recall smoke test fixture" --limit 5"#,
        "recall smoke test fixture --limit 5",
        r#"--q "recall smoke test fixture" --limit 5"#,
    ] {
        let line = invocation
            .replace("${CLAUDE_PLUGIN_ROOT}", &plugin_root.to_string_lossy())
            .replace("$ARGUMENTS", typed);
        let output = Command::new("bash")
            .args(["-c", &line])
            .current_dir(root)
            .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
            .env("HIVEMIND_DIR", &hivemind_dir)
            .env("CLAUDE_PROJECT_DIR", root)
            .output()?;
        require_one_recall_hit(
            &output,
            &format!("/hivemind-capture:query-decisions {typed}"),
        )?;
    }

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn context_verbs_print_usage_when_called_with_no_arguments() -> TestResult<()> {
    // macOS bash 3.2 died on the empty-array expansion under `set -u` when these
    // scripts got no arguments at all. No argument is a usage error, answered
    // before the CLI is even resolved: point the binary override at a path that
    // cannot run, so reaching the CLI would show up as exit 127, not usage.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for verb in ["recall", "why", "verify", "disagree", "supersede"] {
        let output = Command::new(root.join(format!("plugins/hivemind-context/scripts/{verb}.sh")))
            .current_dir(root)
            .env("HIVEMIND_CAPTURE_BIN", "/nonexistent/hivemind")
            .output()?;
        require_eq(
            output.status.code(),
            Some(2),
            &format!("{verb}.sh exit code"),
        )?;
        require_contains(
            &String::from_utf8_lossy(&output.stderr),
            "Usage:",
            &format!("{verb}.sh stderr"),
        )?;
    }
    Ok(())
}

#[test]
fn codex_capture_defaults_actor_from_session_environment() -> TestResult<()> {
    let hivemind_dir = unique_temp_dir("hivemind-codex-default-capture")?;

    let output = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .env_remove("HIVEMIND_ACTOR")
        .env_remove("HIVEMIND_AGENT_TOOL")
        .env_remove("HIVEMIND_AGENT_SESSION")
        .env_remove("HIVEMIND_CODEX_SESSION")
        .env_remove("HIVEMIND_CLAUDE_SESSION")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .env("CODEX_SESSION_ID", "plugin-test-session")
        .arg("--json")
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .args([
            "emit",
            "decision.capture",
            "--bet",
            "--title",
            "Capture Codex plugin decisions without setup",
            "--rationale",
            "The Codex capture path should derive actor provenance from the session",
            "--topic-keys",
            "codex,plugin,capture",
            "--options",
            "default-actor,manual-actor",
            "--chose",
            "default-actor",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "codex capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: Value = serde_json::from_slice(&output.stdout)?;
    let event = proposal_event(
        &hivemind_dir,
        output["value"].as_str().expect("decision id"),
    )?;
    assert_eq!(event.actor_id, "agent:codex:plugin-test-session");
    assert_eq!(event.source.as_str(), "agent");
    // source_ref carries the raw session id (provenance), not a repeat of actor_id --
    // ambient derivation with no --agent-session/--actor-id pin (hivemind-zdsh.9).
    assert_eq!(event.source_ref.as_deref(), Some("plugin-test-session"));

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn capture_plugin_scripts_derive_codex_session_context() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = uuid::Uuid::new_v4().to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-codex-plugin-{unique}"));
    let actor_id = "agent:codex:codex-thread-test";

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    let output = Command::new(&capture_script)
        .current_dir(markerless_cwd())
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CODEX_THREAD_ID", "codex-thread-test")
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args([
            "--title",
            "Derive Codex plugin session defaults",
            "--rationale",
            "The Codex path should capture decisions without manual actor or source setup",
            "--topic-keys",
            "codex,plugin,capture",
            "--options",
            "manual-provenance,session-context",
            "--chose",
            "session-context",
            "--bet",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "codex plugin capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Captured HiveMind decision decision-"));
    assert!(stdout.contains(actor_id));

    let query_script = root.join("plugins/hivemind-capture/scripts/query-decisions.sh");
    let query = Command::new(&query_script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CODEX_THREAD_ID", "codex-thread-test")
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .args(["--q", "session defaults", "--limit", "5"])
        .output()?;
    assert!(
        query.status.success(),
        "codex plugin query failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query: Value = serde_json::from_slice(&query.stdout)?;
    assert_eq!(query["result_count"], 1);
    assert_eq!(
        query["data"]["ranked"]["items"][0]["graph_context"]["actor_ids"][0],
        actor_id
    );

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn capture_plugin_defaults_to_rig_ledger_from_linked_worktree() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = unique_temp_dir("hivemind-plugin-linked-worktree")?;
    let rig = scratch.join("rig");
    let worktree = scratch.join("worktree");
    fs::create_dir_all(&rig)?;
    fs::write(rig.join("README.md"), "fixture repository\n")?;
    run_git(&rig, ["init"])?;
    run_git(&rig, ["add", "README.md"])?;
    run_git(
        &rig,
        [
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "init",
        ],
    )?;
    let worktree_arg = worktree.to_str().ok_or("worktree path should be UTF-8")?;
    run_git(&rig, ["worktree", "add", "--detach", worktree_arg])?;

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    let output = Command::new(&capture_script)
        .current_dir(&worktree)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("CODEX_THREAD_ID", "linked-worktree-test")
        .env_remove("HIVEMIND_DIR")
        .env_remove("CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR")
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args([
            "--title",
            "Capture from linked worktree to rig ledger",
            "--rationale",
            "The plugin default should use the original repository root, not the transient worktree root",
            "--topic-keys",
            "codex,plugin,worktree",
            "--options",
            "rig-ledger,worktree-ledger",
            "--chose",
            "rig-ledger",
            "--bet",
        ])
        .output()?;
    require(
        output.status.success(),
        format!(
            "linked worktree capture failed: {}\nstdout: {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ),
    )?;

    let rig_hivemind_dir = rig.join("hivemind");
    let worktree_hivemind_dir = worktree.join("hivemind");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let rig_hivemind_dir_text = rig_hivemind_dir.to_string_lossy();
    let rig_text = rig.to_string_lossy();
    let worktree_text = worktree.to_string_lossy();
    require_contains(&stderr, "hivemind-dir resolved to", "capture stderr")?;
    require_contains(&stderr, rig_hivemind_dir_text.as_ref(), "capture stderr")?;
    require_contains(&stderr, rig_text.as_ref(), "capture stderr")?;
    require_contains(&stderr, worktree_text.as_ref(), "capture stderr")?;
    require(
        !worktree_hivemind_dir.join("ledger.sqlite").exists(),
        format!(
            "capture should not create a worktree-local ledger at {}",
            worktree_hivemind_dir.display()
        ),
    )?;

    let ledger = SqliteEventLedger::open(&rig_hivemind_dir)?;
    let events = ledger.read(0, 100)?;
    let captured_event = events
        .iter()
        .find(|event| {
            event.event_type == hivemind::events::EventType::DecisionProposed
                && event.payload.get("title").and_then(Value::as_str)
                    == Some("Capture from linked worktree to rig ledger")
        })
        .ok_or("rig ledger should contain the linked-worktree capture event")?;
    require_eq(
        captured_event.actor_id.as_str(),
        "agent:codex:linked-worktree-test",
        "rig ledger actor",
    )?;

    let query_script = root.join("plugins/hivemind-capture/scripts/query-decisions.sh");
    let query = Command::new(&query_script)
        .current_dir(&worktree)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("CODEX_THREAD_ID", "linked-worktree-test")
        .env_remove("HIVEMIND_DIR")
        .env_remove("CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR")
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .args(["--q", "linked worktree", "--limit", "5"])
        .output()?;
    require(
        query.status.success(),
        format!(
            "linked worktree query failed: {}\nstdout: {}",
            String::from_utf8_lossy(&query.stderr),
            String::from_utf8_lossy(&query.stdout)
        ),
    )?;
    let query_stderr = String::from_utf8_lossy(&query.stderr);
    require_contains(&query_stderr, "hivemind-dir resolved to", "query stderr")?;
    require_contains(
        &query_stderr,
        rig_hivemind_dir_text.as_ref(),
        "query stderr",
    )?;
    let query: Value = serde_json::from_slice(&query.stdout)?;
    let result_count = query
        .get("result_count")
        .and_then(Value::as_u64)
        .ok_or("query result_count should be an integer")?;
    require(
        result_count == 1,
        format!("query result_count should be 1, got {result_count}"),
    )?;
    let query_actor = query
        .get("data")
        .and_then(|data| data.get("ranked"))
        .and_then(|ranked| ranked.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("graph_context"))
        .and_then(|graph_context| graph_context.get("actor_ids"))
        .and_then(Value::as_array)
        .and_then(|actor_ids| actor_ids.first())
        .and_then(Value::as_str)
        .ok_or("query actor id should exist")?;
    require_eq(
        query_actor,
        "agent:codex:linked-worktree-test",
        "query actor",
    )?;

    let _ = fs::remove_dir_all(scratch);
    Ok(())
}

#[test]
fn unified_capture_script_records_evidence_with_agent_provenance() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = uuid::Uuid::new_v4().to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-evidence-capture-{unique}"));
    let actor_id = "agent:claude:evidence-test-session";

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture.sh");
    let output = Command::new(&capture_script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "evidence-test-session")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .args([
            "The plugin smoke test wrote a decision and queried it back",
            "--kind",
            "evidence",
        ])
        .output()?;
    require(
        output.status.success(),
        format!(
            "unified evidence capture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    require_contains(
        stdout.as_ref(),
        "Captured HiveMind evidence evidence-",
        "unified evidence capture prints evidence id",
    )?;
    require_contains(
        stdout.as_ref(),
        actor_id,
        "unified evidence capture prints actor id",
    )?;

    let event = event_with_type(&hivemind_dir, hivemind::events::EventType::EvidenceRecorded)?;
    require_eq(event.actor_id.as_str(), actor_id, "evidence actor id")?;
    require_eq(event.source.as_str(), "agent", "evidence source")?;
    // source_ref carries the raw session id (provenance), not a repeat of actor_id --
    // ambient derivation with no explicit session/actor pin (hivemind-zdsh.9).
    require_eq(
        event.source_ref.as_deref(),
        Some("evidence-test-session"),
        "evidence source_ref",
    )?;
    require_eq(
        event.payload.get("content").and_then(Value::as_str),
        Some("The plugin smoke test wrote a decision and queried it back"),
        "evidence content",
    )?;

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn unified_capture_script_uses_classifier_when_kind_is_omitted() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = uuid::Uuid::new_v4().to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-classified-capture-{unique}"));

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture.sh");
    let output = Command::new(&capture_script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CODEX_THREAD_ID", "classifier-thread-test")
        .env("HIVEMIND_CAPTURE_CLASSIFIER_JSON", r#"{"kind":"evidence"}"#)
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("GC_AGENT")
        .env_remove("GC_ALIAS")
        .env_remove("GC_RIG")
        .arg("The test failed with error E")
        .output()?;
    require(
        output.status.success(),
        format!(
            "classified capture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    require_contains(
        stdout.as_ref(),
        "Captured HiveMind evidence evidence-",
        "classified capture prints evidence id",
    )?;

    let event = event_with_type(&hivemind_dir, hivemind::events::EventType::EvidenceRecorded)?;
    require_eq(
        event.actor_id.as_str(),
        "agent:codex:classifier-thread-test",
        "classified actor id",
    )?;
    require_eq(event.source.as_str(), "agent", "classified source")?;
    require_eq(
        event.payload.get("content").and_then(Value::as_str),
        Some("The test failed with error E"),
        "classified content",
    )?;

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

#[test]
fn human_cli_emit_defaults_actor_and_source_from_git_email() -> TestResult<()> {
    let scratch = unique_temp_dir("hivemind-human-cli-default")?;
    let repo = scratch.join("repo");
    let hivemind_dir = scratch.join("ledger");
    fs::create_dir_all(&repo)?;
    run_git(&repo, ["init"])?;
    run_git(
        &repo,
        ["config", "user.email", "Ada.Example+Decisions@Example.COM"],
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .current_dir(&repo)
        .env_remove("HIVEMIND_ACTOR")
        .arg("--json")
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .args([
            "emit",
            "decision.proposed",
            "--title",
            "Use git identity for human CLI writes",
            "--rationale",
            "Bare terminal writes should still carry human provenance",
            "--topic-keys",
            "cli,provenance",
            "--options",
            "git-email,manual-actor",
            "--chose",
            "git-email",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "human CLI emit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: Value = serde_json::from_slice(&output.stdout)?;
    let event = proposal_event(
        &hivemind_dir,
        output["value"].as_str().expect("decision id"),
    )?;
    assert_eq!(event.actor_id, "human:ada.example-decisions@example.com");
    assert_eq!(event.source.as_str(), "human");
    assert_eq!(
        event.source_ref.as_deref(),
        Some("human:ada.example-decisions@example.com")
    );

    let _ = fs::remove_dir_all(scratch);
    Ok(())
}

fn assert_mcp_pins_shared_ledger(mcp: &Value) {
    let server = &mcp["mcpServers"]["hivemind"];
    assert_eq!(server["command"], "hivemind");
    assert_eq!(server["env"]["HIVEMIND_DIR"], "./hivemind/");

    let args = server["args"].as_array().expect("mcp args");
    assert!(
        args.windows(2)
            .any(|window| window[0] == "--hivemind-dir" && window[1] == "./hivemind/"),
        "mcp args should pin --hivemind-dir ./hivemind/: {args:?}"
    );
    assert!(
        args.iter().any(|arg| arg == "mcp"),
        "mcp args should run the mcp subcommand: {args:?}"
    );
}

fn read_json(path: impl AsRef<Path>) -> TestResult<Value> {
    let path = path.as_ref();
    let input = fs::read_to_string(path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("{} is readable: {error}", path.display()),
        )
    })?;
    let value = serde_json::from_str(&input).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is valid json: {error}", path.display()),
        )
    })?;
    Ok(value)
}

#[test]
fn capture_script_forwards_grounding_flags_and_refuses_an_ungrounded_capture() -> TestResult<()> {
    // hivemind-gwhr.2: every decision capture must say what it rests on. The plugin script has to
    // carry the value-taking grounding flags (and `--bet`'s optional statement) through to
    // `emit decision.capture`, and a capture that names nothing must fail loudly (exit 2) with
    // nothing written.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let hivemind_dir = unique_temp_dir("hivemind-capture-script-grounding")?;
    let script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    let base = [
        "--title",
        "Adopt the grounded capture script",
        "--rationale",
        "The plugin script should forward every grounding flag to the CLI",
        "--topic-keys",
        "capture,grounding",
        "--options",
        "forward,drop",
        "--chose",
        "forward",
    ];
    let run_script = |extra: &[&str]| -> TestResult<std::process::Output> {
        Ok(Command::new(&script)
            .current_dir(markerless_cwd())
            .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
            .env("HIVEMIND_DIR", &hivemind_dir)
            .env("CLAUDE_PROJECT_DIR", root)
            .env("CLAUDE_SESSION_ID", "grounding-script-session")
            .env_remove("GC_AGENT")
            .env_remove("GC_ALIAS")
            .env_remove("GC_RIG")
            .args(base)
            .args(extra)
            .output()?)
    };

    let refused = run_script(&[])?;
    assert_eq!(
        refused.status.code(),
        Some(2),
        "an ungrounded capture must exit 2: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("must say what it rests on"));
    assert_eq!(
        SqliteEventLedger::open(&hivemind_dir)?.read(0, 100)?.len(),
        0,
        "a refused capture writes nothing"
    );

    let captured = run_script(&[
        "--rests-on-evidence",
        "the forwarded flags reached the ledger",
        "--evidence-source",
        "ci run 9",
        "--rests-on-assumption",
        "The script keeps forwarding unknown flags",
        "--bet",
        "The CLI keeps its grounding flag names",
        "--would-change-if",
        "the CLI renames a flag",
        "--check-by",
        "2026-12-01",
        "--confidence",
        "high",
    ])?;
    assert!(
        captured.status.success(),
        "grounded plugin capture failed: {}",
        String::from_utf8_lossy(&captured.stderr)
    );
    let stdout = String::from_utf8_lossy(&captured.stdout);
    assert!(
        stdout.contains("Captured HiveMind decision decision-"),
        "{stdout}"
    );

    let evidence = event_with_type(&hivemind_dir, hivemind::events::EventType::EvidenceRecorded)?;
    assert_eq!(evidence.payload["source"], "ci run 9");
    let bet = SqliteEventLedger::open(&hivemind_dir)?
        .read(0, 100)?
        .into_iter()
        .find(|event| {
            event.event_type == hivemind::events::EventType::HypothesisRecorded
                && event.payload.get("kind").and_then(Value::as_str) == Some("bet")
        })
        .ok_or("the bet should have been recorded")?;
    assert_eq!(
        bet.payload["statement"],
        "The CLI keeps its grounding flag names"
    );
    assert_eq!(bet.payload["would_change_if"], "the CLI renames a flag");
    let proposal = event_with_type(&hivemind_dir, hivemind::events::EventType::DecisionProposed)?;
    assert_eq!(proposal.payload["expressed_confidence"], "high");

    let _ = fs::remove_dir_all(hivemind_dir);
    Ok(())
}

/// hivemind-s15q.15: the capture scripts ask the CLI to work a decision's project out from the
/// folder they run in (the nearest `.hivemind-project` walking up). Tests that are not about
/// projects run from here instead of the repository root, where this rig's own marker sits: the
/// fresh test ledger has never registered that project, so a capture from the root would be
/// refused.
fn markerless_cwd() -> std::path::PathBuf {
    std::env::temp_dir()
}

#[test]
fn supersede_script_works_out_the_project_from_the_folder_it_runs_in() -> TestResult<()> {
    // hivemind-s15q.15: supersede records a new decision, so the context plugin's script hands the
    // CLI the same "work the project out from where this runs" switch the capture script does. It
    // is a flag of the `supersede` subcommand: it must land after the subcommand name.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = unique_temp_dir("hivemind-supersede-project")?;
    let hivemind_dir = scratch.join("hivemind");
    let hivemind = |args: &[&str]| -> TestResult<std::process::Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_hivemind"))
            .arg("--hivemind-dir")
            .arg(&hivemind_dir)
            .args(["--actor", "human:test-registrar"])
            .args(args)
            .output()?)
    };
    for handle in ["billing", "payments"] {
        let registered = hivemind(&["project", "register", handle])?;
        require(
            registered.status.success(),
            format!(
                "register {handle} failed: {}",
                String::from_utf8_lossy(&registered.stderr)
            ),
        )?;
        // A registered project's topic keys are declared (hivemind-zywz): declare the ones the
        // fixture captures below use.
        let declared = hivemind(&["project", "declare-topic", handle, "billing", "cadence"])?;
        require(
            declared.status.success(),
            format!(
                "declare topics for {handle} failed: {}",
                String::from_utf8_lossy(&declared.stderr)
            ),
        )?;
    }

    let script_env = |command: &mut Command| {
        command
            .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
            .env("HIVEMIND_DIR", &hivemind_dir)
            .env("CLAUDE_SESSION_ID", "supersede-project-session")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("GC_AGENT")
            .env_remove("GC_ALIAS")
            .env_remove("GC_RIG");
    };
    let capture_script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    for title in ["Ship invoices weekly", "Ship refunds daily"] {
        let mut capture = Command::new(&capture_script);
        script_env(&mut capture);
        let output = capture
            .current_dir(markerless_cwd())
            .args([
                "--title",
                title,
                "--rationale",
                "The fixture decision the supersede script will replace under a project",
                "--topic-keys",
                "billing,cadence",
                "--options",
                "weekly,daily",
                "--chose",
                "weekly",
                "--bet",
                "--project",
                "billing",
            ])
            .output()?;
        require(
            output.status.success(),
            format!(
                "fixture capture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )?;
    }

    let supersede_script = root.join("plugins/hivemind-context/scripts/supersede.sh");
    let supersede = |old: &str, title: &str, cwd: &Path| -> TestResult<String> {
        let mut command = Command::new(&supersede_script);
        script_env(&mut command);
        let output = command
            .current_dir(cwd)
            .args([
                old,
                "--title",
                title,
                "--rationale",
                "Replacing the fixture decision to see which project the replacement lands in",
                "--topic-keys",
                "billing,cadence",
                "--options",
                "monthly,daily",
                "--chose",
                "monthly",
                "--bet",
            ])
            .output()?;
        require(
            output.status.success(),
            format!(
                "supersede failed: {}\nstdout: {}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            ),
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };

    // From a folder attached to another project, the replacement goes there.
    let attached = scratch.join("attached");
    fs::create_dir_all(&attached)?;
    fs::write(attached.join(".hivemind-project"), "payments\n")?;
    let marked = supersede("Ship invoices weekly", "Ship invoices monthly", &attached)?;
    require_contains(
        &marked,
        "project=payments project_source=folder_marker",
        "supersede from a marker folder",
    )?;

    // From an unattached folder nothing is found, so the replacement stays where the old one was.
    let bare = scratch.join("bare");
    fs::create_dir_all(&bare)?;
    let inherited = supersede("Ship refunds daily", "Ship refunds monthly", &bare)?;
    require_contains(
        &inherited,
        "project=billing",
        "supersede from a bare folder",
    )?;

    let _ = fs::remove_dir_all(scratch);
    Ok(())
}

fn unique_temp_dir(label: &str) -> TestResult<std::path::PathBuf> {
    let unique = uuid::Uuid::new_v4().to_string();
    let path = std::env::temp_dir().join(format!("{label}-{unique}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> TestResult<()> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn proposal_event(hivemind_dir: &Path, decision_id: &str) -> TestResult<hivemind::events::Event> {
    let ledger = SqliteEventLedger::open(hivemind_dir)?;
    let events = ledger.read(0, 100)?;
    events
        .into_iter()
        .find(|event| {
            event.event_type == hivemind::events::EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(Value::as_str) == Some(decision_id)
        })
        .ok_or_else(|| format!("proposal event for {decision_id} should exist").into())
}

fn event_with_type(
    hivemind_dir: &Path,
    event_type: hivemind::events::EventType,
) -> TestResult<hivemind::events::Event> {
    let ledger = SqliteEventLedger::open(hivemind_dir)?;
    let events = ledger.read(0, 100)?;
    events
        .into_iter()
        .find(|event| event.event_type == event_type)
        .ok_or_else(|| format!("{event_type:?} event should exist").into())
}

fn require(condition: bool, message: impl Into<String>) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

fn require_contains(haystack: &str, needle: &str, message: &str) -> TestResult<()> {
    require(
        haystack.contains(needle),
        format!("{message}: missing {needle:?}"),
    )
}

fn require_eq<T>(actual: T, expected: T, message: &str) -> TestResult<()>
where
    T: PartialEq + std::fmt::Debug,
{
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{message}: expected {expected:?}, got {actual:?}").into())
    }
}

fn assert_no_todos(name: &str, body: &str) {
    assert!(
        !body.contains("[TODO"),
        "{name} should not contain scaffold TODO placeholders"
    );
}

fn verify_active_capture_skill(name: &str, body: &str) -> TestResult<()> {
    assert_no_todos(name, body);
    require_contains(body, "name: active-capture", name)?;
    require_contains(body, "comparing alternatives", name)?;
    require_contains(
        body,
        "/capture <text> [--kind decision|evidence|hypothesis|blocker]",
        name,
    )?;
    require_contains(body, "`decision`", name)?;
    require_contains(body, "`evidence`", name)?;
    require_contains(body, "`hypothesis`", name)?;
    require_contains(body, "`blocker`", name)?;
    require_contains(
        body,
        "Do NOT call this for synthetic test data or routing chatter",
        name,
    )?;
    require_contains(body, "gc sling", name)?;
    require_contains(body, "br update", name)?;
    require_contains(body, "do not write", name)?;
    require_contains(body, "directly to the ledger from this skill", name)?;
    Ok(())
}

fn assert_executable(path: impl AsRef<Path>) -> TestResult<()> {
    let path = path.as_ref();
    let metadata = fs::metadata(path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("{} has metadata: {error}", path.display()),
        )
    })?;
    #[cfg(unix)]
    {
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "{} should be executable",
            path.display()
        );
    }
    #[cfg(not(unix))]
    {
        assert!(
            metadata.is_file(),
            "{} should be a regular file",
            path.display()
        );
    }
    Ok(())
}
