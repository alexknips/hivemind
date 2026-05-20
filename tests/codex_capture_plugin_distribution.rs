use std::fs;
use std::path::Path;

use serde_json::Value;

#[test]
fn codex_capture_plugin_bundle_is_installable_and_points_at_cli_capture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let marketplace = read_json(root.join(".agents/plugins/marketplace.json"));
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

    let manifest = read_json(root.join("plugins/hivemind-capture/.codex-plugin/plugin.json"));
    assert_eq!(manifest["name"], "hivemind-capture");
    assert_eq!(manifest["skills"], "./skills/");
    assert_no_todos("plugin manifest", &manifest.to_string());

    let skill =
        fs::read_to_string(root.join("plugins/hivemind-capture/skills/hivemind-capture/SKILL.md"))
            .expect("skill is readable");
    assert_no_todos("skill", &skill);
    assert!(skill.contains("decision.capture"));
    assert!(skill.contains("--agent-tool codex"));
    assert!(skill.contains("agent:codex:<session>"));
    assert!(skill.contains("HIVEMIND_DIR"));
}

fn read_json(path: impl AsRef<Path>) -> Value {
    let path = path.as_ref();
    serde_json::from_str(&fs::read_to_string(path).expect("json file is readable"))
        .unwrap_or_else(|error| panic!("{} is valid json: {error}", path.display()))
}

fn assert_no_todos(name: &str, body: &str) {
    assert!(
        !body.contains("[TODO"),
        "{name} should not contain scaffold TODO placeholders"
    );
}
