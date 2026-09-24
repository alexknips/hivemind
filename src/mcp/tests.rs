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
    assert_eq!(tools.len(), 27, "tool count mismatch: {tools:?}"); // ubs:ignore: test-only assertion
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
        "move_decision",
        "get_decision",
        "get_decision_outcome",
        "decision_quality_candidates",
        "get_decision_context",
        "decision_context_candidates",
        "score_decision",
        "scan_decision_quality",
        "scan_misfiled_decisions",
        "analyze_failure_modes",
        "get_relevant_decisions",
        "get_situational_decisions",
        "get_supersession_chain",
        "get_decision_neighborhood",
        "search_decisions",
        "recall_decisions",
        "recent_decisions",
        "dump_graph",
        "hivemind_compact_view",
        "summarize_decisions",
        "classify_queue_list",
        "classify_queue_submit",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}"); // ubs:ignore: test-only assertion
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_and_supersede_tools_require_a_grounding_array() {
    let tools = crate::mcp::tool_definitions();
    for name in ["capture_decision", "supersede_decision"] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("tool {name} is listed"));
        let schema = &tool["inputSchema"];
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            required.contains(&"grounding"),
            "{name}: grounding is required"
        );

        let grounding = &schema["properties"]["grounding"];
        assert_eq!(grounding["type"], "array", "{name}");
        assert_eq!(grounding["minItems"], 1, "{name}: at least one answer");
        let kinds: Vec<&str> = grounding["items"]["oneOf"]
            .as_array()
            .expect("oneOf items")
            .iter()
            .filter_map(|item| item["properties"]["kind"]["const"].as_str())
            .collect();
        assert_eq!(
            kinds,
            ["decision", "evidence", "assumption", "bet"],
            "{name}"
        );

        assert_eq!(
            schema["properties"]["expressed_confidence"]["enum"],
            json!(["low", "medium", "high"]),
            "{name}"
        );
        for alias in ["hypothesis_ids", "evidence_ids"] {
            assert!(
                schema["properties"][alias]["description"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("Deprecated alias")),
                "{name}: {alias} is documented as a deprecated alias"
            );
        }
    }
}

