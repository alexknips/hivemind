use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
    assert_no_todos("plugin manifest", &manifest.to_string());

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
    assert_eq!(settings["env"]["HIVEMIND_DIR"], "./hivemind");
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
        "${CLAUDE_PROJECT_DIR}/hivemind"
    );
    assert_no_todos("Claude plugin manifest", &manifest.to_string());

    let mcp = read_json(root.join("plugins/hivemind-capture/.mcp.json"))?;
    assert_eq!(mcp["mcpServers"]["hivemind"]["command"], "hivemind");
    assert!(mcp["mcpServers"]["hivemind"]["args"]
        .as_array()
        .expect("mcp args")
        .iter()
        .any(|arg| arg == "mcp"));
    assert_eq!(
        mcp["mcpServers"]["hivemind"]["env"]["HIVEMIND_DIR"],
        "${user_config.hivemind_dir}"
    );

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
