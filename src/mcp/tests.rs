// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
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
fn tools_list_includes_all_seven_tools() {
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
        "get_decision",
        "get_relevant_decisions",
        "get_supersession_chain",
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
