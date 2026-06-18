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
fn mcp_stdio_servers_share_sqlite_wal_ledger_under_concurrent_writes(
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    let mut unexpected_event_type = None;
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
            other => {
                unexpected_event_type = Some(other);
                break;
            }
        }
    }

    if let Some(other) = unexpected_event_type {
        return Err(format!("unexpected event type from MCP capture: {other:?}").into());
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
    Ok(())
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

// ---------------------------------------------------------------------------
// M3 read tool e2e tests
// ---------------------------------------------------------------------------

#[test]
fn mcp_search_decisions_e2e() {
    let hivemind_dir = unique_dir("search");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("mcp")
        .arg("--session-id")
        .arg("search-e2e")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hivemind mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    // handshake
    send_mcp_request(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
    )
    .expect("send initialize");
    let init = read_mcp_response(&mut stdout).expect("read initialize");
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");

    // capture a decision with a distinctive keyword
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {
                "name": "capture_decision",
                "arguments": {
                    "actor_id": "agent:test:search-e2e",
                    "title": "Use quorum-consensus for distributed writes",
                    "rationale": "Quorum-consensus ensures durability across nodes",
                    "topic_keys": ["distributed-systems"],
                    "options": [
                        {"label": "quorum-consensus"},
                        {"label": "single-leader"}
                    ],
                    "chosen_option_label": "quorum-consensus"
                }
            }
        }),
    )
    .expect("send capture");
    let captured = read_mcp_response(&mut stdout).expect("read capture");
    assert_eq!(captured["result"]["isError"], Value::Bool(false));
    let decision_id = captured["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id")
        .to_owned();

    // search with a keyword unique to this title
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {
                "name": "search_decisions",
                "arguments": {
                    "q": "quorum-consensus",
                    "limit": 10
                }
            }
        }),
    )
    .expect("send search");
    let searched = read_mcp_response(&mut stdout).expect("read search");
    assert_eq!(searched["result"]["isError"], Value::Bool(false));
    let sc = &searched["result"]["structuredContent"];
    assert_eq!(sc["result_count"], Value::from(1u64), "expected 1 hit");
    assert_eq!(
        sc["data"]["items"][0]["decision"]["id"].as_str(),
        Some(decision_id.as_str()),
        "search result id mismatch"
    );

    drop(stdin);
    let status = child.wait().expect("wait child");
    assert!(status.success(), "server exited non-zero: {status:?}");
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn mcp_summarize_decisions_e2e() {
    let hivemind_dir = unique_dir("summarize");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("mcp")
        .arg("--session-id")
        .arg("summarize-e2e")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hivemind mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    send_mcp_request(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
    )
    .expect("send initialize");
    read_mcp_response(&mut stdout).expect("read initialize");

    // capture a decision
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {
                "name": "capture_decision",
                "arguments": {
                    "actor_id": "agent:test:summarize-e2e",
                    "title": "Adopt event-driven architecture",
                    "rationale": "Loose coupling via event streams reduces blast radius of failures",
                    "topic_keys": ["architecture"],
                    "options": [
                        {"label": "event-driven", "description": "Publish domain events"},
                        {"label": "synchronous-rpc", "description": "Direct service calls"}
                    ],
                    "chosen_option_label": "event-driven"
                }
            }
        }),
    )
    .expect("send capture");
    let captured = read_mcp_response(&mut stdout).expect("read capture");
    assert_eq!(captured["result"]["isError"], Value::Bool(false));
    let decision_id = captured["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id")
        .to_owned();

    // summarize (single mode — default for one id)
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {
                "name": "summarize_decisions",
                "arguments": { "decision_ids": [decision_id] }
            }
        }),
    )
    .expect("send summarize");
    let summarized = read_mcp_response(&mut stdout).expect("read summarize");
    assert_eq!(summarized["result"]["isError"], Value::Bool(false));
    let data = &summarized["result"]["structuredContent"]["data"];
    let summary = data["summary"].as_str().expect("summary text");
    assert!(!summary.is_empty(), "summary must not be empty");
    let cited = data["cited_decision_ids"]
        .as_array()
        .expect("cited_decision_ids array");
    assert!(
        cited.iter().any(|id| id.as_str() == Some(&decision_id)),
        "captured decision must appear in cited_decision_ids"
    );
    assert_eq!(data["unit"], Value::from("single"), "mode must be single");

    // supersede and test chain mode
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {
                "name": "supersede_decision",
                "arguments": {
                    "old_decision_id": decision_id,
                    "title": "Adopt event-driven architecture v2",
                    "rationale": "Adding schema registry for event contracts",
                    "options": [{"label": "event-driven-v2"}],
                    "chosen_option_label": "event-driven-v2"
                }
            }
        }),
    )
    .expect("send supersede");
    let superseded = read_mcp_response(&mut stdout).expect("read supersede");
    assert_eq!(superseded["result"]["isError"], Value::Bool(false));

    // chain mode: walk the supersession chain from the original id
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {
                "name": "summarize_decisions",
                "arguments": { "decision_ids": [decision_id], "mode": "chain" }
            }
        }),
    )
    .expect("send summarize chain");
    let chain_summary = read_mcp_response(&mut stdout).expect("read summarize chain");
    assert_eq!(chain_summary["result"]["isError"], Value::Bool(false));
    let chain_data = &chain_summary["result"]["structuredContent"]["data"];
    assert_eq!(
        chain_data["unit"],
        Value::from("chain"),
        "mode must be chain"
    );
    let chain_cited = chain_data["cited_decision_ids"]
        .as_array()
        .expect("cited array");
    assert!(
        chain_cited.len() >= 2,
        "chain mode must cite at least both decisions in the chain"
    );

    drop(stdin);
    let status = child.wait().expect("wait child");
    assert!(status.success(), "server exited non-zero: {status:?}");
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn mcp_compact_view_e2e() {
    let hivemind_dir = unique_dir("compact");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("mcp")
        .arg("--session-id")
        .arg("compact-e2e")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hivemind mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    send_mcp_request(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
    )
    .expect("send initialize");
    read_mcp_response(&mut stdout).expect("read initialize");

    // capture decision
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {
                "name": "capture_decision",
                "arguments": {
                    "actor_id": "agent:test:compact-e2e",
                    "title": "Deploy on bare-metal servers",
                    "rationale": "Lower operational cost at current scale",
                    "topic_keys": ["infra"],
                    "options": [
                        {"label": "bare-metal"},
                        {"label": "kubernetes"}
                    ],
                    "chosen_option_label": "bare-metal"
                }
            }
        }),
    )
    .expect("send capture");
    let captured = read_mcp_response(&mut stdout).expect("read capture");
    assert_eq!(captured["result"]["isError"], Value::Bool(false));
    let original_id = captured["result"]["structuredContent"]["decision_id"]
        .as_str()
        .expect("decision_id")
        .to_owned();

    // supersede it — the compact view on the original id should walk to the terminal
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {
                "name": "supersede_decision",
                "arguments": {
                    "old_decision_id": original_id,
                    "title": "Deploy on managed Kubernetes",
                    "rationale": "Scale requirements exceeded bare-metal maintenance budget",
                    "options": [{"label": "kubernetes"}],
                    "chosen_option_label": "kubernetes"
                }
            }
        }),
    )
    .expect("send supersede");
    let superseded = read_mcp_response(&mut stdout).expect("read supersede");
    assert_eq!(superseded["result"]["isError"], Value::Bool(false));
    let new_id = superseded["result"]["structuredContent"]["new_decision_id"]
        .as_str()
        .expect("new_decision_id")
        .to_owned();

    // compact view on the original id — should return the terminal (new_id) as focal decision
    send_mcp_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {
                "name": "hivemind_compact_view",
                "arguments": { "decision_id": original_id }
            }
        }),
    )
    .expect("send compact_view");
    let compacted = read_mcp_response(&mut stdout).expect("read compact_view");
    assert_eq!(compacted["result"]["isError"], Value::Bool(false));
    let sc = &compacted["result"]["structuredContent"];
    // result_count == 1 when found
    assert_eq!(sc["result_count"], Value::from(1u64), "expected 1 result");
    let data = &sc["data"];
    // focal decision is the terminal (new) decision
    assert_eq!(
        data["decision"]["id"].as_str(),
        Some(new_id.as_str()),
        "compact view focal node must be the terminal decision"
    );
    // supersession_chain is present with chain_length == 2 and oldest_id == original_id
    let chain = &data["supersession_chain"];
    assert_eq!(
        chain["chain_length"],
        Value::from(2u64),
        "chain_length must be 2"
    );
    assert_eq!(
        chain["oldest_id"].as_str(),
        Some(original_id.as_str()),
        "oldest_id must be the original decision"
    );

    drop(stdin);
    let status = child.wait().expect("wait child");
    assert!(status.success(), "server exited non-zero: {status:?}");
    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn mcp_m3_tools_are_tenant_isolated() {
    let hivemind_dir = unique_dir("tenant-iso");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    // Spawn tenant-a process, capture a decision with a unique keyword.
    let decision_id_a = {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
            .arg("--hivemind-dir")
            .arg(&hivemind_dir)
            .arg("--tenant")
            .arg("tenant-a")
            .arg("mcp")
            .arg("--session-id")
            .arg("tenant-a-session")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tenant-a");

        let mut stdin = child.stdin.take().expect("stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

        send_mcp_request(
            &mut stdin,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .expect("send init a");
        read_mcp_response(&mut stdout).expect("read init a");

        send_mcp_request(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {
                    "name": "capture_decision",
                    "arguments": {
                        "actor_id": "agent:test:tenant-a",
                        "title": "TenantA uses columnstore indexes",
                        "rationale": "Analytical workloads benefit from columnar layout",
                        "topic_keys": ["tenantA-database"],
                        "options": [{"label": "columnstore"}]
                    }
                }
            }),
        )
        .expect("send capture a");
        let resp = read_mcp_response(&mut stdout).expect("read capture a");
        assert_eq!(resp["result"]["isError"], Value::Bool(false));
        let id = resp["result"]["structuredContent"]["decision_id"]
            .as_str()
            .expect("decision_id")
            .to_owned();
        drop(stdin);
        child.wait().expect("wait tenant-a");
        id
    };

    // Spawn tenant-b process, capture a different decision.
    let decision_id_b = {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
            .arg("--hivemind-dir")
            .arg(&hivemind_dir)
            .arg("--tenant")
            .arg("tenant-b")
            .arg("mcp")
            .arg("--session-id")
            .arg("tenant-b-session")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tenant-b");

        let mut stdin = child.stdin.take().expect("stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

        send_mcp_request(
            &mut stdin,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .expect("send init b");
        read_mcp_response(&mut stdout).expect("read init b");

        send_mcp_request(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {
                    "name": "capture_decision",
                    "arguments": {
                        "actor_id": "agent:test:tenant-b",
                        "title": "TenantB uses row-oriented storage",
                        "rationale": "OLTP workloads are row-access dominant",
                        "topic_keys": ["tenantB-database"],
                        "options": [{"label": "rowstore"}]
                    }
                }
            }),
        )
        .expect("send capture b");
        let resp = read_mcp_response(&mut stdout).expect("read capture b");
        assert_eq!(resp["result"]["isError"], Value::Bool(false));
        let id = resp["result"]["structuredContent"]["decision_id"]
            .as_str()
            .expect("decision_id")
            .to_owned();
        drop(stdin);
        child.wait().expect("wait tenant-b");
        id
    };

    // Confirm the two decisions have distinct IDs.
    assert_ne!(
        decision_id_a, decision_id_b,
        "decisions from different tenants must not collide"
    );

    // Query tenant-a: search for tenant-a's keyword — must NOT see tenant-b's data.
    {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
            .arg("--hivemind-dir")
            .arg(&hivemind_dir)
            .arg("--tenant")
            .arg("tenant-a")
            .arg("mcp")
            .arg("--session-id")
            .arg("tenant-a-query")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tenant-a query");

        let mut stdin = child.stdin.take().expect("stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

        send_mcp_request(
            &mut stdin,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .expect("send init");
        read_mcp_response(&mut stdout).expect("read init");

        // Broad search — tenant-a should only see its own decision.
        send_mcp_request(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {
                    "name": "search_decisions",
                    "arguments": { "limit": 100 }
                }
            }),
        )
        .expect("send search a");
        let resp = read_mcp_response(&mut stdout).expect("read search a");
        assert_eq!(resp["result"]["isError"], Value::Bool(false));
        let sc = &resp["result"]["structuredContent"];
        assert_eq!(
            sc["result_count"],
            Value::from(1u64),
            "tenant-a must see exactly 1 decision, not tenant-b's data"
        );
        assert_eq!(
            sc["data"]["items"][0]["decision"]["id"].as_str(),
            Some(decision_id_a.as_str()),
            "tenant-a search must return only tenant-a's decision"
        );

        // Summarize tenant-a's decision — must succeed.
        send_mcp_request(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {
                    "name": "summarize_decisions",
                    "arguments": { "decision_ids": [decision_id_a] }
                }
            }),
        )
        .expect("send summarize a");
        let sum_resp = read_mcp_response(&mut stdout).expect("read summarize a");
        assert_eq!(
            sum_resp["result"]["isError"],
            Value::Bool(false),
            "tenant-a can summarize its own decision"
        );

        // Summarize tenant-b's decision from tenant-a's session — must return isError.
        send_mcp_request(
            &mut stdin,
            json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": {
                    "name": "summarize_decisions",
                    "arguments": { "decision_ids": [decision_id_b] }
                }
            }),
        )
        .expect("send summarize cross-tenant");
        let cross_resp = read_mcp_response(&mut stdout).expect("read summarize cross-tenant");
        assert_eq!(
            cross_resp["result"]["isError"],
            Value::Bool(true),
            "tenant-a must not summarize tenant-b's decision"
        );

        drop(stdin);
        child.wait().expect("wait tenant-a query");
    }

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}
