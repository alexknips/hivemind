//! The daily classification cap over HTTP (hivemind-zdsh.18).
//!
//! This is its own test binary because `HIVEMIND_CLASSIFY_DAILY_CAP` is process-global: the test
//! lowers it to 1, and in `tests/api.rs` that raced every test recording more than one
//! classification into a ledger (hivemind-9512). A separate binary is a separate process, so no
//! other test can meet the lowered cap.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

fn test_ledger_dir() -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-api-cap-test-{}", uuid::Uuid::new_v4()))
}

fn app(hivemind_dir: PathBuf) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
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
    };
    hivemind::api::create_router(&config)
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
    let body: Value = serde_json::from_slice(&bytes).expect("body is JSON");
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

fn ingest_json(batch_id: &str, session_id: &str) -> Value {
    serde_json::json!({
        "batch_id": batch_id,
        "agent_tool": "claude",
        "session_id": session_id,
        "turns": [
            { "turn_id": "t1", "role": "user", "text": "note", "truncated": false }
        ]
    })
}

#[tokio::test]
async fn classify_queue_submit_enforces_daily_cap() {
    let dir = test_ledger_dir();

    for batch_id in ["cap-sess:0-1", "cap-sess:1-2"] {
        let (status, body) = call(
            app(dir.clone()),
            post_json("/v1/ingest", ingest_json(batch_id, "cap-sess")),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}"); // ubs:ignore
    }

    // The only test in this binary, so nothing else reads the variable while it is lowered.
    let saved = std::env::var("HIVEMIND_CLASSIFY_DAILY_CAP").ok();
    unsafe { std::env::set_var("HIVEMIND_CLASSIFY_DAILY_CAP", "1") };

    let submit_one = |batch_id: &'static str| {
        post_json(
            "/v1/classify-queue/submit",
            serde_json::json!({
                "batch_ids": [batch_id],
                "captures": [],
                "model": "agent:worker-a"
            }),
        )
    };

    let (status, body) = call(app(dir.clone()), submit_one("cap-sess:0-1")).await;
    assert_eq!(status, StatusCode::OK, "first submit under cap: {body}"); // ubs:ignore

    let (status, body) = call(app(dir.clone()), submit_one("cap-sess:1-2")).await;

    match saved {
        Some(v) => unsafe { std::env::set_var("HIVEMIND_CLASSIFY_DAILY_CAP", v) },
        None => unsafe { std::env::remove_var("HIVEMIND_CLASSIFY_DAILY_CAP") },
    }

    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "second submit over cap: {body}"
    ); // ubs:ignore
    assert_eq!(body["error"]["code"], "too_many_requests"); // ubs:ignore

    // The over-cap batch stays pending — not dropped.
    let (status, body) = call(app(dir), get_req("/v1/classify-queue?session_id=cap-sess")).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    let batches = body["batches"].as_array().unwrap(); // ubs:ignore
    assert_eq!(batches.len(), 1, "{body}"); // ubs:ignore
    assert_eq!(batches[0]["batch_id"], "cap-sess:1-2"); // ubs:ignore
}
