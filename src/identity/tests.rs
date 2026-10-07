use super::{actor_component, agent_tool_from_client_name};

#[test]
fn client_names_map_to_the_tool_names_the_flag_uses() {
    for (client, tool) in [
        ("Claude Code", "claude"),
        ("claude-code", "claude"),
        ("claude-ai", "claude"),
        ("codex", "codex"),
        ("codex-mcp-client", "codex"),
        ("Cursor", "cursor"),
        ("cursor-vscode", "cursor"),
    ] {
        assert_eq!(
            agent_tool_from_client_name(client).as_deref(),
            Some(tool),
            "{client}"
        );
    }
}

#[test]
fn other_client_names_are_kept_in_the_actor_id_charset() {
    assert_eq!(agent_tool_from_client_name("foo").as_deref(), Some("foo"));
    assert_eq!(
        agent_tool_from_client_name(" My Agent: v2 ").as_deref(),
        Some("my-agent--v2")
    );
    // A name that merely starts like a known client is not that client.
    assert_eq!(
        agent_tool_from_client_name("claudette").as_deref(),
        Some("claudette")
    );
}

#[test]
fn a_client_name_with_nothing_usable_names_no_tool() {
    assert_eq!(agent_tool_from_client_name(""), None);
    assert_eq!(agent_tool_from_client_name("  "), None);
    assert_eq!(agent_tool_from_client_name(" -- "), None);
}

#[test]
fn a_very_long_client_name_is_cut() {
    let tool = agent_tool_from_client_name(&"a".repeat(500)).expect("a tool");
    assert_eq!(tool.len(), 64);
}

#[test]
fn actor_component_normalizes_git_identity_for_actor_ids() {
    assert_eq!(
        actor_component(" Ada.Example+Decisions@Example.COM "),
        "ada.example-decisions@example.com"
    );
    assert_eq!(actor_component(" -- "), "local-user");
}
