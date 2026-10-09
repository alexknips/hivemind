//! The request timeout cuts a read off, never a write (hivemind-s9cx), and cuts off a request
//! body that stalls before any handler has run (hivemind-g3vd0).
//!
//! A write that ran past the 30 s timeout used to answer 408 "request timed out" and still
//! commit: the handler's ledger work runs in `spawn_blocking`, which dropping the awaiting future
//! does not stop. The client was told the write failed when it was recorded, and a client that
//! retried recorded it twice.
//!
//! A held SQLite write lock makes both kinds of request slow the way a loaded host does: every
//! ledger open runs `INSERT OR IGNORE INTO tenants`, a write statement, so it waits for the lock.
//! The router under test cuts a read off after 250 ms instead of 30 s.
//!
//! A write tool called through `POST /mcp` is a write like any other `POST`; a read tool called
//! there is cut off like a `GET`.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

const READ_TIMEOUT: Duration = Duration::from_millis(250);

fn test_ledger_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "hivemind-api-timeout-test-{}",
        uuid::Uuid::new_v4()
    ))
}

fn config(hivemind_dir: &Path) -> hivemind::api::ApiConfig {
    hivemind::api::ApiConfig {
        hivemind_dir: hivemind_dir.to_path_buf(),
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
        slack_api_base_url: None,
    }
}

/// The default router: a 30 s read timeout, far longer than a warm-up request takes.
fn default_app(hivemind_dir: &Path) -> axum::Router {
    hivemind::api::create_router(&config(hivemind_dir))
}

/// A router that cuts a read off after [`READ_TIMEOUT`].
fn short_timeout_app(hivemind_dir: &Path) -> axum::Router {
    hivemind::api::create_router_with_read_timeout(&config(hivemind_dir), READ_TIMEOUT)
}

async fn call(app: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.oneshot(req).await.expect("handler error");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body read error")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-hivemind-actor", "agent:test:session-1")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

fn get_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-hivemind-actor", "agent:test:session-1")
        .body(Body::empty())
        .unwrap()
}

/// A capture that proposes and accepts, so the write is more than one event.
fn capture_body(title: &str) -> Value {
    serde_json::json!({
        "grounding": [{"kind": "bet"}],
        "title": title,
        "rationale": "Timeout fixture: enough words to pass validation",
        "topic_keys": ["request-timeout"],
        "options": [{ "label": "Option one" }, { "label": "Option two" }],
        "chosen_option_label": "Option one",
    })
}

fn ledger_path(hivemind_dir: &Path) -> PathBuf {
    hivemind_dir.join("ledger.sqlite")
}

fn count_events(hivemind_dir: &Path, event_type: &str) -> i64 {
    rusqlite::Connection::open(ledger_path(hivemind_dir))
        .expect("open ledger")
        .query_row(
            "SELECT COUNT(*) FROM events WHERE type = ?1",
            [event_type],
            |row| row.get(0),
        )
        .expect("count events")
}

/// Takes the ledger's write lock and keeps it until the returned connection commits.
fn hold_write_lock(hivemind_dir: &Path) -> rusqlite::Connection {
    let lock = rusqlite::Connection::open(ledger_path(hivemind_dir)).expect("open ledger");
    lock.execute_batch("BEGIN IMMEDIATE").expect("take lock");
    lock
}

