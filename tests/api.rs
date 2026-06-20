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
        base_url: None,
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
        base_url: None,
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

fn post_json_as_tenant(uri: &str, body: Value, tenant: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-hivemind-actor", "agent:test:session-1")
        .header("x-hivemind-tenant", tenant)
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

fn get_req_as_tenant(uri: &str, tenant: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-hivemind-actor", "agent:test:session-1")
        .header("x-hivemind-tenant", tenant)
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
// Search filter coverage: source and comma-separated actor_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_source_filter_matches_api_source() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "actor_id": "agent:test:src-filter",
                "title": "Source filter test decision",
                "rationale": "Verifies source param is wired into search",
                "topic_keys": ["source-test"],
                "options": [{"label": "opt"}]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore

    // source=api must match (HTTP API decisions get source=api)
    let (status, body) = call(
        app(dir.clone()),
        get_req("/v1/decisions/search?q=source+filter&source=api"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search source=api: {body}"); // ubs:ignore
    assert!(body["result_count"].as_u64().unwrap() >= 1); // ubs:ignore

    // source=agent must not match
    let (status, body) = call(
        app(dir.clone()),
        get_req("/v1/decisions/search?q=source+filter&source=agent"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search source=agent: {body}"); // ubs:ignore
    assert_eq!(body["result_count"].as_u64().unwrap(), 0); // ubs:ignore
}

#[tokio::test]
async fn search_actor_id_accepts_comma_separated_list() {
    let dir = test_ledger_dir();

    // post_json sends X-Hivemind-Actor: agent:test:session-1 — that becomes the stored actor_id
    let (status, _) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "title": "Actor list filter decision",
                "rationale": "Verifies comma-separated actor_id is wired",
                "topic_keys": ["actor-test"],
                "options": [{"label": "choice"}]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK); // ubs:ignore

    // Matching actor in a comma-separated list must find the decision
    let (status, body) = call(
        app(dir.clone()),
        get_req(
            "/v1/decisions/search?q=actor+list&actor_id=agent:test:nobody,agent:test:session-1",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search actor list: {body}"); // ubs:ignore
    assert!(body["result_count"].as_u64().unwrap() >= 1); // ubs:ignore

    // Non-matching actor must return zero
    let (status, body) = call(
        app(dir.clone()),
        get_req("/v1/decisions/search?q=actor+list&actor_id=agent:test:nobody"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search no match: {body}"); // ubs:ignore
    assert_eq!(body["result_count"].as_u64().unwrap(), 0); // ubs:ignore
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

#[tokio::test]
async fn auth_rejects_wrong_token() {
    let dir = test_ledger_dir();
    let app = app_with_key(dir, "secret-key-42");

    // Non-empty bearer token that doesn't match the configured key.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .header("authorization", "Bearer not-the-right-key")
        .body(Body::empty())
        .unwrap();

    let (status, body) = call(app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "unauthorized"); // ubs:ignore
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
// RLS cross-tenant isolation via HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rls_cross_tenant_decision_not_visible() {
    // In SQLite dev mode the X-HiveMind-Tenant header controls which tenant's
    // ledger is opened. A decision captured as tenant "alpha" must not appear
    // in a GET /v1/decisions/{id} request scoped to tenant "beta".
    let dir = test_ledger_dir();

    // Capture a decision as tenant "alpha".
    let (status, body) = call(
        app(dir.clone()),
        post_json_as_tenant(
            "/v1/decisions",
            serde_json::json!({
                "title": "Alpha-only architecture decision",
                "rationale": "Belongs to alpha only — must not cross tenant boundary",
                "topic_keys": ["isolation"],
                "options": [{ "label": "opt-a" }]
            }),
            "alpha",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "alpha capture: {body}"); // ubs:ignore
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();

    // Tenant "alpha" can retrieve its own decision.
    let (status, body) = call(
        app(dir.clone()),
        get_req_as_tenant(&format!("/v1/decisions/{decision_id}"), "alpha"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "alpha get own decision: {body}"); // ubs:ignore
    assert!(
        // ubs:ignore
        body["data"].is_object(),
        "alpha must see its own decision, got: {body}"
    );

    // Tenant "beta" cannot see alpha's decision — data must be null.
    let (status, body) = call(
        app(dir.clone()),
        get_req_as_tenant(&format!("/v1/decisions/{decision_id}"), "beta"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "beta get alpha's decision: {body}"); // ubs:ignore
    assert!(
        // ubs:ignore
        body["data"].is_null(),
        "beta must not see alpha's decision; got non-null data: {body}"
    );
}

// ---------------------------------------------------------------------------
// Not-found responses for mutation and supersession-chain endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disagree_with_nonexistent_decision_returns_404() {
    let dir = test_ledger_dir();
    let (status, body) = call(
        app(dir),
        post_json(
            "/v1/decisions/nonexistent-id/disagreements",
            serde_json::json!({ "reason": "I disagree" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
}

#[tokio::test]
async fn supersede_nonexistent_decision_returns_404() {
    let dir = test_ledger_dir();
    let (status, body) = call(
        app(dir),
        post_json(
            "/v1/decisions/nonexistent-id/supersessions",
            serde_json::json!({
                "title": "New decision",
                "rationale": "Better approach",
                "topic_keys": ["test"],
                "options": ["opt-a"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
}

#[tokio::test]
async fn supersession_chain_for_nonexistent_decision_returns_404() {
    let dir = test_ledger_dir();
    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/nonexistent-id/supersession-chain"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
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
        expressed_confidence: None,
        supersedes_id: None,
        assumes_ids: vec![],
        supports_ids: vec![],
        refutes_ids: vec![],
        actor_id: None,
        accepted_by: None,
        rejected_by: None,
        blocked_actor_id: None,
        decision_id: None,
        participants: vec![],
        session_initiator: None,
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

// ---------------------------------------------------------------------------
// MCP Streamable HTTP transport tests
// ---------------------------------------------------------------------------

fn mcp_post(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("x-hivemind-actor", "agent:test:mcp-session")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn mcp_http_initialize_returns_session_id() {
    let dir = test_ledger_dir();
    let req = mcp_post(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "clientInfo": { "name": "test", "version": "0.1" }
        }
    }));
    let response = app(dir).oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK); // ubs:ignore
                                                   // initialize must echo back a Mcp-Session-Id header
    assert!(response.headers().contains_key("mcp-session-id")); // ubs:ignore
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26"); // ubs:ignore
    assert_eq!(body["result"]["serverInfo"]["name"], "hivemind"); // ubs:ignore
}

#[tokio::test]
async fn mcp_http_tools_list_returns_12_tools() {
    let dir = test_ledger_dir();
    let (status, body) = call(
        app(dir),
        mcp_post(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK); // ubs:ignore
    let tools = body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 12); // ubs:ignore
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"capture_decision")); // ubs:ignore
    assert!(names.contains(&"get_decision")); // ubs:ignore
    assert!(names.contains(&"summarize_decisions")); // ubs:ignore
}

#[tokio::test]
async fn mcp_http_capture_and_get_decision_round_trip() {
    let dir = test_ledger_dir();

    // Capture a decision via MCP HTTP
    let (status, body) = call(
        app(dir.clone()),
        mcp_post(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "capture_decision",
                "arguments": {
                    "title": "Use axum for HTTP",
                    "rationale": "Good ergonomics and async support",
                    "topic_keys": ["http", "framework"],
                    "options": [
                        { "label": "axum", "description": "The chosen framework" },
                        { "label": "actix-web" }
                    ],
                    "chosen_option_label": "axum"
                }
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK); // ubs:ignore
    assert_eq!(body["result"]["isError"], false); // ubs:ignore
    let decision_id = body["result"]["structuredContent"]["decision_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!decision_id.is_empty()); // ubs:ignore

    // Read it back via MCP HTTP
    let (status2, body2) = call(
        app(dir),
        mcp_post(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "get_decision",
                "arguments": { "decision_id": decision_id }
            }
        })),
    )
    .await;
    assert_eq!(status2, StatusCode::OK); // ubs:ignore
    assert_eq!(body2["result"]["isError"], false); // ubs:ignore
}

#[tokio::test]
async fn mcp_http_unknown_tool_returns_tool_error() {
    let dir = test_ledger_dir();
    let (status, body) = call(
        app(dir),
        mcp_post(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "does_not_exist", "arguments": {} }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK); // ubs:ignore
                                        // Unknown tool is a protocol error → JSON-RPC error object
    assert!(body.get("error").is_some()); // ubs:ignore
}

#[tokio::test]
async fn mcp_http_oauth_metadata_stubs_respond() {
    let dir = test_ledger_dir();
    let router = app(dir);

    let (s1, b1) = call(
        router.clone(),
        get_req("/.well-known/oauth-protected-resource"),
    )
    .await;
    assert_eq!(s1, StatusCode::OK); // ubs:ignore
    assert!(b1.get("resource").is_some()); // ubs:ignore

    let (s2, b2) = call(router, get_req("/.well-known/oauth-authorization-server")).await;
    assert_eq!(s2, StatusCode::OK); // ubs:ignore
    assert!(b2.get("issuer").is_some()); // ubs:ignore
    assert!(b2.get("authorization_endpoint").is_some()); // ubs:ignore
}
