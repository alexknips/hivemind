//! The request timeout cuts a read off, never a write (hivemind-s9cx).
//!
//! A write that ran past the 30 s timeout used to answer 408 "request timed out" and still
//! commit: the handler's ledger work runs in `spawn_blocking`, which dropping the awaiting future
//! does not stop. The client was told the write failed when it was recorded, and a client that
//! retried recorded it twice.
//!
//! A held SQLite write lock makes both kinds of request slow the way a loaded host does: every
//! ledger open runs `INSERT OR IGNORE INTO tenants`, a write statement, so it waits for the lock.
//! The router under test cuts a read off after 250 ms instead of 30 s.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
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