#[test]
fn capture_decision_refuses_a_call_that_names_nothing_it_rests_on() {
    let dir = unique_dir("no-grounding");
    let config = McpConfig::new(&dir).with_session_id("no-grounding-session");
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "title": "Adopt the new queue",
                "rationale": "Rationale text long enough for the readable floor",
                "topic_keys": ["queue"],
                "options": [{"label": "adopt"}]
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[request.as_str()]);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true, "{result:?}");
    let text = result["content"][0]["text"].as_str().expect("error text");
    assert!(
        text.contains("a captured decision must say what it rests on"),
        "{text}"
    );
    let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
    assert_eq!(
        ledger.latest_offset().expect("latest offset"),
        0,
        "a refused capture writes nothing"
    );
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
                "grounding": [{"kind": "bet"}],
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
fn capture_decision_with_decided_by_advances_to_accepted_with_correct_authorship() {
    // hivemind-zdsh.3: an agent scribing a decision a human actually made must record the
    // human as decider (accepted_by) while staying the recorder (proposed_by) itself — not
    // leave the decision stuck at `proposed`, and not attribute the decision to itself.
    let dir = unique_dir("decided-by");
    let config = McpConfig::new(&dir).with_session_id("scribe-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:claude:hivemind-crew",
                "title": "Ledger must distinguish who decided from who recorded",
                "rationale": "If the human delegates small decisions to agents, we track that; when the agent asks the human to choose and the human does, the human made the decision.",
                "topic_keys": ["governance"],
                "options": [
                    {"label": "Keep recorder and decider as the same actor"},
                    {"label": "Let capture name decided_by separately from the recording actor"}
                ],
                "chosen_option_label": "Let capture name decided_by separately from the recording actor",
                "decided_by": "human:alex.knips@gmail.com"
            }
        }
    })
    .to_string();

    let responses = drive(&config, &[capture.as_str()]);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], serde_json::Value::Bool(false));
    let structured = &result["structuredContent"];
    let decision_id = structured["decision_id"]
        .as_str()
        .expect("decision_id")
        .to_owned();
    assert_eq!(
        structured["decided_by"].as_str(),
        Some("human:alex.knips@gmail.com")
    );

    let get_decision = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "get_decision", "arguments": { "decision_id": decision_id } }
    })
    .to_string();
    let responses = drive(&config, &[get_decision.as_str()]);
    let data = &responses[0]["result"]["structuredContent"]["data"];
    assert_eq!(
        data["status"].as_str(),
        Some("accepted"),
        "a decision captured with decided_by must not be left at proposed: {data:?}"
    );

    let get_context = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "get_decision_context", "arguments": { "decision_id": decision_id } }
    })
    .to_string();
    let responses = drive(&config, &[get_context.as_str()]);
    let data = &responses[0]["result"]["structuredContent"]["data"];
    assert_eq!(
        data["authorship"].as_str(),
        Some("agent_proposed_human_accepted"),
        "recorder must stay the proposer while the human is the acceptor: {data:?}"
    );
    assert_eq!(
        data["proposer_id"].as_str(),
        Some("agent:claude:hivemind-crew")
    );
    assert_eq!(data["review"].as_str(), Some("peer_reviewed"));
    assert_eq!(data["accepted_count"].as_i64(), Some(1));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_decision_decided_by_without_chosen_option_is_rejected() {
    let dir = unique_dir("decided-by-invalid");
    let config = McpConfig::new(&dir).with_session_id("scribe-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:claude:hivemind-crew",
                "title": "Decided by without a choice",
                "rationale": "decided_by with no chosen option should be rejected",
                "topic_keys": ["governance"],
                "options": [{"label": "A"}, {"label": "B"}],
                "decided_by": "human:alex.knips@gmail.com"
            }
        }
    })
    .to_string();

    let responses = drive(&config, &[capture.as_str()]);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], serde_json::Value::Bool(true));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_decision_quote_without_question_is_rejected() {
    let dir = unique_dir("quote-invalid");
    let config = McpConfig::new(&dir).with_session_id("scribe-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:claude:hivemind-crew",
                "title": "Quote with no stated question",
                "rationale": "a quote with no question should be rejected",
                "topic_keys": ["governance"],
                "options": [{"label": "A"}],
                "quote": "1a"
            }
        }
    })
    .to_string();

    let responses = drive(&config, &[capture.as_str()]);
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], serde_json::Value::Bool(true));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_decision_stores_paired_quote_and_question() {
    let dir = unique_dir("quote-valid");
    let config = McpConfig::new(&dir).with_session_id("scribe-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:claude:hivemind-crew",
                "title": "Personal projects are visible tenant-wide",
                "rationale": "Spelled out: a personal project is visible to the whole tenant.",
                "topic_keys": ["projects"],
                "options": [{"label": "A"}],
                "quote": "1a",
                "question": "Should a personal project be visible to the whole tenant?"
            }
        }
    })
    .to_string();

    let responses = drive(&config, &[capture.as_str()]);
    let result = &responses[0]["result"];
    assert_ne!(result.get("isError"), Some(&serde_json::Value::Bool(true)));

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
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
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
        // hivemind-zdsh.10: option ids are opaque now (no longer a slug of the label), so
        // option.id no longer matches "gateway". The MCP tool's auto-generated description
        // ("Option generated from MCP value 'gateway'") does, alongside option.label.
        serde_json::json!(["option.description", "option.label"])
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
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:test:recent",
                "title": "Keep recent decisions discoverable",
                "rationale": "Agents need a bounded recent decisions query",
                "topic_keys": ["query"],
                "options": [{"label": "recent_decisions"}]
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
    // Pin agent_tool explicitly (hivemind-zdsh.9): mcp_actor_id now reflects
    // config.agent_tool instead of always hardcoding "codex", so this test's
    // "agent:codex:..." expectation must not depend on ambient
    // CLAUDE_SESSION_ID/CLAUDE_CODE_SESSION_ID leaking into McpConfig::new's
    // env-derived default_agent_tool() (as it does when this suite runs
    // inside a Claude Code session).
    let config = McpConfig::new(&dir)
        .with_agent_tool("codex")
        .with_session_id("disagree-session");

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:test:1",
                "title": "Keep auth as-is",
                "rationale": "Avoids migration work and keeps the schema stable",
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
fn disagree_decision_tool_ambiguous_description_does_not_write() {
    let dir = unique_dir("disagree-ambiguous");
    let config = McpConfig::new(&dir).with_session_id("disagree-ambiguous-session");

    for topic in ["billing", "notifications"] {
        let capture = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "capture_decision",
                "arguments": {
                    "grounding": [{"kind": "bet"}],
                    "title": format!("Adopt async queue for {topic}"),
                    "rationale": "because those are the reasons we discussed",
                    "topic_keys": [topic],
                    "options": [{"label": "async"}]
                }
            }
        })
        .to_string();
        drive(&config, &[capture.as_str()]);
    }

    let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
    let offset_before = ledger.latest_offset().expect("offset before");

    let disagree = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "disagree_decision",
            "arguments": {
                "description": "adopt async queue",
                "reason": "should not apply to either"
            }
        }
    })
    .to_string();
    let responses = drive(&config, &[disagree.as_str()]);
    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(
        structured["data"]["outcome"],
        serde_json::json!("ambiguous")
    );
    assert_eq!(
        structured["data"]["candidates"].as_array().map(Vec::len),
        Some(2)
    );

    assert_eq!(
        ledger.latest_offset().expect("offset after"),
        offset_before,
        "ambiguous disagree must not append any event"
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
                "grounding": [{"kind": "bet"}],
                "actor_id": "agent:test:1",
                "title": "Use shared admin token",
                "rationale": "Fastest path to ship given the deadline",
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
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
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
            bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            allow_unauthenticated_remote: false,
            api_key: None,
            database_url: None,
            admin_key: None,
            workos_domain: None,
            workos_issuer: None,
            workos_jwks_url: None,
            workos_audience: None,
            spa_dir: None,
            cors_origins: vec![],
            slack_client_id: None,
            slack_client_secret: None,
            slack_signing_secret: None,
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

    /// Like `run_after`, but `build_args` receives the setup call's
    /// generated `decision_id` to build the second call's arguments. Ids are
    /// generated per-ledger, so each transport substitutes its own — stdio's
    /// id never crosses over to the http call and vice versa.
    async fn run_after_with_id(
        setup_tool: &str,
        setup_args: Value,
        tool: &str,
        label: &str,
        build_args: impl Fn(&str) -> Value,
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
        let setup_response = drive(&config, &[setup_request.as_str()])
            .into_iter()
            .next()
            .expect("one response"); // ubs:ignore: test-only; panicking is correct in tests
        let stdio_id = setup_response["result"]["structuredContent"]["decision_id"]
            .as_str()
            .expect("decision_id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": build_args(&stdio_id) }
        })
        .to_string();
        let stdio = drive(&config, &[request.as_str()])
            .into_iter()
            .next()
            .expect("one response"); // ubs:ignore: test-only; panicking is correct in tests

        let setup_http = http_call(&http_dir, setup_tool, setup_args).await;
        let http_id = setup_http["result"]["structuredContent"]["decision_id"]
            .as_str()
            .expect("decision_id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned();
        let http = http_call(&http_dir, tool, build_args(&http_id)).await;

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
        (stdio["result"].clone(), http["result"].clone())
    }

    #[tokio::test]
    async fn get_situational_decisions_matches_topic_key_across_transports() {
        let setup_args = json!({
            "grounding": [{"kind": "bet"}],
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
            "grounding": [{"kind": "bet"}],
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
    async fn recall_decisions_matches_topic_across_transports() {
        let setup_args = json!({
            "grounding": [{"kind": "bet"}],
            "title": "Use SQLite for the ledger",
            "rationale": "Local-first storage is enough for v1",
            "topic_keys": ["storage"],
            "options": [{"label": "sqlite"}],
        });
        let (stdio, http) = run_after(
            "capture_decision",
            setup_args,
            "recall_decisions",
            "recall-topic",
            json!({ "topic": ["storage"] }),
        )
        .await;

        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(data["query"], Value::Null, "{name}: query"); // ubs:ignore: test-only assertion
            assert_eq!(
                data["ranked"]["total_matches"],
                json!(1),
                "{name}: total_matches"
            ); // ubs:ignore: test-only assertion
            let items = data["ranked"]["items"].as_array().expect("items array"); // ubs:ignore: test-only; panicking is correct in tests
            assert_eq!(items.len(), 1, "{name}: items length"); // ubs:ignore: test-only assertion
            assert_eq!(
                items[0]["decision"]["title"], "Use SQLite for the ledger",
                "{name}: decision title"
            ); // ubs:ignore: test-only assertion
            let cited = data["digest"]["cited_decision_ids"]
                .as_array()
                .expect("cited_decision_ids array"); // ubs:ignore: test-only; panicking is correct in tests
            assert_eq!(cited.len(), 1, "{name}: cited_decision_ids length"); // ubs:ignore: test-only assertion
            assert_eq!(
                cited[0], items[0]["decision"]["id"],
                "{name}: cited id matches ranked item id"
            ); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn capture_decision_happy_path_with_chosen_option() {
        let (stdio, http) = run(
            "capture_decision",
            "happy-chosen",
            json!({
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
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
                    "options": [{"label": "a"}], "chosen_option_label": "b",
                    "grounding": [{"kind": "bet"}]
                }),
                "chosen_option_label must match one of the supplied option labels",
            ),
            (
                "missing-grounding",
                json!({"title": "t", "rationale": "r", "topic_keys": ["x"], "options": [{"label": "a"}]}),
                crate::grounding::WIRE_GROUNDING_REFUSAL,
            ),
            (
                "empty-grounding",
                json!({"title": "t", "rationale": "r", "topic_keys": ["x"], "options": [{"label": "a"}], "grounding": []}),
                crate::grounding::WIRE_GROUNDING_REFUSAL,
            ),
            (
                "malformed-grounding-item",
                json!({
                    "title": "t", "rationale": "r", "topic_keys": ["x"],
                    "options": [{"label": "a"}],
                    "grounding": [{"kind": "decision", "description": "x", "decision_id": "decision-1"}]
                }),
                "`grounding[0]` of kind `decision` needs exactly one of `description` or `decision_id`",
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

    /// Like `run`, but first captures one decision per `titles` entry on each
    /// transport's own fresh ledger via `capture_decision`, then runs `tool`.
    /// Used to seed the graph `get_decision_neighborhood` resolves against.
    async fn run_seeded(
        tool: &str,
        label: &str,
        titles: &[&str],
        arguments: Value,
    ) -> (Value, Value) {
        let stdio_dir = unique_dir(&format!("parity-stdio-{label}"));
        let http_dir = unique_dir(&format!("parity-http-{label}"));

        for title in titles {
            let seed = json!({
                "grounding": [{"kind": "bet"}],
                "title": title,
                "rationale": "seed for get_decision_neighborhood parity test",
                "topic_keys": ["parity"],
                "options": [{"label": "only"}],
            });
            stdio_call(&stdio_dir, "capture_decision", seed.clone());
            http_call(&http_dir, "capture_decision", seed).await;
        }

        let stdio = stdio_call(&stdio_dir, tool, arguments.clone());
        let http = http_call(&http_dir, tool, arguments).await;
        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
        (
            stdio["result"].clone(), // ubs:ignore: test-only; index guaranteed by test setup
            http["result"].clone(),  // ubs:ignore: test-only; index guaranteed by test setup
        )
    }

    #[tokio::test]
    async fn get_decision_neighborhood_resolves_unique_description() {
        let (stdio, http) = run_seeded(
            "get_decision_neighborhood",
            "why-resolved",
            &["Adopt async billing queue"],
            json!({ "description": "adopt async billing queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(data["root"]["present"], true, "{name}: root present"); // ubs:ignore: test-only assertion
            assert_eq!(data["root"]["kind"], "decision", "{name}: root kind"); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn get_decision_neighborhood_ambiguous_description_returns_candidates_not_error() {
        let (stdio, http) = run_seeded(
            "get_decision_neighborhood",
            "why-ambiguous",
            &[
                "Adopt async queue for billing",
                "Adopt async queue for notifications",
            ],
            json!({ "description": "adopt async queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(
                structured["data"]["outcome"], "ambiguous",
                "{name}: outcome"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
        }
    }

    #[tokio::test]
    async fn get_decision_neighborhood_not_found_description_returns_envelope_not_error() {
        let (stdio, http) = run_seeded(
            "get_decision_neighborhood",
            "why-not-found",
            &["Adopt async billing queue"],
            json!({ "description": "totally unrelated widget factory zzz" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
        }
    }

    #[tokio::test]
    async fn get_decision_neighborhood_missing_selector_errors_identically() {
        let (stdio, http) = run(
            "get_decision_neighborhood",
            "why-missing-selector",
            json!({}),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert!(
                result["isError"].as_bool().unwrap_or(false), // ubs:ignore: test-only assertion
                "{name}: missing selector should error: {result:?}"
            );
        }
        assert_eq!(
            stdio["content"][0]["text"],
            http["content"][0]["text"], // ubs:ignore: test-only assertion
            "missing-selector message must match across transports"
        );
        assert_eq!(
            stdio["content"][0]["text"].as_str(), // ubs:ignore: test-only assertion
            Some("one of `decision_id` or `description` is required"),
            "missing-selector message text"
        );
    }

    /// Reopens the on-disk ledger under `dir` (same sqlite file either
    /// transport writes) and returns its latest offset, to prove a call
    /// wrote nothing.
    fn ledger_offset(dir: &std::path::Path) -> crate::events::EventId {
        let ledger = SqliteEventLedger::open(dir).expect("ledger opens"); // ubs:ignore: test-only; panicking is correct in tests
        ledger.latest_offset().expect("latest offset") // ubs:ignore: test-only; panicking is correct in tests
    }

    #[tokio::test]
    async fn supersede_decision_resolves_by_id_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-supersede-by-id");
        let http_dir = unique_dir("parity-http-supersede-by-id");

        let seed = json!({
            "grounding": [{"kind": "bet"}],
            "title": "Use shared admin token",
            "rationale": "Fastest path to ship given the deadline",
            "topic_keys": ["auth"],
            "options": [{"label": "shared-token"}],
        });
        let stdio_capture = stdio_call(&stdio_dir, "capture_decision", seed.clone());
        let stdio_old_id = stdio_capture["result"]["structuredContent"]["decision_id"]
            .as_str() // ubs:ignore: test-only; chain continues to expect
            .expect("stdio decision id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned();
        let http_capture = http_call(&http_dir, "capture_decision", seed).await;
        let http_old_id = http_capture["result"]["structuredContent"]["decision_id"]
            .as_str() // ubs:ignore: test-only; chain continues to expect
            .expect("http decision id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned();

        let supersede_args = |old_decision_id: &str| {
            json!({
                "grounding": [{"kind": "bet"}],
                "old_decision_id": old_decision_id,
                "title": "Use scoped service tokens",
                "rationale": "Scoped tokens preserve audit boundaries",
                "options": [{"label": "scoped-service-tokens"}],
                "chosen_option_label": "scoped-service-tokens",
            })
        };
        let stdio = stdio_call(
            &stdio_dir,
            "supersede_decision",
            supersede_args(&stdio_old_id),
        );
        let http = http_call(
            &http_dir,
            "supersede_decision",
            supersede_args(&http_old_id),
        )
        .await;

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);

        for (name, result) in [("stdio", &stdio["result"]), ("http", &http["result"])] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let content = &result["structuredContent"];
            assert_eq!(
                content["old_decision_status"],
                "superseded", // ubs:ignore: test-only assertion
                "{name}: old_decision_status"
            );
            assert_eq!(
                content["new_decision_status"],
                "proposed", // ubs:ignore: test-only assertion
                "{name}: new_decision_status"
            );
            assert!(
                // ubs:ignore: test-only assertion
                content["new_decision_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("decision-")),
                "{name}: new_decision_id = {:?}",
                content["new_decision_id"]
            );
            assert!(
                content["superseded_event_id"].is_u64(), // ubs:ignore: test-only assertion
                "{name}: superseded_event_id should be set"
            );
        }
    }

    #[tokio::test]
    async fn supersede_decision_resolves_unique_description_across_transports() {
        let (stdio, http) = run_seeded(
            "supersede_decision",
            "supersede-unique-desc",
            &["Use shared admin token"],
            json!({
                "grounding": [{"kind": "bet"}],
                "description": "shared admin token",
                "title": "Use scoped service tokens",
                "rationale": "Scoped tokens preserve audit boundaries",
                "options": [{"label": "scoped-service-tokens"}],
            }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let content = &result["structuredContent"];
            assert_eq!(
                content["old_decision_status"], "superseded", // ubs:ignore: test-only assertion
                "{name}: old_decision_status — proves the description resolved and the write happened"
            );
            assert_eq!(
                content["new_decision_status"],
                "proposed", // ubs:ignore: test-only assertion
                "{name}: new_decision_status"
            );
        }
    }

    #[tokio::test]
    async fn supersede_decision_ambiguous_description_writes_nothing_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-supersede-ambiguous");
        let http_dir = unique_dir("parity-http-supersede-ambiguous");

        for title in [
            "Adopt async queue for billing",
            "Adopt async queue for notifications",
        ] {
            let seed = json!({
                "grounding": [{"kind": "bet"}],
                "title": title,
                "rationale": "seed for supersede_decision parity test",
                "topic_keys": ["parity"],
                "options": [{"label": "only"}],
            });
            stdio_call(&stdio_dir, "capture_decision", seed.clone());
            http_call(&http_dir, "capture_decision", seed).await;
        }

        let stdio_offset_before = ledger_offset(&stdio_dir);
        let http_offset_before = ledger_offset(&http_dir);

        let supersede_args = json!({
            "grounding": [{"kind": "bet"}],
            "description": "adopt async queue",
            "title": "Adopt async queue (v2)",
            "rationale": "must not be written — the match is ambiguous",
            "options": [{"label": "replacement"}],
        });
        let stdio = stdio_call(&stdio_dir, "supersede_decision", supersede_args.clone());
        let http = http_call(&http_dir, "supersede_decision", supersede_args).await;

        for (name, result) in [("stdio", &stdio["result"]), ("http", &http["result"])] {
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(
                structured["data"]["outcome"], "ambiguous",
                "{name}: outcome"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
        }

        assert_eq!(
            ledger_offset(&stdio_dir),
            stdio_offset_before,
            "stdio: ambiguous resolution must not write"
        );
        assert_eq!(
            ledger_offset(&http_dir),
            http_offset_before,
            "http: ambiguous resolution must not write"
        );

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn supersede_decision_not_found_description_writes_nothing_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-supersede-not-found");
        let http_dir = unique_dir("parity-http-supersede-not-found");

        let seed = json!({
            "grounding": [{"kind": "bet"}],
            "title": "Adopt async billing queue",
            "rationale": "seed for supersede_decision parity test",
            "topic_keys": ["parity"],
            "options": [{"label": "only"}],
        });
        stdio_call(&stdio_dir, "capture_decision", seed.clone());
        http_call(&http_dir, "capture_decision", seed).await;

        let stdio_offset_before = ledger_offset(&stdio_dir);
        let http_offset_before = ledger_offset(&http_dir);

        let supersede_args = json!({
            "grounding": [{"kind": "bet"}],
            "description": "totally unrelated widget factory zzz",
            "title": "Irrelevant replacement",
            "rationale": "must not be written — nothing matched",
            "options": [{"label": "replacement"}],
        });
        let stdio = stdio_call(&stdio_dir, "supersede_decision", supersede_args.clone());
        let http = http_call(&http_dir, "supersede_decision", supersede_args).await;

        for (name, result) in [("stdio", &stdio["result"]), ("http", &http["result"])] {
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
        }

        assert_eq!(
            ledger_offset(&stdio_dir),
            stdio_offset_before,
            "stdio: not-found resolution must not write"
        );
        assert_eq!(
            ledger_offset(&http_dir),
            http_offset_before,
            "http: not-found resolution must not write"
        );

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn get_decision_outcome_resolves_by_decision_id() {
        let setup_args = json!({
            "grounding": [{"kind": "bet"}],
            "title": "Adopt blue-green deploys",
            "rationale": "Zero-downtime releases keep users unaffected during deploys",
            "topic_keys": ["deploy"],
            "options": [{"label": "blue-green"}],
            "chosen_option_label": "blue-green",
        });
        let (stdio, http) = run_after_with_id(
            "capture_decision",
            setup_args,
            "get_decision_outcome",
            "outcome-by-id",
            |id| json!({ "decision_id": id }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(data["title"], "Adopt blue-green deploys", "{name}: title"); // ubs:ignore: test-only assertion
                                                                                    // hivemind-zdsh.3: capture_decision now carries option_labels through to the
                                                                                    // ledger, so the chosen option's real label survives — no more falling back to
                                                                                    // the generated option_id.
            assert_eq!(
                data["chosen_option"]["label"], "blue-green",
                "{name}: chosen_option label must be the real label, not the generated id"
            ); // ubs:ignore: test-only assertion
               // hivemind-zdsh.10: option ids are opaque now (no longer a slug of the label), so
               // this only checks the "option-" prefix shared with every generated option id.
            assert!(
                data["chosen_option"]["option_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("option-")),
                "{name}: chosen_option.option_id = {:?}",
                data["chosen_option"]["option_id"]
            ); // ubs:ignore: test-only assertion
            assert_eq!(data["still_holds"]["held_up"], true, "{name}: held_up");
            // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn get_decision_outcome_resolves_unique_description() {
        let (stdio, http) = run_seeded(
            "get_decision_outcome",
            "verify-resolved",
            &["Adopt async billing queue"],
            json!({ "description": "adopt async billing queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(data["title"], "Adopt async billing queue", "{name}: title");
            // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn get_supersession_chain_resolves_unique_description() {
        let (stdio, http) = run_seeded(
            "get_supersession_chain",
            "chain-resolved",
            &["Adopt async billing queue"],
            json!({ "description": "adopt async billing queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(
                data["decision_ids"].as_array().map(Vec::len),
                Some(1),
                "{name}: decision_ids length"
            ); // ubs:ignore: test-only assertion
            assert_eq!(data["input_index"], 0, "{name}: input_index"); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn get_decision_outcome_ambiguous_description_returns_candidates_not_error() {
        let (stdio, http) = run_seeded(
            "get_decision_outcome",
            "verify-ambiguous",
            &[
                "Adopt async queue for billing",
                "Adopt async queue for notifications",
            ],
            json!({ "description": "adopt async queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(
                structured["data"]["outcome"], "ambiguous",
                "{name}: outcome"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
        }
    }

    #[tokio::test]
    async fn get_supersession_chain_ambiguous_description_returns_candidates_not_error() {
        let (stdio, http) = run_seeded(
            "get_supersession_chain",
            "chain-ambiguous",
            &[
                "Adopt async queue for billing",
                "Adopt async queue for notifications",
            ],
            json!({ "description": "adopt async queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(
                structured["data"]["outcome"], "ambiguous",
                "{name}: outcome"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
        }
    }

    #[tokio::test]
    async fn get_decision_outcome_not_found_description_returns_envelope_not_error() {
        let (stdio, http) = run_seeded(
            "get_decision_outcome",
            "verify-not-found",
            &["Adopt async billing queue"],
            json!({ "description": "totally unrelated widget factory zzz" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
        }
    }

    #[tokio::test]
    async fn disagree_decision_resolves_by_id_across_transports() {
        let (stdio, http) = run_after_with_id(
            "capture_decision",
            json!({
                "grounding": [{"kind": "bet"}],
                "title": "Keep auth as-is",
                "rationale": "Avoids migration work and keeps the schema stable",
                "topic_keys": ["auth"],
                "options": [{"label": "keep"}],
            }),
            "disagree_decision",
            "disagree-by-id",
            |decision_id| {
                json!({
                    "decision_id": decision_id,
                    "reason": "misses auth implications",
                })
            },
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let content = &result["structuredContent"];
            assert_eq!(
                content["decision_status"],
                json!("rejected"), // ubs:ignore: test-only assertion
                "{name}: decision_status"
            );
            assert!(
                content["event_id"].is_number(), // ubs:ignore: test-only assertion
                "{name}: event_id should be present"
            );
        }
    }

    #[tokio::test]
    async fn disagree_decision_resolves_unique_description_and_writes() {
        let (stdio, http) = run_seeded(
            "disagree_decision",
            "disagree-unique-description",
            &["Adopt async billing queue"],
            json!({
                "description": "adopt async billing queue",
                "reason": "underestimates operational cost",
            }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let content = &result["structuredContent"];
            assert_eq!(
                content["decision_status"],
                json!("rejected"), // ubs:ignore: test-only assertion
                "{name}: decision_status"
            );
            assert!(
                // ubs:ignore: test-only assertion
                content["decision_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("decision-")),
                "{name}: decision_id = {:?}",
                content["decision_id"]
            );
        }
    }

    #[tokio::test]
    async fn disagree_decision_ambiguous_description_returns_candidates_not_error() {
        let (stdio, http) = run_seeded(
            "disagree_decision",
            "disagree-ambiguous",
            &[
                "Adopt async queue for billing",
                "Adopt async queue for notifications",
            ],
            json!({
                "description": "adopt async queue",
                "reason": "should not apply to either",
            }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(
                structured["data"]["outcome"], "ambiguous",
                "{name}: outcome"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
        }
    }

    #[tokio::test]
    async fn disagree_decision_not_found_description_returns_envelope_not_error() {
        let (stdio, http) = run_seeded(
            "disagree_decision",
            "disagree-not-found",
            &["Adopt async billing queue"],
            json!({
                "description": "totally unrelated widget factory zzz",
                "reason": "does not matter",
            }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
        }
    }

    #[tokio::test]
    async fn get_supersession_chain_not_found_description_returns_envelope_not_error() {
        let (stdio, http) = run_seeded(
            "get_supersession_chain",
            "chain-not-found",
            &["Adopt async billing queue"],
            json!({ "description": "totally unrelated widget factory zzz" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
        }
    }

    #[tokio::test]
    async fn get_supersession_chain_missing_selector_errors_identically() {
        let (stdio, http) = run(
            "get_supersession_chain",
            "chain-missing-selector",
            json!({}),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert!(
                result["isError"].as_bool().unwrap_or(false), // ubs:ignore: test-only assertion
                "{name}: missing selector should error: {result:?}"
            );
        }
        assert_eq!(
            stdio["content"][0]["text"],
            http["content"][0]["text"], // ubs:ignore: test-only assertion
            "missing-selector message must match across transports"
        );
        assert_eq!(
            stdio["content"][0]["text"].as_str(), // ubs:ignore: test-only assertion
            Some("one of `decision_id` or `description` is required"),
            "missing-selector message text"
        );
    }

    /// Like `run_seeded`, but captures the seeded decisions and returns each
    /// transport's own `decision_id`s alongside the tool's response —
    /// `hivemind_compact_view`'s by-id case needs a real id per transport,
    /// since stdio and http each get a fresh, independently-assigned ledger.
    async fn run_seeded_with_ids(
        titles: &[&str],
        tool: &str,
        label: &str,
        arguments_from_id: impl Fn(&str) -> Value,
    ) -> (Value, Value) {
        let stdio_dir = unique_dir(&format!("parity-stdio-{label}"));
        let http_dir = unique_dir(&format!("parity-http-{label}"));

        let mut stdio_id = None;
        let mut http_id = None;
        for title in titles {
            let seed = json!({
                "grounding": [{"kind": "bet"}],
                "title": title,
                "rationale": "seed for hivemind_compact_view parity test",
                "topic_keys": ["parity"],
                "options": [{"label": "only"}],
            });
            let stdio_capture = stdio_call(&stdio_dir, "capture_decision", seed.clone());
            let http_capture = http_call(&http_dir, "capture_decision", seed).await;
            stdio_id = stdio_capture["result"]["structuredContent"]["decision_id"]
                .as_str()
                .map(str::to_owned);
            http_id = http_capture["result"]["structuredContent"]["decision_id"]
                .as_str()
                .map(str::to_owned);
        }
        let stdio_id = stdio_id.expect("stdio decision_id"); // ubs:ignore: test-only; panicking is correct in tests
        let http_id = http_id.expect("http decision_id"); // ubs:ignore: test-only; panicking is correct in tests

        let stdio = stdio_call(&stdio_dir, tool, arguments_from_id(&stdio_id));
        let http = http_call(&http_dir, tool, arguments_from_id(&http_id)).await;
        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
        (
            stdio["result"].clone(), // ubs:ignore: test-only; index guaranteed by test setup
            http["result"].clone(),  // ubs:ignore: test-only; index guaranteed by test setup
        )
    }

    #[tokio::test]
    async fn compact_view_resolves_by_id() {
        let (stdio, http) = run_seeded_with_ids(
            &["Adopt async billing queue"],
            "hivemind_compact_view",
            "compact-view-by-id",
            |id| json!({ "decision_id": id }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(
                data["decision"]["title"], "Adopt async billing queue",
                "{name}: decision title"
            ); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn compact_view_resolves_unique_description() {
        let (stdio, http) = run_seeded(
            "hivemind_compact_view",
            "compact-view-resolved",
            &["Adopt async billing queue"],
            json!({ "description": "adopt async billing queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(result["isError"], false, "{name}: expected success"); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(
                data["decision"]["title"], "Adopt async billing queue",
                "{name}: decision title"
            ); // ubs:ignore: test-only assertion
        }
    }

    #[tokio::test]
    async fn compact_view_ambiguous_description_returns_candidates_not_error() {
        let (stdio, http) = run_seeded(
            "hivemind_compact_view",
            "compact-view-ambiguous",
            &[
                "Adopt async queue for billing",
                "Adopt async queue for notifications",
            ],
            json!({ "description": "adopt async queue" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(
                structured["data"]["outcome"], "ambiguous",
                "{name}: outcome"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
        }
    }

    #[tokio::test]
    async fn compact_view_not_found_description_returns_envelope_not_error() {
        let (stdio, http) = run_seeded(
            "hivemind_compact_view",
            "compact-view-not-found",
            &["Adopt async billing queue"],
            json!({ "description": "totally unrelated widget factory zzz" }),
        )
        .await;
        for (name, result) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Projects on captures (hivemind-s15q.4): the project arrives as an argument on
    // both transports; neither infers it.
    // -----------------------------------------------------------------------

    /// A fresh ledger dir per transport with `projects` already registered. There is no MCP
    /// tool for registering a project; the CLI writes the same sqlite file both transports
    /// read, so seeding through `Commands` is the same starting state.
    fn project_dirs(label: &str, projects: &[&str]) -> (std::path::PathBuf, std::path::PathBuf) {
        let stdio_dir = unique_dir(&format!("parity-stdio-{label}"));
        let http_dir = unique_dir(&format!("parity-http-{label}"));
        for dir in [&stdio_dir, &http_dir] {
            std::fs::create_dir_all(dir).expect("create ledger dir"); // ubs:ignore: test-only; panicking is correct in tests
            let ledger = SqliteEventLedger::open(dir).expect("ledger opens"); // ubs:ignore: test-only; panicking is correct in tests
            let commands = Commands::new(&ledger);
            for handle in projects {
                commands
                    .register_project("human:parity", handle, None, None)
                    .expect("register project"); // ubs:ignore: test-only; panicking is correct in tests
            }
        }
        (stdio_dir, http_dir)
    }

    /// `decision.proposed` payloads under `dir`, oldest first.
    fn proposed_payloads(dir: &std::path::Path) -> Vec<Value> {
        let ledger = SqliteEventLedger::open(dir).expect("ledger opens"); // ubs:ignore: test-only; panicking is correct in tests
        ledger
            .read(0, 1000)
            .expect("read ledger") // ubs:ignore: test-only; panicking is correct in tests
            .into_iter()
            .filter(|event| event.event_type == crate::events::EventType::DecisionProposed)
            .map(|event| event.payload)
            .collect()
    }

    fn capture_args(title: &str) -> Value {
        json!({
            "title": title,
            "rationale": "Bounded retries avoid unbounded backlog growth under load",
            "topic_keys": ["billing"],
            "options": [{"label": "queue"}],
            "grounding": [{"kind": "bet"}],
        })
    }

    fn with_args(mut base: Value, extra: Value) -> Value {
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        base
    }

    fn error_text(result: &Value) -> &str {
        result["content"][0]["text"].as_str().unwrap_or_default() // ubs:ignore: test-only; a missing text fails the assertions below
    }

    #[tokio::test]
    async fn capture_decision_records_the_stated_project_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("capture-project-stated", &["billing"]);

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            for (title, extra, source) in [
                (
                    "Adopt async billing queue",
                    json!({ "project": "billing" }),
                    "stated",
                ),
                (
                    "Adopt weekly billing exports",
                    json!({ "project": "billing", "project_source": "folder_marker" }),
                    "folder_marker",
                ),
            ] {
                let args = with_args(capture_args(title), extra);
                let response = if http {
                    http_call(dir, "capture_decision", args).await
                } else {
                    stdio_call(dir, "capture_decision", args)
                };
                let result = &response["result"];
                assert_eq!(
                    result["isError"], false,
                    "{name}: expected success: {result:?}"
                ); // ubs:ignore: test-only assertion
                let reply = &result["structuredContent"];
                assert_eq!(reply["project"], "billing", "{name}: project"); // ubs:ignore: test-only assertion
                assert_eq!(reply["project_source"], source, "{name}: project_source"); // ubs:ignore: test-only assertion
                assert!(
                    reply.get("project_notice").is_none(),
                    "{name}: a determined project has no fallback notice: {reply:?}"
                ); // ubs:ignore: test-only assertion
            }

            let recorded: Vec<(Value, Value)> = proposed_payloads(dir)
                .into_iter()
                .map(|payload| {
                    (
                        payload["project"].clone(),
                        payload["project_source"].clone(),
                    )
                })
                .collect();
            assert_eq!(
                recorded,
                vec![
                    (json!("billing"), json!("stated")),
                    (json!("billing"), json!("folder_marker")),
                ],
                "{name}: the ledger records the stated project and how it was determined"
            ); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn capture_decision_refuses_an_unknown_project_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("capture-project-unknown", &["billing"]);
        let args = with_args(
            capture_args("Adopt async billing queue"),
            json!({ "project": "not-registered" }),
        );

        let stdio = stdio_call(&stdio_dir, "capture_decision", args.clone());
        let http = http_call(&http_dir, "capture_decision", args).await;

        for (name, response, dir) in [("stdio", &stdio, &stdio_dir), ("http", &http, &http_dir)] {
            let result = &response["result"];
            assert_eq!(
                result["isError"], true,
                "{name}: an unknown handle is refused"
            ); // ubs:ignore: test-only assertion
            let text = error_text(result);
            assert!(
                text.contains("project not registered: not-registered")
                    && text.contains("hivemind project register not-registered"),
                "{name}: refusal names the handle and the register command: {text}"
            ); // ubs:ignore: test-only assertion
            assert!(
                proposed_payloads(dir).is_empty(),
                "{name}: a refused capture writes nothing"
            ); // ubs:ignore: test-only assertion
        }
        assert_eq!(
            error_text(&stdio["result"]),
            error_text(&http["result"]),
            "both transports refuse with the same words"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    /// The parts of a project-scoped `get_situational_decisions` answer that must not differ by
    /// transport. Decision ids are generated per ledger, so decisions are compared by title.
    fn scoped_answer_shape(result: &Value) -> Value {
        let data = &result["structuredContent"]["data"];
        let matches: Vec<Value> = data["matches"]
            .as_array()
            .map(|matches| {
                matches
                    .iter()
                    .map(|matched| {
                        json!({
                            "title": matched["decision"]["title"],
                            "project": matched["decision"]["project"],
                            "relation": matched["scope"]["relation"],
                            "label": matched["scope"]["label"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "matches": matches,
            "total_matches": data["total_matches"],
            "scope": data["scope"],
        })
    }

    #[tokio::test]
    async fn get_situational_decisions_is_project_first_across_transports() {
        let (stdio_dir, http_dir) = project_dirs(
            "situational-project-first",
            &["platform", "billing", "auth", "marketing"],
        );
        for dir in [&stdio_dir, &http_dir] {
            let ledger = SqliteEventLedger::open(dir).expect("ledger opens"); // ubs:ignore: test-only; panicking is correct in tests
            let commands = Commands::new(&ledger);
            for (from, to, kind) in [
                (
                    "billing",
                    "platform",
                    crate::events::ProjectLinkKind::PartOf,
                ),
                ("billing", "auth", crate::events::ProjectLinkKind::DependsOn),
            ] {
                commands
                    .link_project("human:parity", from, to, kind)
                    .expect("link projects"); // ubs:ignore: test-only; panicking is correct in tests
            }
        }
        for (title, project) in [
            ("Price per seat", "billing"),
            ("Quote every price in euros", "platform"),
            ("Bill token issuance per call", "auth"),
            ("Launch with a promo price", "marketing"),
        ] {
            let args = with_args(capture_args(title), json!({ "project": project }));
            stdio_call(&stdio_dir, "capture_decision", args.clone());
            http_call(&http_dir, "capture_decision", args).await;
        }

        let arguments = json!({ "paths": ["billing"], "project": "billing" });
        let stdio = stdio_call(&stdio_dir, "get_situational_decisions", arguments.clone());
        let http = http_call(&http_dir, "get_situational_decisions", arguments).await;

        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                response["result"]["isError"], false,
                "{name}: expected success: {response:?}"
            ); // ubs:ignore: test-only assertion
        }
        let expected = json!({
            "matches": [
                { "title": "Price per seat", "project": "billing", "relation": "own", "label": "own project" },
                { "title": "Quote every price in euros", "project": "platform", "relation": "parent", "label": "from platform; billing is part of it" },
                { "title": "Bill token issuance per call", "project": "auth", "relation": "dependency", "label": "from auth; billing depends on it" },
            ],
            "total_matches": 3,
            "scope": {
                "project": "billing",
                "project_label": "billing",
                "followed": [
                    { "project": "billing", "project_label": "billing", "relation": "own" },
                    { "project": "platform", "project_label": "platform", "relation": "parent" },
                    { "project": "auth", "project_label": "auth", "relation": "dependency" },
                ],
                "part_of_levels_not_followed": 0,
                "linked_projects_not_followed": 0,
                "note": "Looked in billing (own project), platform (billing is part of it), auth (billing depends on it); nothing further is linked.",
            },
        });
        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            assert_eq!(
                scoped_answer_shape(&response["result"]),
                expected,
                "{name}: project-first answer"
            ); // ubs:ignore: test-only assertion
        }

        // A wrong project address is refused on both transports, in the same words.
        let unknown = json!({ "paths": ["billing"], "project": "billng" });
        let stdio = stdio_call(&stdio_dir, "get_situational_decisions", unknown.clone());
        let http = http_call(&http_dir, "get_situational_decisions", unknown).await;
        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            let result = &response["result"];
            assert_eq!(result["isError"], true, "{name}: unknown project refused"); // ubs:ignore: test-only assertion
            assert!(
                error_text(result).contains("project not registered: billng"),
                "{name}: refusal names the handle: {}",
                error_text(result)
            ); // ubs:ignore: test-only assertion
        }
        assert_eq!(
            error_text(&stdio["result"]),
            error_text(&http["result"]),
            "both transports refuse with the same words"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    fn rests_on_kinds_and_labels(result: &Value) -> Vec<(String, String)> {
        result["structuredContent"]["rests_on"]
            .as_array()
            .expect("rests_on array") // ubs:ignore: test-only; panicking is correct in tests
            .iter()
            .map(|item| {
                (
                    item["kind"].as_str().unwrap_or_default().to_owned(),
                    item["label"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    async fn seed_premise(stdio_dir: &std::path::Path, http_dir: &std::path::Path, title: &str) {
        let seed = json!({
            "grounding": [{"kind": "bet"}],
            "title": title,
            "rationale": "seed for the grounding parity tests, long enough to read",
            "topic_keys": ["parity"],
            "options": [{"label": "only"}],
        });
        stdio_call(stdio_dir, "capture_decision", seed.clone());
        http_call(http_dir, "capture_decision", seed).await;
    }

    #[tokio::test]
    async fn capture_decision_records_every_grounding_kind_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-grounded");
        let http_dir = unique_dir("parity-http-grounded");
        seed_premise(&stdio_dir, &http_dir, "Keep the ledger append-only").await;

        let args = json!({
            "title": "Adopt the new queue",
            "rationale": "Durability beats latency for billing events",
            "topic_keys": ["parity"],
            "options": [{"label": "adopt"}],
            "expressed_confidence": "high",
            "grounding": [
                {"kind": "decision", "description": "Keep the ledger append-only"},
                {"kind": "evidence", "content": "p95 was 180ms in run 42", "source": "ci run 42"},
                {"kind": "assumption", "statement": "traffic stays under 1k rps"},
                {"kind": "bet", "would_change_if": "they raise prices", "check_by": "2026-12-01"},
            ],
        });
        let stdio = stdio_call(&stdio_dir, "capture_decision", args.clone());
        let http = http_call(&http_dir, "capture_decision", args).await;

        let expected = vec![
            (
                "decision".to_owned(),
                "Keep the ledger append-only".to_owned(),
            ),
            ("evidence".to_owned(), "p95 was 180ms in run 42".to_owned()),
            (
                "assumption".to_owned(),
                "traffic stays under 1k rps".to_owned(),
            ),
            (
                "bet".to_owned(),
                "Judgement call: Adopt the new queue".to_owned(),
            ),
        ];
        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            let result = &response["result"];
            assert_eq!(result["isError"], false, "{name}: {result:?}"); // ubs:ignore: test-only assertion
            assert_eq!(rests_on_kinds_and_labels(result), expected, "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                result["structuredContent"]["premise_stale"],
                json!([]),
                "{name}: premise_stale"
            );
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn capture_decision_without_project_announces_the_personal_fallback_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("capture-project-fallback", &["billing"]);
        let args = capture_args("Adopt async billing queue");

        let stdio = stdio_call(&stdio_dir, "capture_decision", args.clone());
        let http = http_call(&http_dir, "capture_decision", args).await;

        for (name, response, dir) in [("stdio", &stdio, &stdio_dir), ("http", &http, &http_dir)] {
            let result = &response["result"];
            assert_eq!(
                result["isError"], false,
                "{name}: expected success: {result:?}"
            ); // ubs:ignore: test-only assertion
            let reply = &result["structuredContent"];
            assert_eq!(
                reply["project_source"], "personal_fallback",
                "{name}: project_source"
            ); // ubs:ignore: test-only assertion
            assert!(
                reply["project"]
                    .as_str()
                    .is_some_and(|project| project.starts_with("personal:")),
                "{name}: the reply names the derived personal address: {reply:?}"
            ); // ubs:ignore: test-only assertion
            assert!(
                reply["project_notice"]
                    .as_str()
                    .is_some_and(|notice| notice.contains("saved to your personal project")),
                "{name}: the fallback is announced, never silent: {reply:?}"
            ); // ubs:ignore: test-only assertion

            let payloads = proposed_payloads(dir);
            assert_eq!(payloads.len(), 1, "{name}: one proposal"); // ubs:ignore: test-only assertion
            assert_eq!(
                payloads[0]["project_source"], "personal_fallback",
                "{name}: recorded as a fallback"
            ); // ubs:ignore: test-only assertion
            assert!(
                payloads[0].get("project").is_none(),
                "{name}: no handle is recorded for a fallback"
            ); // ubs:ignore: test-only assertion
        }
        assert_eq!(
            stdio["result"]["structuredContent"]["project_notice"],
            http["result"]["structuredContent"]["project_notice"],
            "both transports announce the fallback in the same words"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn capture_decision_ambiguous_premise_writes_nothing_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-grounding-ambiguous");
        let http_dir = unique_dir("parity-http-grounding-ambiguous");
        seed_premise(&stdio_dir, &http_dir, "Adopt the queue").await;
        seed_premise(&stdio_dir, &http_dir, "Adopt the queue").await;
        let stdio_offset_before = ledger_offset(&stdio_dir);
        let http_offset_before = ledger_offset(&http_dir);

        let args = json!({
            "title": "Ship the queue",
            "rationale": "Rationale text long enough for the readable floor",
            "topic_keys": ["parity"],
            "options": [{"label": "ship"}],
            "grounding": [
                {"kind": "assumption", "statement": "must not be stranded"},
                {"kind": "decision", "description": "Adopt the queue"},
            ],
        });
        let stdio = stdio_call(&stdio_dir, "capture_decision", args.clone());
        let http = http_call(&http_dir, "capture_decision", args).await;

        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            let result = &response["result"];
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is data, not an error"
            ); // ubs:ignore: test-only assertion
            let data = &result["structuredContent"]["data"];
            assert_eq!(data["outcome"], "ambiguous", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(data["field"], "grounding[1]", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(
                data["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}"
            ); // ubs:ignore: test-only assertion
        }
        assert_eq!(
            ledger_offset(&stdio_dir),
            stdio_offset_before,
            "stdio wrote"
        ); // ubs:ignore: test-only assertion
        assert_eq!(ledger_offset(&http_dir), http_offset_before, "http wrote"); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn capture_decision_rejects_inconsistent_project_arguments_across_transports() {
        let cases = [
            (
                "source-without-project",
                json!({ "project_source": "rig" }),
                "`project_source` requires `project`",
            ),
            (
                "unknown-source",
                json!({ "project": "billing", "project_source": "guessed" }),
                "`project_source` must be one of stated, folder_marker, rig, current_project, job: guessed",
            ),
            (
                "fallback-source-with-a-handle",
                json!({ "project": "billing", "project_source": "personal_fallback" }),
                "cannot accompany a project handle",
            ),
            (
                "moved-source-with-a-handle",
                json!({ "project": "billing", "project_source": "moved" }),
                "cannot accompany a project handle",
            ),
            (
                "stated-personal-address",
                json!({ "project": "personal:agent:claude" }),
                "reserved \"personal:\" prefix",
            ),
        ];

        for (label, extra, expected) in cases {
            let (stdio_dir, http_dir) =
                project_dirs(&format!("capture-project-{label}"), &["billing"]);
            let args = with_args(capture_args("Adopt async billing queue"), extra);
            let stdio = stdio_call(&stdio_dir, "capture_decision", args.clone());
            let http = http_call(&http_dir, "capture_decision", args).await;
            for (name, response, dir) in [("stdio", &stdio, &stdio_dir), ("http", &http, &http_dir)]
            {
                let result = &response["result"];
                assert_eq!(result["isError"], true, "{label}/{name}: refused"); // ubs:ignore: test-only assertion
                assert!(
                    error_text(result).contains(expected),
                    "{label}/{name}: expected {expected:?} in {:?}",
                    error_text(result)
                ); // ubs:ignore: test-only assertion
                assert!(
                    proposed_payloads(dir).is_empty(),
                    "{label}/{name}: a refused capture writes nothing"
                ); // ubs:ignore: test-only assertion
            }
            assert_eq!(
                error_text(&stdio["result"]),
                error_text(&http["result"]),
                "{label}: both transports refuse with the same words"
            ); // ubs:ignore: test-only assertion
            let _ = std::fs::remove_dir_all(&stdio_dir);
            let _ = std::fs::remove_dir_all(&http_dir);
        }
    }

    #[tokio::test]
    async fn supersede_decision_files_under_a_stated_project_or_inherits_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("supersede-project", &["billing", "payments"]);

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, args: Value| async move {
                if http {
                    http_call(dir, tool, args).await
                } else {
                    stdio_call(dir, tool, args)
                }
            };
            let old = call(
                "capture_decision",
                with_args(
                    capture_args("Use shared admin token"),
                    json!({ "project": "billing", "project_source": "rig" }),
                ),
            )
            .await;
            let old_id = old["result"]["structuredContent"]["decision_id"]
                .as_str()
                .expect("decision id") // ubs:ignore: test-only; panicking is correct in tests
                .to_owned();

            // Not stated: inherits the old decision's project and how it was determined.
            let inherited = call(
                "supersede_decision",
                json!({
                    "old_decision_id": old_id,
                    "title": "Use scoped service tokens",
                    "rationale": "Scoped tokens preserve audit boundaries",
                    "options": [{"label": "scoped-service-tokens"}],
                    "chosen_option_label": "scoped-service-tokens",
                    "grounding": [{"kind": "bet"}],
                }),
            )
            .await;
            let reply = &inherited["result"]["structuredContent"];
            assert_eq!(reply["project"], "billing", "{name}: inherited project"); // ubs:ignore: test-only assertion
            assert_eq!(reply["project_source"], "rig", "{name}: inherited source"); // ubs:ignore: test-only assertion
            assert!(
                reply.get("project_notice").is_none(),
                "{name}: an inherited project is not a fallback: {reply:?}"
            ); // ubs:ignore: test-only assertion
            let new_id = reply["new_decision_id"]
                .as_str()
                .expect("new decision id") // ubs:ignore: test-only; panicking is correct in tests
                .to_owned();

            // Stated: overrides the inherited project.
            let stated = call(
                "supersede_decision",
                json!({
                    "old_decision_id": new_id,
                    "title": "Move token handling to payments",
                    "rationale": "Payments owns credential rotation for both products",
                    "options": [{"label": "payments-owned"}],
                    "chosen_option_label": "payments-owned",
                    "grounding": [{"kind": "bet"}],
                    "project": "payments",
                }),
            )
            .await;
            let reply = &stated["result"]["structuredContent"];
            assert_eq!(reply["project"], "payments", "{name}: stated project"); // ubs:ignore: test-only assertion
            assert_eq!(reply["project_source"], "stated", "{name}: stated source"); // ubs:ignore: test-only assertion

            // Unknown: refused, nothing written.
            let before = proposed_payloads(dir).len();
            let unknown = call(
                "supersede_decision",
                json!({
                    "old_decision_id": old_id,
                    "title": "Use hardware tokens instead",
                    "rationale": "Hardware tokens remove the shared secret entirely",
                    "options": [{"label": "hardware-tokens"}],
                    "chosen_option_label": "hardware-tokens",
                    "grounding": [{"kind": "bet"}],
                    "project": "not-registered",
                }),
            )
            .await;
            assert_eq!(
                unknown["result"]["isError"], true,
                "{name}: unknown handle refused"
            ); // ubs:ignore: test-only assertion
            assert!(
                error_text(&unknown["result"]).contains("project not registered: not-registered"),
                "{name}: {:?}",
                error_text(&unknown["result"])
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                proposed_payloads(dir).len(),
                before,
                "{name}: a refused supersede writes nothing"
            ); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    // -----------------------------------------------------------------------
    // move_decision (hivemind-s15q.11): same resolve-by-description write gate as
    // disagree/supersede, same envelope on both transports.
    // -----------------------------------------------------------------------

    /// `decision.moved` payloads under `dir`, oldest first.
    fn moved_payloads(dir: &std::path::Path) -> Vec<Value> {
        let ledger = SqliteEventLedger::open(dir).expect("ledger opens"); // ubs:ignore: test-only; panicking is correct in tests
        ledger
            .read(0, 1000)
            .expect("read ledger") // ubs:ignore: test-only; panicking is correct in tests
            .into_iter()
            .filter(|event| event.event_type == crate::events::EventType::DecisionMoved)
            .map(|event| event.payload)
            .collect()
    }

    fn captured_id(reply: &Value) -> String {
        reply["result"]["structuredContent"]["decision_id"]
            .as_str()
            .expect("decision id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned()
    }

    #[tokio::test]
    async fn move_decision_by_id_records_from_and_to_and_reverses_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("move-by-id", &["billing", "pricing"]);

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, args: Value| async move {
                if http {
                    http_call(dir, tool, args).await
                } else {
                    stdio_call(dir, tool, args)
                }
            };
            let decision_id = captured_id(
                &call(
                    "capture_decision",
                    with_args(
                        capture_args("Per-seat pricing"),
                        json!({ "project": "billing" }),
                    ),
                )
                .await,
            );

            let moved = call(
                "move_decision",
                json!({
                    "decision_id": decision_id,
                    "to": "pricing",
                    "reason": "per-seat pricing decisions live under Pricing",
                }),
            )
            .await;
            assert_eq!(moved["result"]["isError"], false, "{name}: {moved:?}"); // ubs:ignore: test-only assertion
            let reply = &moved["result"]["structuredContent"];
            assert_eq!(reply["decision_id"], json!(decision_id), "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(reply["from"], "billing", "{name}: from is read, not passed"); // ubs:ignore: test-only assertion
            assert_eq!(reply["to"], "pricing", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(
                reply["reason"], "per-seat pricing decisions live under Pricing",
                "{name}"
            ); // ubs:ignore: test-only assertion
            assert!(reply["event_id"].is_u64(), "{name}: {reply:?}"); // ubs:ignore: test-only assertion

            // Reversal is another recorded move, and `from` follows the first one.
            let back = call(
                "move_decision",
                json!({ "decision_id": decision_id, "to": "billing" }),
            )
            .await;
            let reply = &back["result"]["structuredContent"];
            assert_eq!(reply["from"], "pricing", "{name}: reversal from"); // ubs:ignore: test-only assertion
            assert_eq!(reply["to"], "billing", "{name}: reversal to"); // ubs:ignore: test-only assertion
            assert!(reply.get("reason").is_none(), "{name}: no reason given"); // ubs:ignore: test-only assertion

            let moves = moved_payloads(dir);
            assert_eq!(moves.len(), 2, "{name}: both moves recorded"); // ubs:ignore: test-only assertion
            assert_eq!(moves[0]["from"], "billing", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(moves[0]["to"], "pricing", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(moves[1]["from"], "pricing", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(moves[1]["to"], "billing", "{name}"); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn move_decision_resolves_a_unique_description_and_writes_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("move-unique-desc", &["billing", "pricing"]);

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, args: Value| async move {
                if http {
                    http_call(dir, tool, args).await
                } else {
                    stdio_call(dir, tool, args)
                }
            };
            let decision_id = captured_id(
                &call(
                    "capture_decision",
                    with_args(
                        capture_args("Adopt async billing queue"),
                        json!({ "project": "billing" }),
                    ),
                )
                .await,
            );

            let moved = call(
                "move_decision",
                json!({ "description": "adopt async billing queue", "to": "pricing" }),
            )
            .await;
            assert_eq!(moved["result"]["isError"], false, "{name}: {moved:?}"); // ubs:ignore: test-only assertion
            let reply = &moved["result"]["structuredContent"];
            assert_eq!(
                reply["decision_id"],
                json!(decision_id),
                "{name}: the description resolved to the captured decision"
            ); // ubs:ignore: test-only assertion
            assert_eq!(reply["from"], "billing", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(reply["to"], "pricing", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(moved_payloads(dir).len(), 1, "{name}: one move recorded");
            // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn move_decision_ambiguous_description_returns_candidates_and_writes_nothing_across_transports(
    ) {
        let (stdio_dir, http_dir) = project_dirs("move-ambiguous", &["billing", "pricing"]);

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, args: Value| async move {
                if http {
                    http_call(dir, tool, args).await
                } else {
                    stdio_call(dir, tool, args)
                }
            };
            for title in [
                "Adopt async queue for billing",
                "Adopt async queue for notifications",
            ] {
                call(
                    "capture_decision",
                    with_args(capture_args(title), json!({ "project": "billing" })),
                )
                .await;
            }
            let offset_before = ledger_offset(dir);

            let reply = call(
                "move_decision",
                json!({ "description": "adopt async queue", "to": "pricing" }),
            )
            .await;
            let result = &reply["result"];
            assert_eq!(
                result["isError"], false,
                "{name}: ambiguous is not an error"
            ); // ubs:ignore: test-only assertion
            let structured = &result["structuredContent"];
            assert_eq!(structured["data"]["outcome"], "ambiguous", "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(
                // ubs:ignore: test-only assertion
                structured["data"]["candidates"].as_array().map(Vec::len),
                Some(2),
                "{name}: candidate count"
            );
            assert_eq!(
                ledger_offset(dir),
                offset_before,
                "{name}: an ambiguous move writes nothing"
            ); // ubs:ignore: test-only assertion
            assert!(moved_payloads(dir).is_empty(), "{name}"); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn move_decision_not_found_description_is_a_success_envelope_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("move-not-found", &["billing", "pricing"]);

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, args: Value| async move {
                if http {
                    http_call(dir, tool, args).await
                } else {
                    stdio_call(dir, tool, args)
                }
            };
            call(
                "capture_decision",
                with_args(
                    capture_args("Adopt async billing queue"),
                    json!({ "project": "billing" }),
                ),
            )
            .await;
            let offset_before = ledger_offset(dir);

            let reply = call(
                "move_decision",
                json!({ "description": "totally unrelated widget factory zzz", "to": "pricing" }),
            )
            .await;
            let result = &reply["result"];
            assert_eq!(
                result["isError"], false,
                "{name}: not-found is not an error: {result:?}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["data"]["outcome"],
                "not_found", // ubs:ignore: test-only assertion
                "{name}: outcome"
            );
            assert_eq!(
                ledger_offset(dir),
                offset_before,
                "{name}: a not-found move writes nothing"
            ); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn move_decision_refusals_write_nothing_and_read_alike_across_transports() {
        let (stdio_dir, http_dir) = project_dirs("move-refused", &["billing", "pricing"]);
        let mut messages: Vec<(&str, Vec<String>)> = Vec::new();

        for (name, dir, http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, args: Value| async move {
                if http {
                    http_call(dir, tool, args).await
                } else {
                    stdio_call(dir, tool, args)
                }
            };
            let decision_id = captured_id(
                &call(
                    "capture_decision",
                    with_args(
                        capture_args("Per-seat pricing"),
                        json!({ "project": "billing" }),
                    ),
                )
                .await,
            );

            let mut seen = Vec::new();
            for (label, args, expected) in [
                (
                    "unknown project",
                    json!({ "decision_id": decision_id, "to": "not-registered" }),
                    "project not registered: not-registered",
                ),
                (
                    "already there",
                    json!({ "decision_id": decision_id, "to": "billing" }),
                    "already in project billing",
                ),
                (
                    "no destination",
                    json!({ "decision_id": decision_id }),
                    "missing `to`",
                ),
                (
                    "no target",
                    json!({ "to": "pricing" }),
                    "one of `decision_id` or `description` is required",
                ),
            ] {
                let reply = call("move_decision", args).await;
                let result = &reply["result"];
                assert_eq!(result["isError"], true, "{name}/{label}: {result:?}"); // ubs:ignore: test-only assertion
                assert!(
                    error_text(result).contains(expected),
                    "{name}/{label}: {:?}",
                    error_text(result)
                ); // ubs:ignore: test-only assertion
                   // Decision ids are generated per ledger; the words around them must match.
                seen.push(error_text(result).replace(&decision_id, "<decision>"));
            }
            assert!(
                moved_payloads(dir).is_empty(),
                "{name}: a refused move writes nothing"
            ); // ubs:ignore: test-only assertion
            messages.push((name, seen));
        }

        assert_eq!(
            messages[0].1, messages[1].1,
            "both transports refuse with the same words"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn capture_decision_unmatched_premise_writes_nothing_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-grounding-not-found");
        let http_dir = unique_dir("parity-http-grounding-not-found");
        seed_premise(&stdio_dir, &http_dir, "Keep the ledger append-only").await;
        let stdio_offset_before = ledger_offset(&stdio_dir);
        let http_offset_before = ledger_offset(&http_dir);

        for (label, item, key, text) in [
            (
                "description",
                json!({"kind": "decision", "description": "quantum flux capacitor"}),
                "description",
                "quantum flux capacitor",
            ),
            (
                "id",
                json!({"kind": "decision", "decision_id": "decision-does-not-exist"}),
                "decision_id",
                "decision-does-not-exist",
            ),
        ] {
            let args = json!({
                "title": "Adopt the new queue",
                "rationale": "Rationale text long enough for the readable floor",
                "topic_keys": ["parity"],
                "options": [{"label": "adopt"}],
                "grounding": [
                    {"kind": "evidence", "content": "must not be stranded"},
                    item,
                ],
            });
            let stdio = stdio_call(&stdio_dir, "capture_decision", args.clone());
            let http = http_call(&http_dir, "capture_decision", args).await;
            for (name, response) in [("stdio", &stdio), ("http", &http)] {
                let result = &response["result"];
                assert_eq!(result["isError"], false, "{label}/{name}: a miss is data"); // ubs:ignore: test-only assertion
                let data = &result["structuredContent"]["data"];
                assert_eq!(data["outcome"], "not_found", "{label}/{name}"); // ubs:ignore: test-only assertion
                assert_eq!(data["field"], "grounding[1]", "{label}/{name}"); // ubs:ignore: test-only assertion
                assert_eq!(data[key], text, "{label}/{name}"); // ubs:ignore: test-only assertion
            }
        }
        assert_eq!(
            ledger_offset(&stdio_dir),
            stdio_offset_before,
            "stdio wrote"
        ); // ubs:ignore: test-only assertion
        assert_eq!(ledger_offset(&http_dir), http_offset_before, "http wrote"); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn capture_decision_deprecated_id_aliases_map_into_grounding_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-grounding-aliases");
        let http_dir = unique_dir("parity-http-grounding-aliases");

        for (name, dir, is_http) in [("stdio", &stdio_dir, false), ("http", &http_dir, true)] {
            let call = |tool: &'static str, arguments: Value| {
                let dir = dir.clone();
                async move {
                    if is_http {
                        http_call(&dir, tool, arguments).await
                    } else {
                        stdio_call(&dir, tool, arguments)
                    }
                }
            };
            let evidence = call(
                "capture_evidence",
                json!({"content": "an existing observation"}),
            )
            .await;
            let evidence_id = evidence["result"]["structuredContent"]["evidence_id"]
                .as_str()
                .expect("evidence_id") // ubs:ignore: test-only; panicking is correct in tests
                .to_owned();
            let hypothesis = call(
                "capture_hypothesis",
                json!({"statement": "an existing assumption"}),
            )
            .await;
            let hypothesis_id = hypothesis["result"]["structuredContent"]["hypothesis_id"]
                .as_str()
                .expect("hypothesis_id") // ubs:ignore: test-only; panicking is correct in tests
                .to_owned();

            // No `grounding` at all: the aliases alone satisfy the requirement.
            let captured = call(
                "capture_decision",
                json!({
                    "title": "Adopt the new queue",
                    "rationale": "Rationale text long enough for the readable floor",
                    "topic_keys": ["parity"],
                    "options": [{"label": "adopt"}],
                    "evidence_ids": [evidence_id],
                    "hypothesis_ids": [hypothesis_id],
                }),
            )
            .await;
            let result = &captured["result"];
            assert_eq!(result["isError"], false, "{name}: {result:?}"); // ubs:ignore: test-only assertion
            let kinds: Vec<String> = rests_on_kinds_and_labels(result)
                .into_iter()
                .map(|(kind, _)| kind)
                .collect();
            assert_eq!(kinds, ["evidence", "assumption"], "{name}"); // ubs:ignore: test-only assertion
            let rests_on = result["structuredContent"]["rests_on"]
                .as_array()
                .expect("rests_on"); // ubs:ignore: test-only; panicking is correct in tests
            assert_eq!(rests_on[0]["id"], json!(evidence_id), "{name}"); // ubs:ignore: test-only assertion
            assert_eq!(rests_on[1]["id"], json!(hypothesis_id), "{name}"); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    #[tokio::test]
    async fn supersede_decision_records_grounding_and_reports_stale_premises_across_transports() {
        let stdio_dir = unique_dir("parity-stdio-supersede-grounded");
        let http_dir = unique_dir("parity-http-supersede-grounded");
        seed_premise(&stdio_dir, &http_dir, "Adopt the shared admin token").await;
        seed_premise(&stdio_dir, &http_dir, "Keep audit boundaries").await;

        // Supersede the token decision by description, resting on the boundaries decision.
        let args = json!({
            "description": "Adopt the shared admin token",
            "title": "Use scoped service tokens",
            "rationale": "Scoped tokens preserve audit boundaries for every caller",
            "options": [{"label": "scoped-tokens"}],
            "grounding": [
                {"kind": "decision", "description": "Keep audit boundaries"},
                {"kind": "evidence", "content": "the shared token leaked twice", "source": "incident 7"},
            ],
        });
        let stdio = stdio_call(&stdio_dir, "supersede_decision", args.clone());
        let http = http_call(&http_dir, "supersede_decision", args).await;

        let expected = vec![
            ("decision".to_owned(), "Keep audit boundaries".to_owned()),
            (
                "evidence".to_owned(),
                "the shared token leaked twice".to_owned(),
            ),
        ];
        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            let result = &response["result"];
            assert_eq!(result["isError"], false, "{name}: {result:?}"); // ubs:ignore: test-only assertion
            assert_eq!(
                result["structuredContent"]["old_decision_status"], "superseded",
                "{name}"
            ); // ubs:ignore: test-only assertion
            assert_eq!(rests_on_kinds_and_labels(result), expected, "{name}"); // ubs:ignore: test-only assertion
        }

        // A supersede with nothing named is refused as an invalid argument, on both transports.
        let refused_args = json!({
            "description": "Keep audit boundaries",
            "title": "Use scoped service tokens again",
            "rationale": "Scoped tokens preserve audit boundaries for every caller",
        });
        let stdio = stdio_call(&stdio_dir, "supersede_decision", refused_args.clone());
        let http = http_call(&http_dir, "supersede_decision", refused_args).await;
        for (name, response) in [("stdio", &stdio), ("http", &http)] {
            let result = &response["result"];
            assert_eq!(result["isError"], true, "{name}: {result:?}"); // ubs:ignore: test-only assertion
        }

        let _ = std::fs::remove_dir_all(&stdio_dir);
        let _ = std::fs::remove_dir_all(&http_dir);
    }

    // -----------------------------------------------------------------------
    // Working the project out from where the stdio server runs
    // (hivemind-s15q.12): opt-in via `--project-from-context`, stdio only.
    // -----------------------------------------------------------------------

    /// A folder tree for the context tests: `repo` carries a `.hivemind-project` naming
    /// `billing`, `elsewhere` has no marker anywhere above it.
    fn context_tree(label: &str) -> std::path::PathBuf {
        let root = unique_dir(&format!("context-{label}"));
        std::fs::create_dir_all(root.join("repo/src")).expect("create repo dir"); // ubs:ignore: test-only; panicking is correct in tests
        std::fs::create_dir_all(root.join("elsewhere")).expect("create elsewhere dir"); // ubs:ignore: test-only; panicking is correct in tests
        std::fs::write(root.join("repo/.hivemind-project"), "billing\n").expect("write marker"); // ubs:ignore: test-only; panicking is correct in tests
        root
    }

    /// A ledger dir with `billing` and `payments` registered and `billing` anchored to the
    /// rig `city-rig`.
    fn context_ledger(label: &str) -> std::path::PathBuf {
        let (dir, unused) = project_dirs(label, &["billing", "payments"]);
        let _ = std::fs::remove_dir_all(&unused);
        let ledger = SqliteEventLedger::open(&dir).expect("ledger opens"); // ubs:ignore: test-only; panicking is correct in tests
        Commands::new(&ledger)
            .anchor_project(
                "human:parity",
                "billing",
                crate::events::ProjectAnchorKind::Rig,
                "city-rig",
            )
            .expect("anchor rig"); // ubs:ignore: test-only; panicking is correct in tests
        dir
    }

    fn stdio_call_from(
        dir: &std::path::Path,
        folder: std::path::PathBuf,
        rig: Option<&str>,
        tool: &str,
        arguments: Value,
    ) -> Value {
        let config = McpConfig::new(dir)
            .with_session_id("context-stdio")
            .with_project_context(ProjectContextEnv::new(Some(folder), rig, "human:parity"));
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

    #[test]
    fn stdio_capture_from_context_follows_the_folder_then_the_rig() {
        let dir = context_ledger("capture-ladder");
        let tree = context_tree("capture-ladder");

        // A folder marker names the project, and the reply says how.
        let marker = stdio_call_from(
            &dir,
            tree.join("repo/src"),
            Some("city-rig"),
            "capture_decision",
            capture_args("Adopt async billing queue"),
        );
        let reply = &marker["result"]["structuredContent"];
        assert_eq!(marker["result"]["isError"], false, "{marker:?}"); // ubs:ignore: test-only assertion
        assert_eq!(reply["project"], "billing"); // ubs:ignore: test-only assertion
        assert_eq!(reply["project_source"], "folder_marker"); // ubs:ignore: test-only assertion
        assert!(
            reply.get("project_notice").is_none() && reply.get("project_reminder").is_none(),
            "an attached folder has nothing to be reminded about: {reply:?}"
        ); // ubs:ignore: test-only assertion

        // No marker: the rig's anchored project.
        let rig = stdio_call_from(
            &dir,
            tree.join("elsewhere"),
            Some("city-rig"),
            "capture_decision",
            capture_args("Adopt weekly billing exports"),
        );
        let reply = &rig["result"]["structuredContent"];
        assert_eq!(reply["project"], "billing"); // ubs:ignore: test-only assertion
        assert_eq!(reply["project_source"], "rig"); // ubs:ignore: test-only assertion

        // A stated project wins over the folder and the rig.
        let stated = stdio_call_from(
            &dir,
            tree.join("repo/src"),
            Some("city-rig"),
            "capture_decision",
            with_args(
                capture_args("Adopt nightly payments reports"),
                json!({ "project": "payments" }),
            ),
        );
        let reply = &stated["result"]["structuredContent"];
        assert_eq!(reply["project"], "payments"); // ubs:ignore: test-only assertion
        assert_eq!(reply["project_source"], "stated"); // ubs:ignore: test-only assertion

        let recorded: Vec<(Value, Value)> = proposed_payloads(&dir)
            .into_iter()
            .map(|payload| {
                (
                    payload["project"].clone(),
                    payload["project_source"].clone(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            vec![
                (json!("billing"), json!("folder_marker")),
                (json!("billing"), json!("rig")),
                (json!("payments"), json!("stated")),
            ],
            "the ledger records each project and how it was determined"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&tree);
    }

    #[test]
    fn stdio_capture_from_context_with_nothing_to_go_on_falls_back_with_the_reminder() {
        let dir = context_ledger("capture-fallback");
        let tree = context_tree("capture-fallback");

        let response = stdio_call_from(
            &dir,
            tree.join("elsewhere"),
            None,
            "capture_decision",
            capture_args("Adopt async billing queue"),
        );
        let reply = &response["result"]["structuredContent"];
        assert_eq!(reply["project_source"], "personal_fallback"); // ubs:ignore: test-only assertion
        assert!(
            reply["project_notice"]
                .as_str()
                .is_some_and(|notice| notice.contains("saved to your personal project")),
            "{reply:?}"
        ); // ubs:ignore: test-only assertion
        assert_eq!(
            reply["project_reminder"],
            "this folder is not attached to a project yet; run hivemind project anchor ... to attach it"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&tree);
    }

    #[test]
    fn stdio_capture_without_the_flag_never_reads_the_folder() {
        let dir = context_ledger("capture-opt-in");

        // A plain server (no `with_project_context`) files an unnamed capture under the
        // personal project, whatever folder it happens to run in.
        let response = stdio_call(
            &dir,
            "capture_decision",
            capture_args("Adopt async billing"),
        );
        let reply = &response["result"]["structuredContent"];
        assert_eq!(reply["project_source"], "personal_fallback"); // ubs:ignore: test-only assertion
        assert!(
            reply.get("project_reminder").is_none(),
            "no reminder without the flag: {reply:?}"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stdio_capture_from_context_refuses_a_marker_naming_an_unregistered_project() {
        let dir = context_ledger("capture-marker-unregistered");
        let tree = context_tree("capture-marker-unregistered");
        std::fs::write(tree.join("elsewhere/.hivemind-project"), "billng\n").expect("write marker"); // ubs:ignore: test-only; panicking is correct in tests

        let response = stdio_call_from(
            &dir,
            tree.join("elsewhere"),
            None,
            "capture_decision",
            capture_args("Adopt async billing queue"),
        );
        assert_eq!(response["result"]["isError"], true, "{response:?}"); // ubs:ignore: test-only assertion
        let text = error_text(&response["result"]);
        assert!(
            text.contains("project not registered: billng")
                && text.contains("hivemind project register billng"),
            "{text}"
        ); // ubs:ignore: test-only assertion
        assert!(
            proposed_payloads(&dir).is_empty(),
            "a refused capture writes nothing"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&tree);
    }

    #[test]
    fn stdio_supersede_from_context_follows_the_folder_or_inherits() {
        let dir = context_ledger("supersede-context");
        let tree = context_tree("supersede-context");

        let old = stdio_call(
            &dir,
            "capture_decision",
            with_args(
                capture_args("Use shared admin token"),
                json!({ "project": "payments" }),
            ),
        );
        let old_id = old["result"]["structuredContent"]["decision_id"]
            .as_str()
            .expect("decision id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned();
        let supersede_args = |old_decision_id: &str, title: &str| {
            json!({
                "old_decision_id": old_decision_id,
                "title": title,
                "rationale": "Scoped tokens preserve audit boundaries",
                "options": [{"label": "scoped-service-tokens"}],
                "chosen_option_label": "scoped-service-tokens",
                "grounding": [{"kind": "bet"}],
            })
        };

        // The folder's marker names the superseding decision's project.
        let from_folder = stdio_call_from(
            &dir,
            tree.join("repo/src"),
            None,
            "supersede_decision",
            supersede_args(&old_id, "Use scoped service tokens"),
        );
        let reply = &from_folder["result"]["structuredContent"];
        assert_eq!(reply["project"], "billing", "{reply:?}"); // ubs:ignore: test-only assertion
        assert_eq!(reply["project_source"], "folder_marker"); // ubs:ignore: test-only assertion
        let new_id = reply["new_decision_id"]
            .as_str()
            .expect("new decision id") // ubs:ignore: test-only; panicking is correct in tests
            .to_owned();

        // A folder nothing reaches leaves it unstated: the old project is inherited.
        let inherited = stdio_call_from(
            &dir,
            tree.join("elsewhere"),
            None,
            "supersede_decision",
            supersede_args(&new_id, "Rotate scoped service tokens monthly"),
        );
        let reply = &inherited["result"]["structuredContent"];
        assert_eq!(reply["project"], "billing", "{reply:?}"); // ubs:ignore: test-only assertion
        assert_eq!(reply["project_source"], "folder_marker"); // ubs:ignore: test-only assertion
        assert!(
            reply.get("project_notice").is_none() && reply.get("project_reminder").is_none(),
            "an inherited project needs neither: {reply:?}"
        ); // ubs:ignore: test-only assertion

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&tree);
    }
}
