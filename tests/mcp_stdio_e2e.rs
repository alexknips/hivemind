//! End-to-end MCP stdio test: spawns the real `hivemind mcp` binary, talks to
//! it over its stdin/stdout pipes the same way an MCP client would, and
//! confirms a captured decision lands in the ledger and is retrievable on a
//! follow-up call.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::thread;

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

#[test]
fn mcp_stdio_servers_share_sqlite_wal_ledger_under_concurrent_writes() {
    let hivemind_dir = unique_dir("concurrent");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for server_index in 0..2 {
        let hivemind_dir = hivemind_dir.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            capture_decision_through_mcp_server(server_index, hivemind_dir, barrier)
        }));
    }

    let mut decision_ids = BTreeSet::new();
    for handle in handles {
        let decision_id = handle
            .join()
            .expect("mcp worker thread joins")
            .expect("mcp worker captures decision");
        assert!(
            decision_ids.insert(decision_id),
            "concurrent captures must produce distinct decision ids"
        );
    }

    let ledger = hivemind::ledger::SqliteEventLedger::open(&hivemind_dir).expect("ledger reopens");
    let expected_events = 4;
    assert_eq!(
        hivemind::ledger::EventLedger::latest_offset(&ledger).expect("latest offset"),
        expected_events
    );

    let events = hivemind::ledger::EventLedger::read(&ledger, 0, expected_events as usize + 1)
        .expect("read all events");
    assert_eq!(events.len(), expected_events as usize);

    let mut seen_event_ids = BTreeSet::new();
    let mut decision_event_ids = BTreeSet::new();
    let mut decision_count = 0;
    let mut relation_count = 0;
    for (index, event) in events.iter().enumerate() {
        let event_id = event.event_id.expect("stored event has event_id");
        assert!(
            seen_event_ids.insert(event_id),
            "duplicate event_id {event_id}"
        );
        assert_eq!(event_id, (index + 1) as u64);

        match event.event_type {
            hivemind::events::EventType::DecisionProposed => {
                decision_count += 1;
                decision_event_ids.insert(event_id);
            }
            hivemind::events::EventType::RelationAdded => {
                relation_count += 1;
            }
            _ => {}
        }
    }

    assert_eq!(decision_count, 2);
    assert_eq!(relation_count, 2);

    for event in events
        .iter()
        .filter(|event| event.event_type == hivemind::events::EventType::RelationAdded)
    {
        let relation_event_id = event.event_id.expect("stored event has event_id");
        let root_event_id = event
            .causation_event_id
            .expect("MCP proposal relation records causation_event_id");
        assert!(
            root_event_id < relation_event_id,
            "relation {relation_event_id} must follow root {root_event_id}"
        );
        assert!(
            decision_event_ids.contains(&root_event_id),
            "relation {relation_event_id} points to missing decision event {root_event_id}"
        );
    }

    let mut replayed_event_ids = Vec::new();
    hivemind::ledger::EventLedger::replay_from(&ledger, 0, &mut |event| {
        replayed_event_ids.push(event.event_id.expect("replayed event has event_id"));
        Ok(())
    })
    .expect("replay succeeds");
    assert_eq!(replayed_event_ids, vec![1, 2, 3, 4]);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

fn capture_decision_through_mcp_server(
    server_index: usize,
    hivemind_dir: PathBuf,
    barrier: Arc<Barrier>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("mcp")
        .arg("--session-id")
        .arg(format!("concurrent-{server_index}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    send_mcp_request(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
    )?;
    let init = read_mcp_response(&mut stdout)?;
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");

    barrier.wait();

    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "capture_decision",
                "arguments": {
                    "actor_id": format!("agent:e2e:{server_index}"),
                    "title": format!("Concurrent MCP write {server_index}"),
                    "rationale": "Exercise independent MCP subprocesses sharing one WAL ledger.",
                    "topic_keys": ["sqlite-wal", "dogfood"],
                    "options": [{"label": format!("mcp-writer-{server_index}")}]
                }
            }
        }),
    )?;
    let captured = read_mcp_response(&mut stdout)?;
    assert_eq!(captured["result"]["isError"], Value::Bool(false));
    let decision_id = captured["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id present")
        .to_owned();

    drop(stdin);
    let status = child.wait()?;
    assert!(status.success(), "server exited non-zero: {status:?}");

    Ok(decision_id)
}

fn send_mcp_request(
    stdin: &mut impl Write,
    request: Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    writeln!(stdin, "{request}")?;
    stdin.flush()?;
    Ok(())
}

fn read_mcp_response(
    stdout: &mut impl BufRead,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut line = String::new();
    stdout.read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}
