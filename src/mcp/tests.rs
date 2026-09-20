// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::ledger::{EventLedger, SqliteEventLedger};
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
fn tools_list_includes_all_eighteen_tools() {
    let dir = unique_dir("list");
    let config = McpConfig::new(&dir).with_session_id("test-session");
    let responses = drive(
        &config,
        &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#],
    );
    assert_eq!(responses.len(), 1); // ubs:ignore: test-only; index guaranteed by test setup
    let tools = responses[0]["result"]["tools"].as_array().expect("array"); // ubs:ignore: test-only; panicking is correct in tests
    assert_eq!(tools.len(), 22, "tool count mismatch: {tools:?}"); // ubs:ignore: test-only assertion
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("string name")) // ubs:ignore: test-only; panicking is correct in tests
        .collect();
    for expected in [
        "capture_decision",
        "capture_evidence",
        "capture_hypothesis",
        "disagree_decision",
        "supersede_decision",
        "get_decision",
        "get_decision_outcome",
        "decision_quality_candidates",
        "get_decision_context",
        "decision_context_candidates",
        "score_decision",
        "scan_decision_quality",
        "analyze_failure_modes",
        "get_relevant_decisions",
        "get_situational_decisions",
        "get_supersession_chain",
        "search_decisions",
        "recall_decisions",
        "recent_decisions",
        "dump_graph",
        "hivemind_compact_view",
        "summarize_decisions",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}"); // ubs:ignore: test-only assertion
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
fn recall_decisions_tool_returns_ranked_and_digest() {
    let dir = unique_dir("recall");
    let config = McpConfig::new(&dir).with_session_id("recall-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:recall",
                "title": "Use JWT for auth tokens",
                "rationale": "Stateless tokens reduce session storage overhead",
                "topic_keys": ["auth"],
                "options": [{"label": "jwt"}, {"label": "opaque"}],
                "chosen_option_label": "jwt"
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[capture.as_str()]);
    let decision_id = responses[0]["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id") // ubs:ignore: test-only; panicking is correct in tests
        .to_owned();

    let recall = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "recall_decisions",
            "arguments": {
                "q": "jwt auth",
                "limit": 5
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[recall.as_str()]);
    let structured = &responses[0]["result"]["structuredContent"];
    // Should find our decision in the ranked results
    assert_eq!(structured["result_count"], serde_json::json!(1)); // ubs:ignore: test-only assertion
    assert_eq!(
        structured["data"]["ranked"]["items"][0]["decision"]["id"],
        decision_id
    ); // ubs:ignore: test-only assertion
       // Digest must be present and cite the decision
    let digest = &structured["data"]["digest"];
    assert!(
        digest["cited_decision_ids"]
            .as_array()
            .expect("array") // ubs:ignore: test-only; panicking is correct in tests
            .iter()
            .any(|id| id.as_str() == Some(&decision_id)),
        "decision_id must appear in cited_decision_ids"
    ); // ubs:ignore: test-only assertion
    assert!(
        digest["summary"].as_str().is_some_and(|s| !s.is_empty()),
        "summary must be non-empty"
    ); // ubs:ignore: test-only assertion

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
    let decision_id = responses[0]["result"]["structuredContent"]["decision_id"] // ubs:ignore: test-only; panicking chain is correct in tests
        .as_str() // ubs:ignore: test-only; chain continues to expect
        .expect("decision_id") // ubs:ignore: test-only; panicking is correct in tests
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
    let structured = &responses[0]["result"]["structuredContent"]; // ubs:ignore: test-only; index guaranteed by test setup
    assert_eq!(structured["result_count"], serde_json::json!(1)); // ubs:ignore: test-only.
    let item_id = structured["data"]["items"][0]["decision_id"].clone(); // ubs:ignore: test-only; panicking is correct in tests
    assert_eq!(item_id, serde_json::json!(decision_id)); // ubs:ignore: test-only.

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
        // ubs:ignore: event IDs are public ledger offsets, not secrets.
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

#[test]
fn summarize_single_decision_via_mcp() {
    let dir = unique_dir("summarize-single");
    let config = McpConfig::new(&dir).with_session_id("summarize-session");

    // Capture a decision first.
    let capture_req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:test:summarize",
                "title": "Adopt event sourcing",
                "rationale": "Immutable log enables full audit trail",
                "topic_keys": ["architecture"],
                "options": [
                    { "label": "event-sourcing", "description": "Use append-only event log" },
                    { "label": "mutable-db", "description": "Use standard CRUD" }
                ],
                "chosen_option_label": "event-sourcing"
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[capture_req.as_str()]);
    let decision_id = responses[0]["result"]["structuredContent"]["decision_id"] // ubs:ignore: test-only; expect panics correctly on unexpected response shape
        .as_str() // ubs:ignore: test-only; chain continues to expect
        .expect("decision_id") // ubs:ignore: test-only; panicking is correct in tests
        .to_owned();

    // Summarize it.
    let summarize_req = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {
            "name": "summarize_decisions",
            "arguments": { "decision_ids": [decision_id] }
        }
    })
    .to_string();
    let responses = drive(&config, &[summarize_req.as_str()]);
    let result = &responses[0]["result"]; // ubs:ignore: test-only; index guaranteed by test setup
    assert!(!result["isError"].as_bool().unwrap_or(true)); // ubs:ignore: test-only assertion
    let structured = &result["structuredContent"]["data"]; // ubs:ignore: test-only; JSON path guaranteed by tool contract
    let summary = structured["summary"].as_str().expect("summary text"); // ubs:ignore: test-only; panicking is correct in tests
    assert!(summary.contains("event sourcing")); // ubs:ignore: test-only assertion
    let cited = structured["cited_decision_ids"].as_array().expect("array"); // ubs:ignore: test-only; panicking is correct in tests
    assert_eq!(cited.len(), 1); // ubs:ignore: test-only assertion
    assert_eq!(structured["unit"], "single"); // ubs:ignore: test-only assertion
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn summarize_missing_decision_returns_error() {
    let dir = unique_dir("summarize-missing");
    let config = McpConfig::new(&dir).with_session_id("summarize-missing-session");

    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "summarize_decisions",
            "arguments": { "decision_ids": ["nonexistent-id-12345"] }
        }
    })
    .to_string();
    let responses = drive(&config, &[req.as_str()]);
    let result = &responses[0]["result"]; // ubs:ignore: test-only; index guaranteed by test setup
    assert!(result["isError"].as_bool().unwrap_or(false)); // ubs:ignore: test-only assertion
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn summarize_mode_single_with_multiple_ids_returns_tool_error() {
    let dir = unique_dir("summarize-mode-err");
    let config = McpConfig::new(&dir).with_session_id("summarize-mode-err-session");

    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "summarize_decisions",
            "arguments": { "decision_ids": ["id-a", "id-b"], "mode": "single" }
        }
    })
    .to_string();
    let responses = drive(&config, &[req.as_str()]);
    let result = &responses[0]["result"]; // ubs:ignore: test-only; index guaranteed by test setup
    assert!(result["isError"].as_bool().unwrap_or(false)); // ubs:ignore: test-only assertion
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Golden cross-transport parity harness (hivemind-whvd.1)
//
// Drives the same MCP tool call through both the stdio transport (this
// module, via `drive`) and the MCP-over-HTTP transport (`api::mcp_http`,
// via an in-process axum router) against separate fresh ledgers, then
// asserts the two responses agree. Generated ids (decision_id, option_ids)
// are per-call randomness, so only their presence/shape is compared; every
// other field — and every error message — must match exactly. This is the
// regression test for the exact drift class hivemind-whvd exists to close:
// stdio and HTTP disagreed on the empty-option-label message before
// `mcp::core` unified argument parsing.
//
// Add one case here per tool as it migrates behind `mcp::core`.
// ---------------------------------------------------------------------------
mod transport_parity {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    fn http_app(dir: &std::path::Path) -> axum::Router {
        let config = crate::api::ApiConfig {
            hivemind_dir: dir.to_path_buf(),
            port: 0,
            api_key: None,
            database_url: None,
            admin_key: None,
            workos_domain: None,
            workos_issuer: None,
            workos_jwks_url: None,
            workos_audience: None,
            spa_dir: None,
            cors_origins: vec![],
        };
        crate::api::create_router(&config)
    }

