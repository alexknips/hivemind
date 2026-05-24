use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
    assert!(skill.contains("agent:codex:<session>"));
    assert!(skill.contains("HIVEMIND_DIR"));
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
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
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
    assert_eq!(
        mcp["mcpServers"]["hivemind"]["env"]["HIVEMIND_DIR"],
        "./hivemind/"
    );
    assert_mcp_pins_shared_ledger(&mcp);

    let capture_command =
        fs::read_to_string(root.join("plugins/hivemind-capture/commands/capture-decision.md"))?;
    assert_no_todos("Claude capture command", &capture_command);
    assert!(capture_command.contains("/hivemind-capture:query-decisions"));
    assert!(capture_command.contains("agent:claude:<session>"));
    assert!(capture_command.contains("${CLAUDE_PLUGIN_ROOT}/scripts/capture-decision.sh"));

    let query_command =
        fs::read_to_string(root.join("plugins/hivemind-capture/commands/query-decisions.md"))?;
    assert_no_todos("Claude query command", &query_command);
    assert!(query_command.contains("query search_decisions"));
    assert!(query_command.contains("truncated"));

    let readme = fs::read_to_string(root.join("plugins/hivemind-capture/README.md"))?;
    assert_no_todos("Claude plugin README", &readme);
    assert!(readme.contains("/plugin install hivemind-capture@hivemind"));
    assert!(readme.contains("/plugin uninstall hivemind-capture@hivemind"));
    assert!(readme.contains("/hivemind-capture:capture-decision"));

    assert_executable(root.join("plugins/hivemind-capture/scripts/capture-decision.sh"))?;
    assert_executable(root.join("plugins/hivemind-capture/scripts/query-decisions.sh"))?;
    Ok(())
}

#[test]
fn claude_code_plugin_capture_and_query_scripts_write_agent_decision() -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let hivemind_dir = std::env::temp_dir().join(format!("hivemind-claude-plugin-{unique}"));
    let actor_id = "agent:claude:plugin-test-session";

    let capture_script = root.join("plugins/hivemind-capture/scripts/capture-decision.sh");
    let output = Command::new(&capture_script)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", &hivemind_dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .env("CLAUDE_SESSION_ID", "plugin-test-session")
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
        query["data"]["items"][0]["graph_context"]["actor_ids"][0],
        actor_id
    );

    let _ = fs::remove_dir_all(hivemind_dir);
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
        .env("CODEX_SESSION_ID", "plugin-test-session")
        .arg("--json")
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .args([
            "emit",
            "decision.capture",
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
    assert_eq!(
        event.source_ref.as_deref(),
        Some("agent:codex:plugin-test-session")
    );

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

fn unique_temp_dir(label: &str) -> TestResult<std::path::PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
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

fn assert_no_todos(name: &str, body: &str) {
    assert!(
        !body.contains("[TODO"),
        "{name} should not contain scaffold TODO placeholders"
    );
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
