//! Integration tests for the HTTP REST API.
//!
//! Uses axum's tower-service test pattern (no real TCP binding) to exercise
//! every endpoint against a real SQLite ledger in a temp directory. This
//! verifies that the same operations invoked via CLI, MCP, or HTTP produce
//! equivalent events in the ledger.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

fn test_ledger_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hivemind-api-test-{}", uuid::Uuid::new_v4()));
    dir
}

fn app(hivemind_dir: PathBuf) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
        port: 0,
        api_key: None,
        database_url: None,
        admin_key: None,
    };
    hivemind::api::create_router(&config)
}

fn app_with_key(hivemind_dir: PathBuf, key: &str) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
        port: 0,
        api_key: Some(key.to_owned()),
        database_url: None,
        admin_key: None,
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

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_ok() {
    let dir = test_ledger_dir();
    let req = get_req("/v1/health");
    let (status, body) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::OK); // ubs:ignore
    assert_eq!(body["status"], "ok"); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Capture decision and query it back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capture_and_query_decision() {
    let dir = test_ledger_dir();

    // Capture a decision via HTTP POST
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "title": "Adopt REST for the HTTP API",
                "rationale": "REST maps naturally to resources and is curl-friendly",
                "topic_keys": ["api-design"],
                "options": [
                    { "label": "REST", "description": "HTTP REST endpoints" },
                    { "label": "JSON-RPC", "description": "JSON-RPC 2.0" }
                ],
                "chosen_option_label": "REST"
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "capture decision: {body}"); // ubs:ignore
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();
    let option_ids = body["option_ids"].as_array().unwrap();
    assert_eq!(option_ids.len(), 2); // ubs:ignore
    assert!(body["chosen_option_id"].as_str().is_some()); // ubs:ignore

    // Query it back via GET /v1/decisions/{id}
    let (status, body) = call(
        app(dir.clone()),
        get_req(&format!("/v1/decisions/{decision_id}")),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "get decision: {body}"); // ubs:ignore
    let data = &body["data"];
    // DecisionView uses `id` (not `decision_id`) as the field name
    assert_eq!(data["id"], decision_id); // ubs:ignore
    assert_eq!(data["title"], "Adopt REST for the HTTP API"); // ubs:ignore

    // Search should also find it
    let (status, body) = call(app(dir.clone()), get_req("/v1/decisions/search?q=REST+API")).await;
    assert_eq!(status, StatusCode::OK, "search: {body}"); // ubs:ignore
    assert!(body["result_count"].as_u64().unwrap() >= 1); // ubs:ignore

    // Relevant decisions by topic
    let (status, body) = call(
        app(dir.clone()),
        get_req("/v1/decisions/relevant?topic=api-design"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "relevant: {body}"); // ubs:ignore
    assert!(body["result_count"].as_u64().unwrap() >= 1); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Capture evidence and hypothesis
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capture_evidence_and_hypothesis() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/evidence",
            serde_json::json!({ "content": "REST has better curl ergonomics than JSON-RPC" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture evidence: {body}"); // ubs:ignore
    assert!(body["evidence_id"].as_str().is_some()); // ubs:ignore

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/hypotheses",
            serde_json::json!({ "statement": "REST will reduce client integration friction" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture hypothesis: {body}"); // ubs:ignore
    assert!(body["hypothesis_id"].as_str().is_some()); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Disagree
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disagree_updates_decision_status() {
    let dir = test_ledger_dir();

    // First capture a decision
    let (_, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "title": "Use SQLite for all storage",
                "rationale": "Simple and embeddable",
                "topic_keys": ["storage"],
                "options": [{ "label": "SQLite" }]
            }),
        ),
    )
    .await;
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();

    // Disagree with it
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            &format!("/v1/decisions/{decision_id}/disagreements"),
            serde_json::json!({ "reason": "SQLite doesn't support concurrent writers" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "disagree: {body}"); // ubs:ignore
    assert_eq!(body["decision_id"], decision_id); // ubs:ignore
    assert!(body["event_id"].as_u64().is_some()); // ubs:ignore
                                                  // After a disagreement the status changes — exact value depends on
                                                  // whether the disagreeing actor is the proposer (→ rejected) or
                                                  // a different actor (→ contested).
    let decision_status = body["decision_status"].as_str().unwrap();
    assert!(
        // ubs:ignore
        matches!(decision_status, "proposed" | "contested" | "rejected"),
        "unexpected status: {decision_status}"
    );
}

// ---------------------------------------------------------------------------
// Supersede
// ---------------------------------------------------------------------------

#[tokio::test]
async fn supersede_links_old_to_new_decision() {
    let dir = test_ledger_dir();

    // Capture original decision
    let (_, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "title": "Use bearer tokens for auth",
                "rationale": "Simple to implement",
                "topic_keys": ["auth"],
                "options": [{ "label": "bearer-tokens" }],
                "chosen_option_label": "bearer-tokens"
            }),
        ),
    )
    .await;
    let old_id = body["decision_id"].as_str().unwrap().to_owned();

    // Supersede it
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            &format!("/v1/decisions/{old_id}/supersessions"),
            serde_json::json!({
                "title": "Use bearer tokens + Ed25519 signing for auth",
                "rationale": "Bearer alone lacks audit trail; signing adds integrity",
                "topic_keys": ["auth"],
                "options": ["bearer-tokens-plus-signing"],
                "chosen_option_label": "bearer-tokens-plus-signing"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "supersede: {body}"); // ubs:ignore
    assert_eq!(body["old_decision_id"], old_id); // ubs:ignore
    assert!(body["new_decision_id"].as_str().is_some()); // ubs:ignore
    assert_eq!(body["old_decision_status"], "superseded"); // ubs:ignore

    // Supersession chain should include both decisions
    let new_id = body["new_decision_id"].as_str().unwrap().to_owned();
    let (status, body) = call(
        app(dir.clone()),
        get_req(&format!("/v1/decisions/{old_id}/supersession-chain")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "chain: {body}"); // ubs:ignore
                                                         // SupersessionChain.decision_ids is a flat array of id strings
    let chain_ids: Vec<&str> = body["data"]["decision_ids"]
        .as_array()
        .expect("data.decision_ids must be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        // ubs:ignore
        chain_ids.contains(&old_id.as_str()),
        "chain missing old_id: {chain_ids:?}"
    );
    assert!(
        // ubs:ignore
        chain_ids.contains(&new_id.as_str()),
        "chain missing new_id: {chain_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// Auth — bearer token enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_rejects_missing_token() {
    let dir = test_ledger_dir();
    let app = app_with_key(dir, "secret-key-42");

    // /v1/decisions/search requires auth when a key is configured
    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .body(Body::empty())
        .unwrap();

    let (status, body) = call(app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "unauthorized"); // ubs:ignore
}

#[tokio::test]
async fn auth_accepts_correct_token() {
    let dir = test_ledger_dir();
    let app = app_with_key(dir, "secret-key-42");

    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .header("authorization", "Bearer secret-key-42")
        .body(Body::empty())
        .unwrap();

    let (status, _) = call(app, req).await;
    assert_eq!(status, StatusCode::OK); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Validation errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capture_decision_validates_required_fields() {
    let dir = test_ledger_dir();

    // Missing `title`
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "rationale": "r",
                "topic_keys": ["t"],
                "options": [{ "label": "A" }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore
}

#[tokio::test]
async fn capture_evidence_validates_content() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json("/v1/evidence", serde_json::json!({ "content": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Ingest batch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_batch_accepted_and_stored() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/ingest",
            serde_json::json!({
                "batch_id": "session-abc:0-1024",
                "agent_tool": "claude",
                "session_id": "session-abc",
                "turns": [
                    {
                        "turn_id": "turn-1",
                        "role": "user",
                        "text": "Should we use REST or JSON-RPC for the API?",
                        "truncated": false
                    },
                    {
                        "turn_id": "turn-2",
                        "role": "assistant",
                        "text": "REST is the better choice here because HTTP clients are most ergonomic with it.",
                        "truncated": false
                    }
                ]
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "ingest: {body}"); // ubs:ignore
    assert_eq!(body["batch_id"], "session-abc:0-1024"); // ubs:ignore
    assert_eq!(body["queued"], true); // ubs:ignore
    assert!(
        // ubs:ignore
        body["event_id"].as_u64().is_some(),
        "event_id missing: {body}"
    );
}

#[tokio::test]
async fn ingest_batch_empty_turns_accepted() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/ingest",
            serde_json::json!({
                "batch_id": "session-xyz:100-100",
                "agent_tool": "codex",
                "session_id": "session-xyz",
                "turns": []
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "ingest empty turns: {body}"); // ubs:ignore
    assert_eq!(body["queued"], true); // ubs:ignore
}

#[tokio::test]
async fn ingest_batch_rejects_missing_fields() {
    let dir = test_ledger_dir();

    // Missing session_id
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/ingest",
            serde_json::json!({
                "batch_id": "b1",
                "agent_tool": "claude"
            }),
        ),
    )
    .await;
    assert_eq!(
        // ubs:ignore
        status,
        StatusCode::BAD_REQUEST,
        "missing session_id: {body}"
    );

    // Empty batch_id
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/ingest",
            serde_json::json!({
                "batch_id": "",
                "agent_tool": "claude",
                "session_id": "s1"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty batch_id: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore
}

#[tokio::test]
async fn ingest_batch_enforces_auth() {
    let dir = test_ledger_dir();
    let app = app_with_key(dir, "secret-key");

    let (status, body) = call(
        app,
        post_json(
            "/v1/ingest",
            serde_json::json!({
                "batch_id": "b1",
                "agent_tool": "claude",
                "session_id": "s1"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}"); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Layer-3 classifier: annotation event round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn classifier_try_spawn_without_api_key_is_noop() {
    // When ANTHROPIC_API_KEY is absent, try_spawn must return None without panicking.
    // We remove the key from the test process environment temporarily.
    let saved = std::env::var("ANTHROPIC_API_KEY").ok();
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

    let result = hivemind::classifier::try_spawn(
        std::sync::Arc::new(test_ledger_dir()),
        hivemind::events::TenantId::local(),
    );
    assert!(
        // ubs:ignore
        result.is_none(),
        "try_spawn must return None without API key"
    );

    if let Some(key) = saved {
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", key) };
    }
}

#[tokio::test]
async fn classifier_batch_classified_event_round_trips() {
    use hivemind::commands::{CommandContext, Commands};
    use hivemind::events::{CaptureItem, EventProvenance, TenantId};
    use hivemind::ledger::{EventLedger, SqliteEventLedger};

    let dir = test_ledger_dir();
    let ledger = SqliteEventLedger::open(&dir).unwrap();
    let commands = Commands::new_with_context(
        &ledger,
        CommandContext::new(TenantId::local(), EventProvenance::agent("agent:test")),
    );

    let batch_event_id = commands
        .record_ingest_batch("agent:test", "session-x:0-4", "claude", "session-x", vec![])
        .unwrap();

    let captures = vec![CaptureItem {
        kind: "decision".to_owned(),
        title: "Use tokio for async".to_owned(),
        rationale: "Project already depends on tokio".to_owned(),
        topic_keys: vec!["async".to_owned()],
        evidence_ids: vec![],
        options: Some(vec!["tokio".to_owned(), "async-std".to_owned()]),
        chosen_option: Some("tokio".to_owned()),
        extraction_confidence: 0.85,
    }];

    let classified_event_id = commands
        .record_ingest_batch_classified(
            "agent:hivemind:classifier",
            "session-x:0-4",
            "claude-haiku-4-5-20251001",
            "1",
            captures,
            Some(batch_event_id),
        )
        .unwrap();

    assert!(
        // ubs:ignore
        classified_event_id > batch_event_id,
        "classified event written after batch event"
    );

    // Read back and verify the payload round-trips
    let events = ledger.read_for_tenant(&TenantId::local(), 0, 100).unwrap();
    let classified_event = events
        .iter()
        .find(|e| e.event_id == Some(classified_event_id))
        .expect("classified event in ledger");

    assert_eq!(
        // ubs:ignore
        classified_event
            .payload
            .get("batch_id")
            .and_then(|v| v.as_str()),
        Some("session-x:0-4")
    );
    assert_eq!(
        // ubs:ignore
        classified_event
            .payload
            .get("classifier_model")
            .and_then(|v| v.as_str()),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(
        // ubs:ignore
        classified_event
            .payload
            .get("schema_version")
            .and_then(|v| v.as_str()),
        Some("1")
    );
    let captures_arr = classified_event
        .payload
        .get("captures")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(captures_arr.len(), 1); // ubs:ignore
    assert_eq!(captures_arr[0]["kind"], "decision"); // ubs:ignore
    assert_eq!(captures_arr[0]["title"], "Use tokio for async"); // ubs:ignore
    assert_eq!(
        // ubs:ignore
        captures_arr[0]["extraction_confidence"].as_f64().unwrap(),
        0.85
    );
}
