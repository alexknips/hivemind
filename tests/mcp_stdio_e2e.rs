//! End-to-end MCP stdio test: spawns the real `hivemind mcp` binary, talks to
//! it over its stdin/stdout pipes the same way an MCP client would, and
//! confirms a captured decision lands in the ledger and is retrievable on a
//! follow-up call.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn unique_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("hivemind-mcp-e2e-{label}-{}", uuid::Uuid::new_v4()));
    dir
}

#[test]
fn mcp_stdio_server_handles_initialize_list_and_capture() {
    let hivemind_dir = unique_dir("roundtrip");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("mcp")
        .arg("--session-id")
        .arg("e2e-session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hivemind mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let mut send = |request: Value| {
        writeln!(stdin, "{}", request).expect("write request");
        stdin.flush().expect("flush");
    };
    let mut read = || -> Value {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim()).expect("response is json")
    };

    // 1. initialize
    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}));
    let init = read();
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");

    // 2. tools/list
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let listed = read();
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools is array");
    let names: BTreeSet<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    for expected in [
        "capture_decision",
        "capture_evidence",
        "capture_hypothesis",
        "get_decision",
        "get_relevant_decisions",
        "get_supersession_chain",
        "recent_decisions",
        "dump_graph",
    ] {
        assert!(
            names.contains(expected),
            "missing tool `{expected}` in {names:?}"
        );
    }

    // 3. capture_decision
    send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "capture_decision",
            "arguments": {
                "actor_id": "agent:e2e:1",
                "title": "Adopt MCP transport",
                "rationale": "Reuse one server across MCP-aware agents",
                "topic_keys": ["distribution"],
                "options": [{"label": "ship"}, {"label": "defer"}],
                "chosen_option_label": "ship"
            }
        }
    }));
    let captured = read();
    assert_eq!(captured["result"]["isError"], Value::Bool(false));
    let decision_id = captured["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id present")
        .to_owned();
    assert!(decision_id.starts_with("decision-"));

    // 4. get_decision round-trip
    send(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "get_decision",
            "arguments": {"decision_id": decision_id }
        }
    }));
    let fetched = read();
    let data = &fetched["result"]["structuredContent"]["data"];
    assert_eq!(data["title"].as_str(), Some("Adopt MCP transport"));
    assert_eq!(data["topic_keys"][0].as_str(), Some("distribution"));

    // 5. close stdin so the server loop exits cleanly
    drop(stdin);
    let status = child.wait().expect("wait child");
    assert!(status.success(), "server exited non-zero: {status:?}");

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn mcp_capture_evidence_persists_across_invocations() {
    let hivemind_dir = unique_dir("evidence");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hivemind mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "capture_evidence",
                "arguments": {
                    "actor_id": "agent:e2e:1",
                    "content": "Observed throughput: 1.2k req/s"
                }
            }
        })
    )
    .expect("write");
    stdin.flush().expect("flush");

    let mut line = String::new();
    stdout.read_line(&mut line).expect("read");
    let response: Value = serde_json::from_str(line.trim()).expect("response json");
    assert_eq!(response["result"]["isError"], Value::Bool(false));
    let evidence_id = response["result"]["structuredContent"]["evidence_id"]
        .as_str()
        .expect("evidence_id");
    assert!(evidence_id.starts_with("evidence-"), "id = {evidence_id}");

    drop(stdin);
    let _ = child.wait();

    // Re-open the ledger directly and confirm one EvidenceRecorded event landed.
    let ledger = hivemind::ledger::SqliteEventLedger::open(&hivemind_dir).expect("ledger reopens");
    let events = hivemind::ledger::EventLedger::read(&ledger, 0, 16).expect("read events");
    assert_eq!(events.len(), 1, "exactly one event expected: {events:?}");
    assert_eq!(
        events[0].event_type,
        hivemind::events::EventType::EvidenceRecorded
    );
    assert_eq!(events[0].source, hivemind::events::EventSource::Agent);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}
