use std::process::Command;

const MANUAL_AGENT_SESSION: &str = "manual-session";

pub fn default_actor() -> String {
    env_value("HIVEMIND_ACTOR").unwrap_or_else(default_human_actor_id)
}

pub fn default_human_actor_id() -> String {
    if let Some(actor_id) =
        env_value("HIVEMIND_ACTOR").filter(|value| value.trim().starts_with("human:"))
    {
        return actor_id;
    }

    let raw = git_config_value("user.email")
        .or_else(|| git_config_value("user.name"))
        .or_else(|| env_value("USER"))
        .unwrap_or_else(|| "local-user".to_owned());
    format!("human:{}", actor_component(&raw))
}

pub fn default_agent_tool() -> String {
    agent_tool_from_env().unwrap_or_else(|| "codex".to_owned())
}

/// The agent tool the environment names or implies, with no fallback: `None` means
/// nothing in the environment says which agent (if any) is running this process.
fn agent_tool_from_env() -> Option<String> {
    env_value("HIVEMIND_AGENT_TOOL")
        .or_else(|| env_value("HIVEMIND_TOOL"))
        .or_else(|| {
            if any_env_present(&[
                "HIVEMIND_CLAUDE_SESSION",
                "CLAUDE_SESSION_ID",
                "CLAUDE_CODE_SESSION_ID",
            ]) {
                Some("claude".to_owned())
            } else if any_env_present(&[
                "HIVEMIND_CODEX_SESSION",
                "CODEX_SESSION_ID",
                "CODEX_TASK_ID",
            ]) {
                Some("codex".to_owned())
            } else {
                None
            }
        })
}

/// Whether the environment carries any evidence that an agent, rather than a person at a
/// terminal, is running this process: a named tool, or a stable or per-run agent session.
/// `default_agent_tool` and `default_agent_session` fall back to `codex` and `manual-session`
/// when there is none, which names an agent nobody saw -- so a caller that must not invent
/// one (a person's `emit decision.capture`, hivemind-6ait) asks this first.
pub fn agent_present_in_env() -> bool {
    agent_tool_from_env().is_some() || agent_session_from_env("codex").is_some()
}

pub fn default_agent_session(tool: &str) -> String {
    agent_session_from_env(tool).unwrap_or_else(|| MANUAL_AGENT_SESSION.to_owned())
}

/// A durable name for "who" this process runs as, stable across restarts of the same
/// logical agent slot. Gas City assigns every crew/polecat/refinery slot a fixed name
/// (`GC_AGENT`, mirrored in `GC_ALIAS`) that survives process restarts; a raw session
/// id (`CLAUDE_SESSION_ID`, `CODEX_SESSION_ID`, ...) does not -- it's freshly generated
/// on every run. Folding a raw session id into the actor name makes the same physical
/// agent appear as a different actor on every restart (hivemind-zdsh.9); this is the
/// stable alternative `agent_session_from_env` prefers before falling back to one.
pub fn stable_agent_identity() -> Option<String> {
    env_value("GC_AGENT").or_else(|| env_value("GC_ALIAS"))
}

pub fn agent_session_from_env(tool: &str) -> Option<String> {
    env_value("HIVEMIND_AGENT_SESSION")
        .or_else(stable_agent_identity)
        .or_else(|| raw_agent_session_from_env(tool))
}

/// The literal per-run session id, ignoring `HIVEMIND_AGENT_SESSION` and the stable
/// Gas City identity `agent_session_from_env` prefers. Unlike that stable name, this
/// value is different on every run -- exactly the kind of ephemeral detail that
/// belongs in provenance (`source_ref`), never in `actor_id` itself.
pub fn raw_agent_session_from_env(tool: &str) -> Option<String> {
    let normalized_tool = tool.trim().to_ascii_lowercase();
    let tool_specific = match normalized_tool.as_str() {
        "claude" => env_value("HIVEMIND_CLAUDE_SESSION")
            .or_else(|| env_value("CLAUDE_SESSION_ID"))
            .or_else(|| env_value("CLAUDE_CODE_SESSION_ID")),
        "codex" => env_value("HIVEMIND_CODEX_SESSION")
            .or_else(|| env_value("CODEX_SESSION_ID"))
            .or_else(|| env_value("CODEX_TASK_ID")),
        _ => None,
    };

    tool_specific
        .or_else(|| env_value("HIVEMIND_CLAUDE_SESSION"))
        .or_else(|| env_value("CLAUDE_SESSION_ID"))
        .or_else(|| env_value("CLAUDE_CODE_SESSION_ID"))
        .or_else(|| env_value("HIVEMIND_CODEX_SESSION"))
        .or_else(|| env_value("CODEX_SESSION_ID"))
        .or_else(|| env_value("CODEX_TASK_ID"))
}

pub fn agent_actor_id(tool: &str, session: &str) -> String {
    format!("agent:{}:{}", tool.trim(), session.trim())
}

pub(crate) fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn any_env_present(keys: &[&str]) -> bool {
    keys.iter().any(|key| env_value(key).is_some())
}

fn git_config_value(key: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn actor_component(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    for byte in raw.trim().bytes() {
        let byte = byte.to_ascii_lowercase();
        match byte {
            b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'@' | b'-' => {
                normalized.push(char::from(byte));
            }
            _ => normalized.push('-'),
        }
    }

    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        "local-user".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::actor_component;

    #[test]
    fn actor_component_normalizes_git_identity_for_actor_ids() {
        assert_eq!(
            actor_component(" Ada.Example+Decisions@Example.COM "),
            "ada.example-decisions@example.com"
        );
        assert_eq!(actor_component(" -- "), "local-user");
    }
}