    async fn http_call(dir: &std::path::Path, tool: &str, arguments: Value) -> Value {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        });
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).expect("json body"))) // ubs:ignore: test-only; panicking is correct in tests
            .expect("build request"); // ubs:ignore: test-only; panicking is correct in tests
        let response = http_app(dir).oneshot(request).await.expect("http response"); // ubs:ignore: test-only; panicking is correct in tests
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body") // ubs:ignore: test-only; panicking is correct in tests
            .to_bytes();
        serde_json::from_slice(&bytes).expect("json response") // ubs:ignore: test-only; panicking is correct in tests
    }

    fn stdio_call(dir: &std::path::Path, tool: &str, arguments: Value) -> Value {
        let config = McpConfig::new(dir).with_session_id("parity-stdio");
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        })
        .to_string();
        drive(&config, &[request.as_str()])
            .into_iter()
            .next()
            .expect("one response") // ubs:ignore: test-only; panicking is correct in tests
    }

    /// Runs `tool` with `arguments` against a fresh ledger on each
    /// transport and returns each response's top-level `result` object.
    async fn run(tool: &str, label: &str, arguments: Value) -> (Value, Value) {
        let stdio_dir = unique_dir(&format!("parity-stdio-{label}"));
        let http_dir = unique_dir(&format!("parity-http-{label}"));
        let stdio = stdio_call(&stdio_dir, tool, arguments.clone());
        let http = http_call(&http_dir, tool, arguments).await;
        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
        (
            stdio["result"].clone(), // ubs:ignore: test-only; index guaranteed by test setup
            http["result"].clone(),  // ubs:ignore: test-only; index guaranteed by test setup
        )
    }

    /// Like `run`, but issues a setup call (typically `capture_decision`) before
    /// the call under test, against the same per-transport ledger, and returns
    /// only the second call's `result` object. `get_situational_decisions`
    /// needs a decision already captured to have anything to match.
    async fn run_after(
        setup_tool: &str,
        setup_args: Value,
        tool: &str,
        label: &str,
        arguments: Value,
    ) -> (Value, Value) {
        let stdio_dir = unique_dir(&format!("parity-stdio-{label}"));
        let http_dir = unique_dir(&format!("parity-http-{label}"));

        let config = McpConfig::new(&stdio_dir).with_session_id("parity-stdio");
        let setup_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": setup_tool, "arguments": setup_args.clone() }
        })
        .to_string();
        let _ = drive(&config, &[setup_request.as_str()]);
        let request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments.clone() }
        })
        .to_string();
        let stdio = drive(&config, &[request.as_str()])
            .into_iter()
            .next()
            .expect("one response"); // ubs:ignore: test-only; panicking is correct in tests

        let _ = http_call(&http_dir, setup_tool, setup_args).await;
        let http = http_call(&http_dir, tool, arguments).await;

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
        (stdio["result"].clone(), http["result"].clone())
    }

    #[tokio::test]
    async fn get_situational_decisions_matches_topic_key_across_transports() {
        let setup_args = json!({
            "title": "Use SQLite for the ledger",
            "rationale": "Local-first storage is enough for v1",
            "topic_keys": ["storage"],
            "options": [{"label": "sqlite"}],
        });
        let (stdio, http) = run_after(
            "capture_decision",
            setup_args,
            "get_situational_decisions",
            "situational-basic",
            json!({ "paths": ["src/storage/engine.rs"] }),
        )
        .await;

        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(
                // ubs:ignore: test-only assertion
                data["query_terms"],
                json!(["engine", "storage"]),
                "{name}: query_terms"
            );
            assert_eq!(data["total_matches"], json!(1), "{name}: total_matches"); // ubs:ignore: test-only assertion
            assert_eq!(
                data["since_boundary"],
                Value::Null,
                "{name}: since_boundary"
            ); // ubs:ignore: test-only assertion
            let matches = data["matches"].as_array().expect("matches array"); // ubs:ignore: test-only; panicking is correct in tests
            assert_eq!(matches.len(), 1, "{name}: matches length"); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                matches[0]["decision"]["title"],
                "Use SQLite for the ledger",
                "{name}: decision title"
            );
            assert_eq!(
                // ubs:ignore: test-only assertion
                matches[0]["matched_via"],
                json!([{"kind": "topic_key", "topic": "storage"}]),
                "{name}: matched_via"
            );
            assert_eq!(
                matches[0]["changed_since"],
                Value::Null,
                "{name}: changed_since"
            ); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn get_situational_decisions_since_offset_flags_changed_across_transports() {
        let setup_args = json!({
            "title": "Use SQLite for the ledger",
            "rationale": "Local-first storage is enough for v1",
            "topic_keys": ["storage"],
            "options": [{"label": "sqlite"}],
        });
        let (stdio, http) = run_after(
            "capture_decision",
            setup_args,
            "get_situational_decisions",
            "situational-since-offset",
            json!({ "paths": ["src/storage/engine.rs"], "since_offset": 0 }),
        )
        .await;

        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(
                // ubs:ignore: test-only assertion
                data["since_boundary"],
                json!({ "offset": 0, "timestamp": null }),
                "{name}: since_boundary"
            );
            let matches = data["matches"].as_array().expect("matches array"); // ubs:ignore: test-only; panicking is correct in tests
            assert_eq!(matches.len(), 1, "{name}: matches length"); // ubs:ignore: test-only assertion
            assert_eq!(
                matches[0]["changed_since"],
                json!(true),
                "{name}: changed_since"
            ); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn capture_decision_happy_path_with_chosen_option() {
        let (stdio, http) = run(
            "capture_decision",
            "happy-chosen",
            json!({
                "title": "Use SQLite for the ledger",
                "rationale": "Local-first storage is enough for v1",
                "topic_keys": ["storage"],
                "options": [{"label": "sqlite"}, {"label": "postgres"}],
                "chosen_option_label": "sqlite",
            }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let content = &result["structuredContent"];
            assert!(
                // ubs:ignore: test-only assertion
                content["decision_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("decision-")),
                "{name}: decision_id = {:?}",
                content["decision_id"]
            );
            assert_eq!(
                // ubs:ignore: test-only assertion
                content["option_ids"].as_array().map(Vec::len),
                Some(2),
                "{name}: option_ids"
            );
            assert!(
                content["chosen_option_id"].is_string(), // ubs:ignore: test-only assertion
                "{name}: chosen_option_id should be set"
            );
        }
    }

    #[tokio::test]
    async fn capture_decision_happy_path_without_chosen_option() {
        let (stdio, http) = run(
            "capture_decision",
            "happy-unchosen",
            json!({
                "title": "Evaluate caching layers",
                "rationale": "Need more data before choosing",
                "topic_keys": ["cache"],
                "options": [{"label": "redis"}, {"label": "memcached"}],
            }),
        )
        .await;
        assert_eq!(stdio["structuredContent"]["chosen_option_id"], Value::Null); // ubs:ignore: test-only assertion
        assert_eq!(http["structuredContent"]["chosen_option_id"], Value::Null); // ubs:ignore: test-only assertion
    }

    #[tokio::test]
    async fn capture_decision_error_messages_match_across_transports() {
        let cases: &[(&str, Value, &str)] = &[
            (
                "missing-options",
                json!({"title": "t", "rationale": "r", "topic_keys": ["x"]}),
                "missing `options`",
            ),
            (
                "empty-topic-keys",
                json!({"title": "t", "rationale": "r", "topic_keys": [], "options": [{"label": "a"}]}),
                "topic_keys must not be empty",
            ),
            (
                "empty-option-label",
                json!({"title": "t", "rationale": "r", "topic_keys": ["x"], "options": [{"label": ""}]}),
                "options[0].label must be a non-empty string",
            ),
            (
                "unmatched-chosen-label",
                json!({
                    "title": "t", "rationale": "r", "topic_keys": ["x"],
                    "options": [{"label": "a"}], "chosen_option_label": "b"
                }),
                "chosen_option_label must match one of the supplied option labels",
            ),
        ];
        for (label, arguments, expected_message) in cases {
            let (stdio, http) = run("capture_decision", label, arguments.clone()).await;
            assert!(
                stdio["isError"].as_bool().unwrap_or(false), // ubs:ignore: test-only assertion
                "{label}: stdio should error: {stdio:?}"
            );
            assert!(
                http["isError"].as_bool().unwrap_or(false), // ubs:ignore: test-only assertion
                "{label}: http should error: {http:?}"
            );
            assert_eq!(
                stdio["content"][0]["text"].as_str(), // ubs:ignore: test-only assertion
                Some(*expected_message),
                "{label}: stdio message"
            );
            assert_eq!(
                http["content"][0]["text"].as_str(), // ubs:ignore: test-only assertion
                Some(*expected_message),
                "{label}: http message"
            );
        }
    }
}