#[tokio::test]
async fn a_write_slower_than_the_read_timeout_is_not_cut_off_and_is_recorded_once() {
    let dir = test_ledger_dir();

    // Warm-up: creates the ledger and records one decision, so the counts below have a baseline.
    let (status, warm) = call(
        default_app(&dir),
        post_json("/v1/decisions", capture_body("Warm-up decision")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "warm-up capture: {warm}");
    assert_eq!(count_events(&dir, "decision.proposed"), 1);
    assert_eq!(count_events(&dir, "decision.accepted"), 1);

    let lock = hold_write_lock(&dir);
    let slow_write = tokio::spawn(call(
        short_timeout_app(&dir),
        post_json("/v1/decisions", capture_body("Slow decision")),
    ));

    // Well past the read timeout. The write is still waiting on the lock, so it has answered
    // nothing: a 408 here would already have finished this task.
    tokio::time::sleep(READ_TIMEOUT * 4).await;
    assert!(
        !slow_write.is_finished(),
        "the write answered before the lock was released: the read timeout cut it off"
    );

    lock.execute_batch("COMMIT").expect("release lock");
    let (status, body) = tokio::time::timeout(Duration::from_secs(30), slow_write)
        .await
        .expect("the write finishes once the lock is released")
        .expect("the write task did not panic");

    assert_eq!(
        status,
        StatusCode::OK,
        "a slow write answers with its result, not 408: {body}"
    );
    assert!(
        body["decision_id"].is_string(),
        "the answer names the decision it recorded: {body}"
    );
    assert_eq!(
        count_events(&dir, "decision.proposed"),
        2,
        "the slow write is recorded exactly once"
    );
    assert_eq!(count_events(&dir, "decision.accepted"), 2);
}

#[tokio::test]
async fn a_read_slower_than_the_read_timeout_answers_408() {
    let dir = test_ledger_dir();

    // Warm-up: creates the ledger.
    let (status, health) = call(default_app(&dir), get_req("/v1/health")).await;
    assert_eq!(status, StatusCode::OK, "warm-up health: {health}");

    let lock = hold_write_lock(&dir);
    let (status, body) = call(short_timeout_app(&dir), get_req("/v1/health")).await;
    lock.execute_batch("COMMIT").expect("release lock");

    assert_eq!(
        status,
        StatusCode::REQUEST_TIMEOUT,
        "a read past the timeout is cut off: {body}"
    );
    assert_eq!(body, Value::String("request timed out".to_owned()));
}

/// A request body that delivers `first` and then neither another byte nor an end: a client that
/// sent its headers and a complete payload, and then went quiet without finishing the request.
struct StallingBody {
    first: Option<Bytes>,
}

impl http_body::Body for StallingBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, Infallible>>> {
        match self.first.take() {
            Some(bytes) => Poll::Ready(Some(Ok(http_body::Frame::data(bytes)))),
            None => Poll::Pending,
        }
    }
}

fn post_then_stall(uri: &str, payload: &Value) -> Request<Body> {
    let bytes = Bytes::from(serde_json::to_vec(payload).expect("payload serializes"));
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-hivemind-actor", "agent:test:session-1")
        .body(Body::new(StallingBody { first: Some(bytes) }))
        .expect("request builds")
}

/// A JSON-RPC `tools/call` for `POST /mcp`.
fn mcp_tool_call(tool: &str, arguments: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
}

/// A `capture_decision` call: the MCP twin of [`capture_body`].
fn mcp_capture(title: &str) -> Value {
    let mut arguments = capture_body(title);
    arguments["actor_id"] = Value::String("agent:test:session-1".to_owned());
    mcp_tool_call("capture_decision", arguments)
}

