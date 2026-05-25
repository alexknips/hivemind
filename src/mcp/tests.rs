// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::ledger::EventLedger;
use std::io::Cursor;
use std::path::PathBuf;

fn unique_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("hivemind-mcp-{label}-{}", uuid::Uuid::new_v4()));
    dir
}

fn drive(config: &McpConfig, requests: &[&str]) -> Vec<Value> {
    let mut input = String::new();
    for req in requests {
        input.push_str(req);
        input.push('\n');
    }
    let mut output: Vec<u8> = Vec::new();
    serve(config, Cursor::new(input.as_bytes()), &mut output).expect("server loop");
    let text = String::from_utf8(output).expect("utf-8 output");
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("response is json"))
        .collect()
}

#[test]
fn initialize_reports_server_metadata() {
    let dir = unique_dir("init");
    let config = McpConfig::new(&dir).with_session_id("test-session");
    let responses = drive(
        &config,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#],
    );
    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tools_list_includes_all_eleven_tools() {
    let dir = unique_dir("list");
    let config = McpConfig::new(&dir).with_session_id("test-session");
    let responses = drive(
        &config,
        &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#],
    );
    assert_eq!(responses.len(), 1);
    let tools = responses[0]["result"]["tools"].as_array().expect("array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("string name"))
        .collect();
    for expected in [
        "capture_decision",
        "capture_evidence",
        "capture_hypothesis",
        "disagree_decision",
        "supersede_decision",
        "get_decision",
        "get_relevant_decisions",
        "get_supersession_chain",
        "search_decisions",
        "recent_decisions",
        "dump_graph",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn notifications_produce_no_response() {
    let dir = unique_dir("notify");
    let config = McpConfig::new(&dir).with_session_id("test-session");
    let responses = drive(
        &config,
        &[r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#],
    );
    assert!(
        responses.is_empty(),
        "notifications must not produce output"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_then_get_round_trips_a_decision() {
    let dir = unique_dir("roundtrip");
    let config = McpConfig::new(&dir).with_session_id("roundtrip-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:1",
                "title": "Use SQLite for ledger",
                "rationale": "Local-first storage is enough for v1",
                "topic_keys": ["storage"],
                "options": [
                    {"label": "sqlite"},
                    {"label": "postgres"}
                ],
                "chosen_option_label": "sqlite"
            }
        }
    })
    .to_string();

    let responses = drive(&config, &[capture.as_str()]);
    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], serde_json::Value::Bool(false));
    let structured = &result["structuredContent"];
    let decision_id = structured["decision_id"].as_str().expect("decision_id");
    assert!(decision_id.starts_with("decision-"), "id = {decision_id}");

    let fetch = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "get_decision",
            "arguments": { "decision_id": decision_id }
        }
    })
    .to_string();

    let responses = drive(&config, &[fetch.as_str()]);
    let structured = &responses[0]["result"]["structuredContent"];
    let data = &structured["data"];
    assert_eq!(data["id"].as_str(), Some(decision_id));
    assert_eq!(data["title"].as_str(), Some("Use SQLite for ledger"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_tools_default_actor_to_configured_agent_session() {
    let dir = unique_dir("default-actor");
    let config = McpConfig::new(&dir)
        .with_agent_tool("claude")
        .with_session_id("session-123");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "title": "Default MCP actor",
                "rationale": "MCP write tools should not require per-call actor boilerplate",
                "topic_keys": ["capture"],
                "options": [{"label": "default"}, {"label": "explicit"}],
                "chosen_option_label": "default"
            }
        }
    })
    .to_string();

    let responses = drive(&config, &[capture.as_str()]);
    assert_eq!(
        responses[0]["result"]["isError"],
        serde_json::Value::Bool(false)
    );

    let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
    let events = crate::ledger::EventLedger::read(&ledger, 0, 16).expect("events read");
    let proposal = events
        .iter()
        .find(|event| event.event_type == crate::events::EventType::DecisionProposed)
        .expect("proposal exists");
    assert_eq!(proposal.actor_id, "agent:claude:session-123");
    assert_eq!(proposal.source, crate::events::EventSource::Agent);
    assert_eq!(
        proposal.source_ref.as_deref(),
        Some("agent:claude:session-123")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_decisions_tool_returns_fts_query_response() {
    let dir = unique_dir("search");
    let config = McpConfig::new(&dir).with_session_id("search-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:search",
                "title": "Adopt authentication boundary",
                "rationale": "OAuth routing keeps decision search anchored",
                "topic_keys": ["security"],
                "options": [
                    {"label": "gateway"},
                    {"label": "sidecar"}
                ],
                "chosen_option_label": "gateway"
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[capture.as_str()]);
    let decision_id = responses[0]["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id")
        .to_owned();

    let search = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "search_decisions",
            "arguments": {
                "q": "gateway",
                "topic": ["security"],
                "actor_id": ["agent:test:search"],
                "limit": 5
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[search.as_str()]);
    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["result_count"], serde_json::json!(1));
    assert_eq!(
        structured["data"]["items"][0]["decision"]["id"],
        decision_id
    );
    assert_eq!(
        structured["data"]["items"][0]["matched_fields"],
        serde_json::json!(["option.id"])
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recent_decisions_tool_returns_recent_query_response() {
    let dir = unique_dir("recent-decisions");
    let config = McpConfig::new(&dir).with_session_id("recent-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:recent",
                "title": "Keep recent decisions discoverable",
                "rationale": "Agents need a bounded recent decisions query",
                "topic_keys": ["query"],
                "options": [{"label": "recent_decisions"}],
                "chosen_option_label": "recent_decisions"
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[capture.as_str()]);
    let decision_id = responses[0]["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id")
        .to_owned();

    let recent = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "recent_decisions",
            "arguments": {
                "since": "1970-01-01T00:00:00Z",
                "topic": ["query"],
                "actor": ["agent:test:recent"],
                "status": ["proposed"],
                "limit": 5
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[recent.as_str()]);
    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["result_count"], serde_json::json!(1));
    assert_eq!(
        structured["data"]["items"][0]["decision_id"],
        serde_json::json!(decision_id)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disagree_decision_tool_contests_and_defaults_actor() {
    let dir = unique_dir("disagree");
    let config = McpConfig::new(&dir).with_session_id("disagree-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:1",
                "title": "Keep auth as-is",
                "rationale": "Avoids migration work",
                "topic_keys": ["auth"],
                "options": [{"label": "keep"}]
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[capture.as_str()]);
    let decision_id = responses[0]["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision id")
        .to_owned();

    let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
    Commands::new(&ledger)
        .accept_decision(&decision_id, "actor:bob")
        .expect("accept succeeds");

    let disagree = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "disagree_decision",
            "arguments": {
                "decision_id": decision_id,
                "reason": "misses auth implications"
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[disagree.as_str()]);
    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(
        structured["decision_status"],
        serde_json::json!("contested")
    );
    let event_id = structured["event_id"].as_u64().expect("event id");

    let events = ledger.read(0, 20).expect("events read");
    let rejected = events
        .iter()
        .find(|event| event.event_id == Some(event_id))
        .expect("rejected event");
    assert_eq!(rejected.actor_id, "agent:codex:disagree-session");
    assert_eq!(rejected.source, crate::events::EventSource::Agent);
    assert_eq!(
        rejected.payload.get("reason").and_then(Value::as_str),
        Some("misses auth implications")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn supersede_decision_tool_marks_old_and_is_idempotent() {
    let dir = unique_dir("supersede");
    let config = McpConfig::new(&dir).with_session_id("supersede-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:1",
                "title": "Use shared admin token",
                "rationale": "Fastest path",
                "topic_keys": ["auth"],
                "options": [{"label": "shared-token"}]
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[capture.as_str()]);
    let old_decision_id = responses[0]["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision id")
        .to_owned();

    let supersede = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "supersede_decision",
            "arguments": {
                "old_decision_id": old_decision_id,
                "title": "Use scoped service tokens",
                "rationale": "Scoped tokens preserve audit boundaries",
                "options": [{"label": "scoped-service-tokens"}],
                "chosen_option_label": "scoped-service-tokens"
            }
        }
    })
    .to_string();
    let first = drive(&config, &[supersede.as_str()]);
    let first_structured = &first[0]["result"]["structuredContent"];
    assert_eq!(
        first_structured["old_decision_status"],
        serde_json::json!("superseded")
    );
    assert_eq!(
        first_structured["new_decision_status"],
        serde_json::json!("proposed")
    );

    let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let second = drive(&config, &[supersede.as_str()]);
    let second_structured = &second[0]["result"]["structuredContent"];
    assert_eq!(
        second_structured["new_decision_id"],
        first_structured["new_decision_id"]
    );
    assert_eq!(
        second_structured["superseded_event_id"],
        first_structured["superseded_event_id"]
    );
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_required_field_reports_invalid_params() {
    let dir = unique_dir("missing");
    let config = McpConfig::new(&dir).with_session_id("test-session");
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_evidence",
            "arguments": { "actor_id": "agent:test:1" }
        }
    })
    .to_string();

    let responses = drive(&config, &[request.as_str()]);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], serde_json::Value::Bool(true));
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(
        text.contains("content"),
        "error mentions missing field: {text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_method_returns_minus_32601() {
    let dir = unique_dir("nomethod");
    let config = McpConfig::new(&dir).with_session_id("test-session");
    let responses = drive(
        &config,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"bogus/method"}"#],
    );
    assert_eq!(responses[0]["error"]["code"], JSONRPC_METHOD_NOT_FOUND);
}