#[tokio::test]
async fn a_body_that_stalls_answers_408_and_records_nothing() {
    let dir = test_ledger_dir();

    // Warm-up: creates the ledger and records one decision, the baseline the counts are held to.
    let (status, warm) = call(
        default_app(&dir),
        post_json("/v1/decisions", capture_body("Warm-up decision")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "warm-up capture: {warm}");

    // The payload is a complete, valid capture. Only the end of the body never arrives, so a
    // handler that ran on what it had would record it.
    for (uri, payload) in [
        ("/v1/decisions", capture_body("Stalled decision")),
        ("/mcp", mcp_capture("Stalled MCP decision")),
    ] {
        let (status, body) = tokio::time::timeout(
            Duration::from_secs(10),
            call(short_timeout_app(&dir), post_then_stall(uri, &payload)),
        )
        .await
        .unwrap_or_else(|_| panic!("POST {uri}: a stalled body held the request open"));

        assert_eq!(status, StatusCode::REQUEST_TIMEOUT, "POST {uri}: {body}");
        assert_eq!(body, Value::String("request body timed out".to_owned()));
        assert_eq!(
            count_events(&dir, "decision.proposed"),
            1,
            "POST {uri}: nothing is recorded from a body that never finished"
        );
    }
}

#[tokio::test]
async fn stalled_bodies_do_not_hold_the_servers_request_slots() {
    let dir = test_ledger_dir();
    let (status, health) = call(default_app(&dir), get_req("/v1/health")).await;
    assert_eq!(status, StatusCode::OK, "warm-up health: {health}");

    // More stalled requests than the server has slots. Each holds a slot only until its body
    // gives up, so every one of them is answered; with no body deadline the surplus waits for a
    // slot that never frees.
    let app = short_timeout_app(&dir);
    let payload = capture_body("Stalled decision");
    let stalled: Vec<_> = (0..hivemind::api::MAX_CONCURRENT_REQUESTS + 10)
        .map(|_| {
            tokio::spawn(call(
                app.clone(),
                post_then_stall("/v1/decisions", &payload),
            ))
        })
        .collect();
    for request in stalled {
        let (status, body) = tokio::time::timeout(Duration::from_secs(30), request)
            .await
            .expect("every stalled request is answered, none waits on a slot a stalled body holds")
            .expect("the request task did not panic");
        assert_eq!(status, StatusCode::REQUEST_TIMEOUT, "{body}");
    }

    let (status, body) = call(app, get_req("/v1/health")).await;
    assert_eq!(status, StatusCode::OK, "the slots are free again: {body}");
    assert_eq!(count_events(&dir, "decision.proposed"), 0);
}

#[tokio::test]
async fn a_read_tool_over_mcp_slower_than_the_read_timeout_answers_a_timeout_error() {
    let dir = test_ledger_dir();

    // Warm-up: creates the ledger, and shows the tool answers when nothing is slow.
    let read = mcp_tool_call("dump_graph", serde_json::json!({}));
    let (status, warm) = call(default_app(&dir), post_json("/mcp", read.clone())).await;
    assert_eq!(status, StatusCode::OK, "warm-up read: {warm}");
    assert_eq!(warm["result"]["isError"], false, "warm-up read: {warm}");

    let lock = hold_write_lock(&dir);
    let (status, body) = tokio::time::timeout(
        Duration::from_secs(10),
        call(short_timeout_app(&dir), post_json("/mcp", read)),
    )
    .await
    .expect("a read tool is cut off at the read timeout, not left running");
    lock.execute_batch("COMMIT").expect("release lock");

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["isError"], true, "{body}");
    let message = body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("timed out") && message.contains("dump_graph"),
        "the error says what was cut off and why: {body}"
    );
}

#[tokio::test]
async fn a_write_tool_over_mcp_slower_than_the_read_timeout_answers_its_result_and_is_recorded_once(
) {
    let dir = test_ledger_dir();

    let (status, warm) = call(
        default_app(&dir),
        post_json("/mcp", mcp_capture("Warm-up decision")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "warm-up capture: {warm}");
    assert_eq!(warm["result"]["isError"], false, "warm-up capture: {warm}");
    assert_eq!(count_events(&dir, "decision.proposed"), 1);

    let lock = hold_write_lock(&dir);
    let slow_write = tokio::spawn(call(
        short_timeout_app(&dir),
        post_json("/mcp", mcp_capture("Slow MCP decision")),
    ));

    // Well past the read timeout. The write is still waiting on the lock, so it has answered
    // nothing: a timeout error here would already have finished this task.
    tokio::time::sleep(READ_TIMEOUT * 4).await;
    assert!(
        !slow_write.is_finished(),
        "the write tool answered before the lock was released: the read timeout cut it off"
    );

    lock.execute_batch("COMMIT").expect("release lock");
    let (status, body) = tokio::time::timeout(Duration::from_secs(30), slow_write)
        .await
        .expect("the write finishes once the lock is released")
        .expect("the write task did not panic");

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["result"]["isError"], false,
        "a slow write tool answers its result, not a timeout: {body}"
    );
    assert!(
        body["result"]["structuredContent"]["decision_id"].is_string(),
        "the answer names the decision it recorded: {body}"
    );
    assert_eq!(
        count_events(&dir, "decision.proposed"),
        2,
        "the slow write is recorded exactly once"
    );
}
