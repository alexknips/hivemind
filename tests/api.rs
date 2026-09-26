//! Integration tests for the HTTP REST API.
//!
//! Uses axum's tower-service test pattern (no real TCP binding) to exercise
//! every endpoint against a real SQLite ledger in a temp directory. This
//! verifies that the same operations invoked via CLI, MCP, or HTTP produce
//! equivalent events in the ledger.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
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

fn app_with_key(hivemind_dir: PathBuf, key: &str) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: Some(key.to_owned()),
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

fn app_with_cors(hivemind_dir: PathBuf, origins: Vec<String>) -> axum::Router {
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
        cors_origins: origins,
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
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
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
                "grounding": [{"kind": "bet"}],
                "title": "Use SQLite for all storage",
                "rationale": "Simple and embeddable, no separate server to run",
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
                "grounding": [{"kind": "bet"}],
                "title": "Use bearer tokens for auth",
                "rationale": "Simple to implement and easy to revoke per session",
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
                "grounding": [{"kind": "bet"}],
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

    // Since hivemind-rkbf.1, an unregistered tenant hard-errors at
    // ledger-open time instead of silently opening a fresh scope, so both
    // tenants used below must be registered first.
    let ledger =
        hivemind::ledger::SqliteEventLedger::open(&dir).expect("open ledger for provisioning");
    for tenant in ["alpha", "beta"] {
        ledger
            .create_tenant(&hivemind::events::TenantId::new(tenant).expect("valid tenant id"))
            .expect("register test tenant");
    }

    // Capture a decision as tenant "alpha".
    let (status, body) = call(
        app(dir.clone()),
        post_json_as_tenant(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
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

    // Tenant "beta" cannot see alpha's decision — this must be indistinguishable from
    // a genuinely nonexistent id (404), not a 200 that leaks "it exists, just hidden".
    let (status, body) = call(
        app(dir.clone()),
        get_req_as_tenant(&format!("/v1/decisions/{decision_id}"), "beta"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "beta get alpha's decision: {body}" // ubs:ignore
    );
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
}

#[tokio::test]
async fn unregistered_tenant_header_is_rejected_on_read_and_write() {
    // Unlike "alpha"/"beta" above, this tenant is never registered via
    // SqliteEventLedger::create_tenant — a typo'd X-HiveMind-Tenant must not
    // silently open a fresh, empty scope (hivemind-rkbf.1).
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json_as_tenant(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Should never be captured",
                "rationale": "the tenant does not exist",
                "topic_keys": ["isolation"],
                "options": [{ "label": "opt-a" }]
            }),
            "never-registered",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "write: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
    assert!(
        // ubs:ignore
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("never-registered"),
        "error must name the tenant: {body}"
    );

    let (status, body) = call(
        app(dir.clone()),
        get_req_as_tenant("/v1/decisions/search?q=test", "never-registered"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "read: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
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
                "grounding": [{"kind": "bet"}],
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
async fn get_decision_for_nonexistent_id_returns_404() {
    let dir = test_ledger_dir();
    let (status, body) = call(app(dir), get_req("/v1/decisions/nonexistent-id")).await;
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
// Fluent (no-id) read routes: situational, recall, why, verify (hivemind-ot72.4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn situational_route_matches_before_decision_id_route() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Situational route test decision",
                "rationale": "Verifies the static /situational route wins over /:id",
                // A single alphanumeric token: topic_keys are matched verbatim
                // (lowercased) against tokenized path terms, so a hyphenated key
                // like "situational-route-test" would never match "situational"
                // or "route" as separate tokens.
                "topic_keys": ["situationalroutetest"],
                "options": [{"label": "opt"}]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();

    // If axum ever matched `/v1/decisions/{id}` first, "situational" would be treated
    // as a decision id and this would 404 with `{"error":{"code":"not_found",...}}`
    // instead of a situational-shaped envelope — that is the fact this test checks.
    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/situational?paths=situationalroutetest"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "situational: {body}"); // ubs:ignore
    assert!(body.get("result_count").is_some(), "{body}"); // ubs:ignore
    let matches = body["data"]["matches"].as_array().unwrap();
    assert!(
        matches.iter().any(|m| m["decision"]["id"] == decision_id),
        "{body}"
    ); // ubs:ignore
}

#[tokio::test]
async fn situational_requires_at_least_one_path() {
    let dir = test_ledger_dir();
    let (status, body) = call(app(dir), get_req("/v1/decisions/situational")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore
}

#[tokio::test]
async fn recall_route_returns_ranked_items_and_digest() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Recall route test: adopt widget caching",
                "rationale": "Verifies GET /v1/decisions/recall returns ranked items + digest",
                "topic_keys": ["recall-route-test"],
                "options": [{"label": "opt"}]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();

    let (status, body) = call(app(dir), get_req("/v1/decisions/recall?q=widget+caching")).await;
    assert_eq!(status, StatusCode::OK, "recall: {body}"); // ubs:ignore
    let items = body["data"]["ranked"]["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["decision"]["id"] == decision_id),
        "{body}"
    ); // ubs:ignore
    assert!(
        body["data"]["digest"]["summary"].as_str().is_some(),
        "{body}"
    ); // ubs:ignore
}

#[tokio::test]
async fn why_resolves_by_id_and_by_description() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Why route test: unique neighborhood target",
                "rationale": "Verifies GET /v1/decisions/why by id and by description",
                "topic_keys": ["why-route-test"],
                "options": [{"label": "opt"}]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();

    let (status, body) = call(
        app(dir.clone()),
        get_req(&format!("/v1/decisions/why?id={decision_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "why by id: {body}"); // ubs:ignore
    assert_eq!(body["data"]["root"]["present"], true); // ubs:ignore
    assert_eq!(body["data"]["root"]["id"], decision_id); // ubs:ignore

    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/why?description=unique+neighborhood+target"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "why by description: {body}"); // ubs:ignore
    assert_eq!(body["data"]["root"]["present"], true); // ubs:ignore
    assert_eq!(body["data"]["root"]["id"], decision_id); // ubs:ignore
}

#[tokio::test]
async fn why_returns_404_for_missing_id() {
    let dir = test_ledger_dir();
    let (status, body) = call(app(dir), get_req("/v1/decisions/why?id=nonexistent-id")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
}

#[tokio::test]
async fn why_returns_ambiguous_outcome_for_ambiguous_description() {
    let dir = test_ledger_dir();

    for title in [
        "Adopt async queue for billing",
        "Adopt async queue for notifications",
    ] {
        let (status, body) = call(
            app(dir.clone()),
            post_json(
                "/v1/decisions",
                serde_json::json!({
                    "grounding": [{"kind": "bet"}],
                    "title": title,
                    "rationale": "Fixture for why-ambiguity test",
                    "topic_keys": ["why-ambiguous-test"],
                    "options": [{"label": "opt"}]
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore
    }

    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/why?description=adopt+async+queue"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["data"]["outcome"], "ambiguous"); // ubs:ignore
    assert_eq!(body["data"]["candidates"].as_array().unwrap().len(), 2); // ubs:ignore
}

#[tokio::test]
async fn why_requires_exactly_one_of_id_or_description() {
    let dir = test_ledger_dir();

    let (status, body) = call(app(dir.clone()), get_req("/v1/decisions/why")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing both: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore

    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/why?id=some-id&description=some+text"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "both given: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore
}

#[tokio::test]
async fn verify_resolves_by_id_and_by_description() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Verify route test: unique brief target",
                "rationale": "Verifies GET /v1/decisions/verify by id and by description",
                "topic_keys": ["verify-route-test"],
                "options": [{"label": "opt"}],
                "chosen_option_label": "opt"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore
    let decision_id = body["decision_id"].as_str().unwrap().to_owned();

    let (status, body) = call(
        app(dir.clone()),
        get_req(&format!("/v1/decisions/verify?id={decision_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify by id: {body}"); // ubs:ignore
    assert_eq!(body["data"]["decision_id"], decision_id); // ubs:ignore
    assert!(
        body["data"]["still_holds"]["held_up"].as_bool().is_some(),
        "{body}"
    ); // ubs:ignore

    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/verify?description=unique+brief+target"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "verify by description: {body}"); // ubs:ignore
    assert_eq!(body["data"]["decision_id"], decision_id); // ubs:ignore
}

#[tokio::test]
async fn verify_returns_404_for_missing_id() {
    let dir = test_ledger_dir();
    let (status, body) = call(app(dir), get_req("/v1/decisions/verify?id=nonexistent-id")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "not_found"); // ubs:ignore
}

#[tokio::test]
async fn verify_returns_ambiguous_outcome_for_ambiguous_description() {
    let dir = test_ledger_dir();

    for title in [
        "Adopt async queue for billing",
        "Adopt async queue for notifications",
    ] {
        let (status, body) = call(
            app(dir.clone()),
            post_json(
                "/v1/decisions",
                serde_json::json!({
                    "grounding": [{"kind": "bet"}],
                    "title": title,
                    "rationale": "Fixture for verify-ambiguity test",
                    "topic_keys": ["verify-ambiguous-test"],
                    "options": [{"label": "opt"}]
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "capture: {body}"); // ubs:ignore
    }

    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/verify?description=adopt+async+queue"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["data"]["outcome"], "ambiguous"); // ubs:ignore
    assert_eq!(body["data"]["candidates"].as_array().unwrap().len(), 2); // ubs:ignore
}

#[tokio::test]
async fn verify_requires_exactly_one_of_id_or_description() {
    let dir = test_ledger_dir();

    let (status, body) = call(app(dir.clone()), get_req("/v1/decisions/verify")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing both: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore

    let (status, body) = call(
        app(dir),
        get_req("/v1/decisions/verify?id=some-id&description=some+text"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "both given: {body}"); // ubs:ignore
    assert_eq!(body["error"]["code"], "validation_error"); // ubs:ignore
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
        premised_on_ids: vec![],
        supports_ids: vec![],
        refutes_ids: vec![],
        actor_id: None,
        accepted_by: vec![],
        rejected_by: vec![],
        blocked_actor_id: None,
        decision_id: None,
        participants: vec![],
        session_initiator: None,
    }];

    let classified_event_id = commands
        .record_ingest_batch_classified(
            "agent:hivemind:classifier",
            &["session-x:0-4".to_owned()],
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
// Classify queue (Worker A HTTP transport, hivemind-zdsh.18)
// ---------------------------------------------------------------------------

fn ingest_json(batch_id: &str, session_id: &str, agent_tool: &str, text: &str) -> Value {
    serde_json::json!({
        "batch_id": batch_id,
        "agent_tool": agent_tool,
        "session_id": session_id,
        "turns": [
            { "turn_id": "t1", "role": "user", "text": text, "truncated": false }
        ]
    })
}

#[tokio::test]
async fn classify_queue_list_returns_pending_batches_with_turn_text() {
    let dir = test_ledger_dir();

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/ingest",
            ingest_json(
                "sess-cq-1:0-10",
                "sess-cq-1",
                "claude",
                "Should we cache query results with an LRU?",
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}"); // ubs:ignore

    let (status, body) = call(app(dir), get_req("/v1/classify-queue")).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    let batches = body["batches"].as_array().expect("batches array"); // ubs:ignore
    assert_eq!(batches.len(), 1, "{body}"); // ubs:ignore
    assert_eq!(batches[0]["batch_id"], "sess-cq-1:0-10"); // ubs:ignore
    assert_eq!(batches[0]["session_id"], "sess-cq-1"); // ubs:ignore
    assert_eq!(batches[0]["agent_tool"], "claude"); // ubs:ignore
    let batch_text = batches[0]["batch_text"].as_str().unwrap_or(""); // ubs:ignore
    assert!(
        batch_text.contains("cache query results"),
        "batch_text must carry turn text: {batch_text}"
    );
    assert_eq!(body["budget"]["classified_today"], 0); // ubs:ignore
    assert!(body["budget"]["cap"].as_u64().unwrap_or(0) > 0, "{body}"); // ubs:ignore
}

#[tokio::test]
async fn classify_queue_list_filters_by_session_id() {
    let dir = test_ledger_dir();

    for (batch_id, session_id) in [("s1:0-1", "session-a"), ("s2:0-1", "session-b")] {
        let (status, body) = call(
            app(dir.clone()),
            post_json(
                "/v1/ingest",
                ingest_json(batch_id, session_id, "claude", "note"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}"); // ubs:ignore
    }

    let (status, body) = call(app(dir), get_req("/v1/classify-queue?session_id=session-a")).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    let batches = body["batches"].as_array().unwrap(); // ubs:ignore
    assert_eq!(batches.len(), 1, "{body}"); // ubs:ignore
    assert_eq!(batches[0]["batch_id"], "s1:0-1"); // ubs:ignore
}

#[tokio::test]
async fn classify_queue_submit_covers_multiple_batches_and_decision_readable_via_why() {
    let dir = test_ledger_dir();

    for batch_id in ["multi-sess:0-1", "multi-sess:1-2"] {
        let (status, body) = call(
            app(dir.clone()),
            post_json(
                "/v1/ingest",
                ingest_json(batch_id, "multi-sess", "claude", "note"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}"); // ubs:ignore
    }

    let (status, body) = call(
        app(dir.clone()),
        get_req("/v1/classify-queue?session_id=multi-sess"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["batches"].as_array().unwrap().len(), 2, "{body}"); // ubs:ignore

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/classify-queue/submit",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "batch_ids": ["multi-sess:0-1", "multi-sess:1-2"],
                "captures": [{
                    "kind": "decision",
                    "title": "Cache query results with an LRU",
                    "rationale": "Avoids repeat queries under load",
                    "topic_keys": ["caching"],
                    "evidence_ids": [],
                    "options": ["lru-cache", "no-cache"],
                    "chosen_option": "lru-cache",
                    "extraction_confidence": 0.9
                }],
                "model": "agent:worker-a"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["capture_count"], 1, "{body}"); // ubs:ignore
    let event_id = body["event_id"].as_u64().expect("event_id"); // ubs:ignore

    // Both batches move from pending to classified together.
    let (status, body) = call(
        app(dir.clone()),
        get_req("/v1/classify-queue?session_id=multi-sess"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(
        body["batches"].as_array().unwrap().len(),
        0,
        "both batches must be drained: {body}"
    );

    // The resulting decision is readable via `why`.
    let decision_id = format!("capture:{event_id}:0");
    let (status, body) = call(
        app(dir.clone()),
        get_req(&format!("/v1/decisions/why?id={decision_id}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["data"]["root"]["present"], true, "{body}"); // ubs:ignore
    assert_eq!(body["data"]["root"]["id"], decision_id, "{body}"); // ubs:ignore

    // Full decision content round-trips too.
    let (status, body) = call(app(dir), get_req(&format!("/v1/decisions/{decision_id}"))).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(
        body["data"]["title"], "Cache query results with an LRU",
        "{body}"
    );
}

#[tokio::test]
async fn classify_queue_submit_enforces_daily_cap() {
    let dir = test_ledger_dir();

    for batch_id in ["cap-sess:0-1", "cap-sess:1-2"] {
        let (status, body) = call(
            app(dir.clone()),
            post_json(
                "/v1/ingest",
                ingest_json(batch_id, "cap-sess", "claude", "note"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}"); // ubs:ignore
    }

    // HIVEMIND_CLASSIFY_DAILY_CAP is process-global; this test is the only
    // one that overrides it, and every other test's ledger is a fresh temp
    // directory with zero classifications today, so forcing the cap to 1
    // here cannot make an unrelated concurrently-running test's single
    // submit fail (see classifier::daily_classification_cap doc comment).
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

/// Requires the `shared-backend-postgres` feature and a live Postgres
/// instance. Set HIVEMIND_TEST_POSTGRES_URL to run; skipped when unset (same
/// pattern as tests/migrate.rs). CI's dedicated `rust-postgres` job always
/// sets it, so this always runs for real there — see hivemind-zdsh.18's
/// acceptance criteria: ingest, list, and submit over HTTP against a
/// Postgres-backed server, not only SQLite dev mode.
#[cfg(feature = "shared-backend-postgres")]
mod classify_queue_postgres {
    use super::*;

    pub(super) const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

    pub(super) fn skip_if_no_postgres() -> Option<String> {
        std::env::var(TEST_DATABASE_URL_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
    }

    pub(super) fn unique_tenant(prefix: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{prefix}-{nanos}-{}", std::process::id())
    }

    fn app_postgres(database_url: &str) -> axum::Router {
        let config = hivemind::api::ApiConfig {
            hivemind_dir: test_ledger_dir(),
            bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            allow_unauthenticated_remote: false,
            api_key: None,
            database_url: Some(database_url.to_owned()),
            admin_key: Some("classify-queue-test-admin-key".to_owned()),
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

    #[tokio::test(flavor = "multi_thread")]
    async fn classify_queue_submit_covers_multiple_batches_over_postgres() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping classify-queue Postgres test; set {TEST_DATABASE_URL_ENV}");
            return;
        };
        let admin_key = "classify-queue-test-admin-key";
        let tenant_id = unique_tenant("cq-test");
        // create_router synchronously opens the r2d2/postgres pool and runs
        // schema init, which calls into the `postgres` crate's own internal
        // blocking runtime. Calling that directly from this async test body
        // panics with "Cannot start a runtime from within a runtime" — same
        // failure class as the extract_ctx bug fixed in
        // src/ledger/postgres/tests.rs (async_safety_tests). spawn_blocking
        // moves it off the Tokio worker thread, matching that fix.
        let app = tokio::task::spawn_blocking({
            let pg_url = pg_url.clone();
            move || app_postgres(&pg_url)
        })
        .await
        .expect("spawn_blocking join must not panic");

        // Provision a tenant; its token authenticates every call below —
        // Postgres mode ignores x-hivemind-actor/x-hivemind-tenant headers
        // and derives both from the bearer token (docs/SELF_HOSTING.md).
        let (status, body) = call(
            app.clone(),
            admin_post(
                "/v1/tenants",
                serde_json::json!({ "tenant_id": tenant_id, "display_name": "classify-queue test" }),
                admin_key,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "provision tenant: {body}"); // ubs:ignore
        let token = body["token_secret"]
            .as_str()
            .expect("token_secret")
            .to_owned(); // ubs:ignore

        for batch_id in ["pg-sess:0-1", "pg-sess:1-2"] {
            let (status, body) = call(
                app.clone(),
                authed_post(
                    "/v1/ingest",
                    ingest_json(batch_id, "pg-sess", "claude", "note"),
                    &token,
                ),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED, "ingest {batch_id}: {body}");
            // ubs:ignore
        }

        let (status, body) = call(
            app.clone(),
            authed_get("/v1/classify-queue?session_id=pg-sess", &token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        let batches = body["batches"].as_array().expect("batches array"); // ubs:ignore
        assert_eq!(batches.len(), 2, "{body}"); // ubs:ignore
        for batch in batches {
            assert!(
                batch["batch_text"].as_str().unwrap_or("").contains("note"),
                "{batch}"
            );
        }

        let (status, body) = call(
            app.clone(),
            authed_post(
                "/v1/classify-queue/submit",
                serde_json::json!({
                    "grounding": [{"kind": "bet"}],
                    "batch_ids": ["pg-sess:0-1", "pg-sess:1-2"],
                    "captures": [{
                        "kind": "decision",
                        "title": "Use Postgres-backed classify-queue test",
                        "rationale": "Proves the HTTP transport works against the shared backend, not only SQLite",
                        "topic_keys": ["classify-queue"],
                        "evidence_ids": [],
                        "options": ["postgres", "sqlite-only"],
                        "chosen_option": "postgres",
                        "extraction_confidence": 0.9
                    }],
                    "model": "agent:worker-a"
                }),
                &token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(body["capture_count"], 1, "{body}"); // ubs:ignore
        let event_id = body["event_id"].as_u64().expect("event_id"); // ubs:ignore

        let (status, body) = call(
            app.clone(),
            authed_get("/v1/classify-queue?session_id=pg-sess", &token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body["batches"].as_array().unwrap().len(),
            0,
            "both batches must be drained: {body}"
        );

        let decision_id = format!("capture:{event_id}:0");
        let (status, body) = call(
            app.clone(),
            authed_get(&format!("/v1/decisions/why?id={decision_id}"), &token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(body["data"]["root"]["present"], true, "{body}"); // ubs:ignore
        assert_eq!(body["data"]["root"]["id"], decision_id, "{body}"); // ubs:ignore

        let (status, body) = call(
            app.clone(),
            authed_get(&format!("/v1/decisions/{decision_id}"), &token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body["data"]["title"], "Use Postgres-backed classify-queue test",
            "{body}"
        );

        // `app` is the last live Router clone, so dropping it here would tear
        // down the r2d2/postgres connection pool synchronously: the
        // `postgres` crate's Client::drop calls block_on internally
        // (postgres-0.19.14/src/connection.rs:66), which panics with
        // "Cannot start a runtime from within a runtime" on this Tokio
        // worker thread — same failure class as the connect-time panic
        // above. spawn_blocking moves the drop off the worker thread, same
        // as the connect/query/drop pattern in
        // src/ledger/postgres/tests.rs's async_safety_tests.
        tokio::task::spawn_blocking(move || drop(app))
            .await
            .expect("dropping app must not panic");
    }
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
async fn mcp_http_tools_list_returns_18_tools() {
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
    assert_eq!(tools.len(), 29); // ubs:ignore
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"capture_decision")); // ubs:ignore
    assert!(names.contains(&"get_decision")); // ubs:ignore
    assert!(names.contains(&"classify_queue_list")); // ubs:ignore
    assert!(names.contains(&"classify_queue_submit")); // ubs:ignore
    assert!(names.contains(&"get_decision_outcome")); // ubs:ignore
    assert!(names.contains(&"get_situational_decisions")); // ubs:ignore
    assert!(names.contains(&"get_decision_neighborhood")); // ubs:ignore
    assert!(names.contains(&"decision_quality_candidates")); // ubs:ignore
    assert!(names.contains(&"get_decision_context")); // ubs:ignore
    assert!(names.contains(&"decision_context_candidates")); // ubs:ignore
    assert!(names.contains(&"score_decision")); // ubs:ignore
    assert!(names.contains(&"scan_decision_quality")); // ubs:ignore
    assert!(names.contains(&"get_suggestions")); // ubs:ignore
    assert!(names.contains(&"scan_misfiled_decisions")); // ubs:ignore
    assert!(names.contains(&"analyze_failure_modes")); // ubs:ignore
    assert!(names.contains(&"recall_decisions")); // ubs:ignore
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
                    "grounding": [{"kind": "bet"}],
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

    // Without WorkOS configured, the authorization-server endpoint indicates
    // no AS is set up (WorkOS is the AS in the WorkOS bake-off branch).
    let (s2, _b2) = call(router, get_req("/.well-known/oauth-authorization-server")).await;
    assert_eq!(s2, StatusCode::NOT_FOUND); // ubs:ignore
}

// ---------------------------------------------------------------------------
// GET /v1/graph
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_returns_shape_after_decision() {
    let dir = test_ledger_dir();

    // Capture a decision so the graph is non-empty.
    let (status, _) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Use SQLite for the hosted MVP",
                "rationale": "Single binary, zero ops",
                "topic_keys": ["persistence"],
                "options": [
                    { "label": "SQLite", "description": "Local file" },
                    { "label": "Postgres", "description": "Managed Postgres" }
                ],
                "chosen_option_label": "SQLite"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "precondition: capture failed"); // ubs:ignore

    let (gs, gb) = call(app(dir), get_req("/v1/graph")).await;
    assert_eq!(gs, StatusCode::OK, "GET /v1/graph: {gb}"); // ubs:ignore

    // Response must have the three top-level fields.
    assert!(gb.get("decisions").is_some(), "missing decisions field"); // ubs:ignore
    assert!(gb.get("nodes").is_some(), "missing nodes field"); // ubs:ignore
    assert!(gb.get("edges").is_some(), "missing edges field"); // ubs:ignore

    // At least one decision node should be present.
    let decisions = gb["decisions"].as_array().unwrap();
    assert!(!decisions.is_empty(), "expected at least one decision"); // ubs:ignore

    // Each decision must have id and title.
    let d = &decisions[0];
    assert!(d.get("id").is_some(), "decision missing id"); // ubs:ignore
    assert!(d.get("title").is_some(), "decision missing title"); // ubs:ignore

    // Every edge is an arrow from the newer node to the older one, with the relation read
    // along it (docs/GRAPH_CONTRACT.md).
    let edges = gb["edges"].as_array().unwrap();
    assert!(!edges.is_empty(), "expected edges"); // ubs:ignore
    for edge in edges {
        for field in ["from", "to", "relation", "label", "reversed"] {
            assert!(edge.get(field).is_some(), "edge missing {field}: {edge}"); // ubs:ignore
        }
    }
}

/// The `decisions` entry of a `GET /v1/graph` body for this decision id.
fn graph_decision<'a>(graph: &'a Value, decision_id: &str) -> &'a Value {
    graph["decisions"]
        .as_array()
        .expect("decisions array")
        .iter()
        .find(|d| d["id"] == decision_id)
        .unwrap_or_else(|| panic!("decision {decision_id} missing from /v1/graph: {graph}"))
}

/// Captures a decision and returns its id. `chosen` accepts it (by `decided_by`, else the
/// recording actor); without one the decision stays proposed.
async fn capture_for_graph(
    dir: &std::path::Path,
    title: &str,
    chosen: Option<&str>,
    decided_by: Option<&str>,
) -> String {
    let mut body = serde_json::json!({
        "grounding": [{"kind": "bet"}],
        "title": title,
        "rationale": "Graph standing fixture: enough words to pass validation",
        "topic_keys": ["graph-standing"],
        "options": [{ "label": "Option one" }, { "label": "Option two" }],
    });
    if let Some(chosen) = chosen {
        body["chosen_option_label"] = chosen.into();
    }
    if let Some(decided_by) = decided_by {
        body["decided_by"] = decided_by.into();
    }
    let (status, response) = call(app(dir.to_path_buf()), post_json("/v1/decisions", body)).await;
    assert_eq!(status, StatusCode::OK, "capture {title}: {response}");
    response["decision_id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn graph_decisions_carry_their_status_and_who_decided() {
    let dir = test_ledger_dir();

    // Accepted, decided by a human while an agent recorded it.
    let accepted = capture_for_graph(
        &dir,
        "Accepted by a human",
        Some("Option one"),
        Some("human:alex.knips@gmail.com"),
    )
    .await;
    // Nobody has decided yet.
    let proposed = capture_for_graph(&dir, "Still only proposed", None, None).await;
    // Accepted by the recording agent, then disagreed with by a human.
    let contested =
        capture_for_graph(&dir, "Accepted then disputed", Some("Option one"), None).await;
    let (status, body) = call(
        app(dir.clone()),
        Request::builder()
            .method("POST")
            .uri(format!("/v1/decisions/{contested}/disagreements"))
            .header("content-type", "application/json")
            .header("x-hivemind-actor", "human:dana")
            .body(Body::from(
                serde_json::json!({ "reason": "This does not survive the load numbers" })
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "disagree: {body}");
    // Accepted, then replaced.
    let superseded = capture_for_graph(&dir, "Replaced later", Some("Option two"), None).await;
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            &format!("/v1/decisions/{superseded}/supersessions"),
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "The replacement",
                "rationale": "Learned something that changes the call",
                "topic_keys": ["graph-standing"],
                "options": ["Option three"],
                "chosen_option_label": "Option three"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "supersede: {body}");
    let successor = body["new_decision_id"].as_str().unwrap().to_owned();
    let supersession = body;

    let (status, graph) = call(app(dir), get_req("/v1/graph")).await;
    assert_eq!(status, StatusCode::OK, "GET /v1/graph: {graph}");

    let human = serde_json::json!({"id": "human:alex.knips@gmail.com", "kind": "human"});
    let agent = serde_json::json!({"id": "agent:test:session-1", "kind": "agent"});
    let expected = [
        (&accepted, "accepted", serde_json::json!([human])),
        (&proposed, "proposed", serde_json::json!([])),
        (&contested, "contested", serde_json::json!([agent])),
        // Superseded wins over the acceptance, which stays on record.
        (&superseded, "superseded", serde_json::json!([agent])),
        // A superseding decision starts proposed: supersede records the call, it does not accept it.
        (&successor, "proposed", serde_json::json!([])),
    ];
    for (decision_id, status, deciders) in expected {
        let decision = graph_decision(&graph, decision_id);
        assert_eq!(decision["status"], status, "{decision_id}: {decision}");
        assert_eq!(decision["deciders"], deciders, "{decision_id}: {decision}");
    }
    // The graph says what the supersede response itself said about both ends.
    assert_eq!(
        graph_decision(&graph, &superseded)["status"],
        supersession["old_decision_status"]
    );
    assert_eq!(
        graph_decision(&graph, &successor)["status"],
        supersession["new_decision_status"]
    );
    // Every decision has both fields, whatever its state.
    for decision in graph["decisions"].as_array().unwrap() {
        assert!(decision["status"].is_string(), "no status: {decision}");
        assert!(decision["deciders"].is_array(), "no deciders: {decision}");
    }
}

#[tokio::test]
async fn graph_option_nodes_carry_a_title() {
    let dir = test_ledger_dir();
    let (status, captured) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Pick the store",
                "rationale": "Graph option title fixture: enough words to pass validation",
                "topic_keys": ["graph-standing"],
                "options": [{ "label": "SQLite" }, { "label": "Postgres" }],
                "chosen_option_label": "SQLite"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture: {captured}");
    let chosen_id = captured["chosen_option_id"].as_str().unwrap();

    let (status, graph) = call(app(dir), get_req("/v1/graph")).await;
    assert_eq!(status, StatusCode::OK, "GET /v1/graph: {graph}");

    let nodes = graph["nodes"].as_array().unwrap();
    let chosen = nodes
        .iter()
        .find(|n| n["id"] == format!("Option:{chosen_id}"))
        .expect("chosen option node");
    assert_eq!(chosen["title"], "SQLite", "{chosen}");
    let mut titles: Vec<&str> = nodes
        .iter()
        .filter(|n| n["kind"] == "Option")
        .map(|n| n["title"].as_str().expect("every Option has a title"))
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["Postgres", "SQLite"]);
    // Only options carry one; every other node keeps just its `label`.
    assert!(
        nodes
            .iter()
            .filter(|n| n["kind"] != "Option")
            .all(|n| n.get("title").is_none()),
        "title leaked onto a non-option node: {graph}"
    );
}

#[tokio::test]
async fn graph_option_recorded_without_a_label_is_titled_by_its_id() {
    use hivemind::events::{Event, EventSource, EventType, TenantId};
    use hivemind::ledger::{EventLedger, SqliteEventLedger};

    let dir = test_ledger_dir();
    // A decision recorded before options carried labels, with an id that names nothing.
    SqliteEventLedger::open(&dir)
        .unwrap()
        .append(Event {
            tenant_id: TenantId::local(),
            event_id: None,
            event_uuid: uuid::Uuid::new_v4(),
            correlation_id: None,
            causation_event_id: None,
            event_type: EventType::DecisionProposed,
            actor_id: "human:alex.knips@gmail.com".to_owned(),
            source: EventSource::Cli,
            source_ref: None,
            payload: serde_json::json!({
                "decision_id": "decision:before-labels",
                "title": "Recorded before options had labels",
                "rationale": "Historical event without option_labels",
                "topic_keys": ["legacy"],
                "option_ids": ["opt-7f3a"],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
            ts: Some(chrono::Utc::now()),
        })
        .unwrap();

    let (status, graph) = call(app(dir), get_req("/v1/graph")).await;
    assert_eq!(status, StatusCode::OK, "GET /v1/graph: {graph}");
    let option = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "Option:opt-7f3a")
        .expect("option node");
    assert_eq!(option["title"], "opt-7f3a", "{option}");
    assert!(
        option.get("label").is_none(),
        "no label was recorded: {option}"
    );
}

// ---------------------------------------------------------------------------
// CORS tests
// ---------------------------------------------------------------------------

/// Helper: send an OPTIONS preflight and return the response headers.
async fn preflight(app: axum::Router, origin: &str) -> (StatusCode, HeaderMap) {
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/v1/health")
        .header("origin", origin)
        .header("access-control-request-method", "GET")
        .header(
            "access-control-request-headers",
            "authorization,content-type",
        )
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.expect("handler error");
    let status = response.status();
    (status, response.headers().clone())
}

/// Helper: send a simple cross-origin GET and return the response headers.
async fn cross_origin_get(app: axum::Router, origin: &str) -> (StatusCode, HeaderMap) {
    let req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.expect("handler error");
    let status = response.status();
    (status, response.headers().clone())
}

#[tokio::test]
async fn cors_disabled_by_default() {
    let dir = test_ledger_dir();
    let (status, headers) = cross_origin_get(app(dir), "https://alexknips.github.io").await;
    assert_eq!(status, StatusCode::OK, "health check should succeed"); // ubs:ignore
    assert!(
        headers.get("access-control-allow-origin").is_none(),
        "no CORS headers expected when cors_origins is empty"
    ); // ubs:ignore
}

#[tokio::test]
async fn cors_preflight_allowed_origin() {
    let dir = test_ledger_dir();
    let origin = "https://alexknips.github.io";
    let (status, headers) = preflight(app_with_cors(dir, vec![origin.to_owned()]), origin).await;
    // tower-http CorsLayer returns 200 for valid preflights
    assert!(
        status.is_success(),
        "preflight from allowed origin should succeed, got {status}"
    ); // ubs:ignore
    let acao = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(acao, origin, "ACAO header must echo the allowed origin"); // ubs:ignore
}

#[tokio::test]
async fn cors_preflight_disallowed_origin_gets_no_acao() {
    let dir = test_ledger_dir();
    let (status, headers) = preflight(
        app_with_cors(dir, vec!["https://alexknips.github.io".to_owned()]),
        "https://evil.example.com",
    )
    .await;
    // Request still succeeds (CORS is about the browser; server always responds)
    assert!(status.is_success(), "preflight call itself should succeed"); // ubs:ignore
    let acao = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_ne!(
        acao, "https://evil.example.com",
        "disallowed origin must not appear in ACAO"
    ); // ubs:ignore
}

#[tokio::test]
async fn cors_actual_request_allowed_origin() {
    let dir = test_ledger_dir();
    let origin = "https://alexknips.github.io";
    let (status, headers) =
        cross_origin_get(app_with_cors(dir, vec![origin.to_owned()]), origin).await;
    assert_eq!(status, StatusCode::OK, "health check must succeed"); // ubs:ignore
    let acao = headers
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(acao, origin, "ACAO header must echo the allowed origin"); // ubs:ignore
}

#[tokio::test]
async fn cors_auth_still_enforced_on_cross_origin_request() {
    // Use app_with_key (api_key set) — cross-origin GET /v1/decisions/search
    // without a bearer token must still return 401, not 200.
    let dir = test_ledger_dir();
    let origin = "https://alexknips.github.io";
    let config = hivemind::api::ApiConfig {
        hivemind_dir: dir,
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: Some("secret".to_owned()),
        database_url: None,
        admin_key: None,
        workos_domain: None,
        workos_issuer: None,
        workos_jwks_url: None,
        workos_audience: None,
        spa_dir: None,
        cors_origins: vec![origin.to_owned()],
        slack_client_id: None,
        slack_client_secret: None,
        slack_signing_secret: None,
        slack_api_base_url: None,
    };
    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap();
    let response = hivemind::api::create_router(&config)
        .oneshot(req)
        .await
        .expect("handler error");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "auth must still be enforced on cross-origin requests"
    ); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Multi-user auth (SQLite user store)
// ---------------------------------------------------------------------------

fn app_with_admin_key(hivemind_dir: PathBuf, admin_key: &str) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: None,
        database_url: None,
        admin_key: Some(admin_key.to_owned()),
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

fn admin_post(uri: &str, body: Value, admin_key: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {admin_key}"))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

fn admin_get(uri: &str, admin_key: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {admin_key}"))
        .body(Body::empty())
        .unwrap()
}

fn authed_post(uri: &str, body: Value, token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        // intentionally different from the token-bound actor to test spoofing prevention
        .header("x-hivemind-actor", "agent:evil:spoofer")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

/// Only used by the Postgres-gated classify-queue module below; without a
/// `#[cfg]` here it would warn as dead code under default features.
#[cfg(feature = "shared-backend-postgres")]
fn authed_get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn create_user_and_authenticate() {
    let dir = test_ledger_dir();
    let admin_key = "admin-key-xyz";
    let app = app_with_admin_key(dir, admin_key);

    let (status, body) = call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({
                "email": "alice@example.com",
                "display_name": "Alice",
                "role": "member"
            }),
            admin_key,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}"); // ubs:ignore
    let token = body["token_secret"].as_str().unwrap().to_owned();
    assert!(token.starts_with("hm_tk_"), "token must have hm_tk_ prefix"); // ubs:ignore

    // Token must grant access to the API
    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::OK, "user token should grant access"); // ubs:ignore
}

#[tokio::test]
async fn actor_bound_to_token_not_header() {
    // actor_id must come from the token record, not X-HiveMind-Actor header
    let dir = test_ledger_dir();
    let admin_key = "admin-key-abc";
    let app = app_with_admin_key(dir, admin_key);

    let (_, body) = call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({ "email": "bob@example.com", "display_name": "Bob", "role": "member" }),
            admin_key,
        ),
    )
    .await;
    let token = body["token_secret"].as_str().unwrap().to_owned();

    // Capture a decision using the user token but with a spoofed X-HiveMind-Actor header
    let (status, dec) = call(
        app.clone(),
        authed_post(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Actor binding test",
                "rationale": "verifying actor comes from token",
                "topic_keys": ["auth"],
                "chosen_option_label": "a",
                "options": [{"label": "a", "description": "option a"}]
            }),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dec}"); // ubs:ignore

    // Search by the token-bound actor — should find the decision
    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=Actor+binding&actor_id=human:bob@example.com")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, body) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    let hits = body["data"]["items"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        hits > 0,
        "decision must be found by token-bound actor_id, got: {body}"
    ); // ubs:ignore

    // Spoofed actor should NOT find it
    let req2 = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=Actor+binding&actor_id=agent:evil:spoofer")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (_, body2) = call(app.clone(), req2).await;
    let hits2 = body2["data"]["items"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(
        hits2, 0,
        "spoofed actor must not find the decision, got: {body2}"
    ); // ubs:ignore
}

#[tokio::test]
async fn revoke_token_denies_access() {
    let dir = test_ledger_dir();
    let admin_key = "admin-revoke";
    let app = app_with_admin_key(dir, admin_key);

    let (_, body) = call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({ "email": "carol@example.com", "display_name": "Carol", "role": "member" }),
            admin_key,
        ),
    )
    .await;
    let token = body["token_secret"].as_str().unwrap().to_owned();
    let user_id = body["user_id"].as_str().unwrap().to_owned();
    let token_id = body["token_id"].as_str().unwrap().to_owned();

    // Revoke the token
    let revoke_req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/users/{user_id}/tokens/{token_id}"))
        .header("authorization", format!("Bearer {admin_key}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(revoke_req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "revoke must succeed"
    ); // ubs:ignore

    // Revoked token must now be rejected
    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = call(app.clone(), req).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked token must be rejected"
    ); // ubs:ignore
}

#[tokio::test]
async fn create_user_requires_admin_key() {
    let dir = test_ledger_dir();
    let app = app_with_admin_key(dir, "real-admin");

    // Wrong key
    let (status, body) = call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({ "email": "eve@example.com", "display_name": "Eve", "role": "member" }),
            "wrong-key",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}"); // ubs:ignore

    // No key at all
    let req = Request::builder()
        .method("POST")
        .uri("/v1/users")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(
                &serde_json::json!({"email":"e@e.com","display_name":"E","role":"member"}),
            )
            .unwrap(),
        ))
        .unwrap();
    let (status, _) = call(app.clone(), req).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "missing admin key must be rejected"
    ); // ubs:ignore
}

#[tokio::test]
async fn list_users_returns_created_users() {
    let dir = test_ledger_dir();
    let admin_key = "list-admin";
    let app = app_with_admin_key(dir, admin_key);

    call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({"email":"x@x.com","display_name":"X","role":"member"}),
            admin_key,
        ),
    )
    .await;
    call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({"email":"y@y.com","display_name":"Y","role":"admin"}),
            admin_key,
        ),
    )
    .await;

    let (status, body) = call(app.clone(), admin_get("/v1/users", admin_key)).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2, "should list both users"); // ubs:ignore
    assert!(
        users.iter().any(|u| u["email"] == "x@x.com"),
        "x@x.com must be listed"
    ); // ubs:ignore
    assert!(
        users.iter().any(|u| u["email"] == "y@y.com"),
        "y@y.com must be listed"
    ); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Agent-token minting (hivemind-zdsh.19): an admin can mint a token bound
// directly to an agent identity (agent:<tool>:<name>), distinct from
// create_user's human:<email> shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_agent_token_writes_show_agent_actor() {
    let dir = test_ledger_dir();
    let admin_key = "agent-token-admin";
    let app = app_with_admin_key(dir, admin_key);

    let (status, body) = call(
        app.clone(),
        admin_post(
            "/v1/agent-tokens",
            serde_json::json!({"agent_tool": "claude", "agent_name": "gastown-crew"}),
            admin_key,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}"); // ubs:ignore
    assert_eq!(body["actor_id"], "agent:claude:gastown-crew"); // ubs:ignore
    let token = body["token_secret"].as_str().unwrap().to_owned();
    assert!(token.starts_with("hm_tk_"), "token must have hm_tk_ prefix"); // ubs:ignore

    // A decision captured with this token — even with a spoofed
    // X-HiveMind-Actor header — must be credited to the minted agent
    // identity, never the header and never a human:<email> shape.
    let (status, dec) = call(
        app.clone(),
        authed_post(
            "/v1/decisions",
            serde_json::json!({
                "grounding": [{"kind": "bet"}],
                "title": "Agent token actor test",
                "rationale": "verifying agent tokens read as agents",
                "topic_keys": ["auth"],
                "chosen_option_label": "a",
                "options": [{"label": "a", "description": "option a"}]
            }),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dec}"); // ubs:ignore

    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=Agent+token&actor_id=agent:claude:gastown-crew")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, body) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    let hits = body["data"]["items"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        hits > 0,
        "decision must be found under the minted agent actor, got: {body}"
    ); // ubs:ignore

    // Neither the spoofed header actor nor a human:<email> shape should have
    // been recorded.
    let spoofed_req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=Agent+token&actor_id=agent:evil:spoofer")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (_, spoofed_body) = call(app.clone(), spoofed_req).await;
    let spoofed_hits = spoofed_body["data"]["items"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(
        spoofed_hits, 0,
        "spoofed actor must not find the decision, got: {spoofed_body}"
    ); // ubs:ignore
}

#[tokio::test]
async fn mint_agent_token_requires_admin_key() {
    let dir = test_ledger_dir();
    let app = app_with_admin_key(dir, "real-admin");

    let (status, body) = call(
        app.clone(),
        admin_post(
            "/v1/agent-tokens",
            serde_json::json!({"agent_tool": "claude", "agent_name": "gastown-crew"}),
            "wrong-key",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}"); // ubs:ignore
}

#[tokio::test]
async fn mint_agent_token_rejects_empty_tool_or_name() {
    let dir = test_ledger_dir();
    let admin_key = "agent-token-admin-2";
    let app = app_with_admin_key(dir, admin_key);

    let (status, body) = call(
        app.clone(),
        admin_post(
            "/v1/agent-tokens",
            serde_json::json!({"agent_tool": "", "agent_name": "gastown-crew"}),
            admin_key,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore

    let (status, body) = call(
        app.clone(),
        admin_post(
            "/v1/agent-tokens",
            serde_json::json!({"agent_tool": "claude", "agent_name": ""}),
            admin_key,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
}

#[tokio::test]
async fn mint_agent_token_can_be_revoked() {
    let dir = test_ledger_dir();
    let admin_key = "agent-token-admin-3";
    let app = app_with_admin_key(dir, admin_key);

    let (_, body) = call(
        app.clone(),
        admin_post(
            "/v1/agent-tokens",
            serde_json::json!({"agent_tool": "codex", "agent_name": "gastown-polecat"}),
            admin_key,
        ),
    )
    .await;
    let token = body["token_secret"].as_str().unwrap().to_owned();
    let token_id = body["token_id"].as_str().unwrap().to_owned();

    // Agent tokens have no associated user_id; revoke via a nil user_id path
    // component (the handler validates token_id/tenant_id, not user_id).
    let revoke_req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/users/{}/tokens/{token_id}", uuid::Uuid::nil()))
        .header("authorization", format!("Bearer {admin_key}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(revoke_req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "revoke must succeed for an agent token"
    ); // ubs:ignore

    let req = Request::builder()
        .method("GET")
        .uri("/v1/decisions/search?q=test")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = call(app.clone(), req).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revoked agent token must be rejected"
    ); // ubs:ignore
}

// ---------------------------------------------------------------------------
// Grounded capture (hivemind-gwhr.2): POST /v1/decisions and /supersessions ask
// "what does this decision rest on?" exactly like the MCP tools.
// ---------------------------------------------------------------------------

fn capture_body(title: &str, grounding: Value) -> Value {
    serde_json::json!({
        "title": title,
        "rationale": "Rationale text long enough for the readable floor and then some",
        "topic_keys": ["grounding"],
        "options": [{ "label": "adopt" }],
        "grounding": grounding,
    })
}

async fn ledger_offset_of(dir: &PathBuf) -> i64 {
    use hivemind::ledger::{EventLedger, SqliteEventLedger};
    let ledger = SqliteEventLedger::open(dir).unwrap();
    ledger.latest_offset().unwrap() as i64
}

#[tokio::test]
async fn capture_without_grounding_is_a_validation_error_and_writes_nothing() {
    let dir = test_ledger_dir();

    for grounding in [serde_json::json!([]), Value::Null] {
        let (status, body) = call(
            app(dir.clone()),
            post_json(
                "/v1/decisions",
                capture_body("Adopt the new queue", grounding),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
        assert!(
            body.to_string()
                .contains("a captured decision must say what it rests on"),
            "{body}"
        ); // ubs:ignore
    }
    // Malformed items are named by their index.
    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            capture_body(
                "Adopt the new queue",
                serde_json::json!([{ "kind": "opinion" }]),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
    assert!(body.to_string().contains("grounding[0].kind"), "{body}"); // ubs:ignore
}

#[tokio::test]
async fn capture_records_grounding_and_replies_with_rests_on() {
    let dir = test_ledger_dir();
    let (_, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            capture_body(
                "Keep the ledger append-only",
                serde_json::json!([{ "kind": "bet" }]),
            ),
        ),
    )
    .await;
    let goal_id = body["decision_id"].as_str().unwrap().to_owned();

    let mut request = capture_body(
        "Adopt the new queue",
        serde_json::json!([
            { "kind": "decision", "description": "Keep the ledger append-only" },
            { "kind": "evidence", "content": "p95 was 180ms in run 42", "source": "ci run 42" },
            { "kind": "assumption", "statement": "traffic stays under 1k rps" },
            { "kind": "bet", "would_change_if": "they raise prices" },
        ]),
    );
    request["expressed_confidence"] = serde_json::json!("high");
    let (status, body) = call(app(dir.clone()), post_json("/v1/decisions", request)).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore

    let rests_on = body["rests_on"].as_array().unwrap();
    let kinds: Vec<&str> = rests_on
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["decision", "evidence", "assumption", "bet"]); // ubs:ignore
    assert_eq!(rests_on[0]["id"], goal_id); // ubs:ignore
    assert_eq!(rests_on[0]["label"], "Keep the ledger append-only"); // ubs:ignore
    assert_eq!(body["premise_stale"], serde_json::json!([])); // ubs:ignore

    // The deprecated id aliases alone still satisfy the requirement.
    let (_, evidence) = call(
        app(dir.clone()),
        post_json(
            "/v1/evidence",
            serde_json::json!({ "content": "an existing observation" }),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_owned();
    let mut aliased = capture_body("Adopt the other queue", Value::Null);
    aliased["evidence_ids"] = serde_json::json!([evidence_id]);
    let (status, body) = call(app(dir.clone()), post_json("/v1/decisions", aliased)).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["rests_on"][0]["kind"], "evidence"); // ubs:ignore
    assert_eq!(body["rests_on"][0]["id"], evidence_id); // ubs:ignore
}

#[tokio::test]
async fn capture_with_an_unresolved_premise_returns_data_and_writes_nothing() {
    let dir = test_ledger_dir();
    for _ in 0..2 {
        call(
            app(dir.clone()),
            post_json(
                "/v1/decisions",
                capture_body("Adopt the queue", serde_json::json!([{ "kind": "bet" }])),
            ),
        )
        .await;
    }
    let offset_before = ledger_offset_of(&dir).await;

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            capture_body(
                "Ship the queue",
                serde_json::json!([
                    { "kind": "assumption", "statement": "must not be stranded" },
                    { "kind": "decision", "description": "Adopt the queue" },
                ]),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["data"]["outcome"], "ambiguous"); // ubs:ignore
    assert_eq!(body["data"]["field"], "grounding[1]"); // ubs:ignore
    assert_eq!(body["data"]["candidates"].as_array().map(Vec::len), Some(2)); // ubs:ignore

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            capture_body(
                "Ship the queue",
                serde_json::json!([{ "kind": "decision", "description": "quantum flux capacitor" }]),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["data"]["outcome"], "not_found"); // ubs:ignore
    assert_eq!(body["data"]["description"], "quantum flux capacitor"); // ubs:ignore

    assert_eq!(ledger_offset_of(&dir).await, offset_before); // ubs:ignore
}

#[tokio::test]
async fn supersession_requires_and_records_grounding() {
    let dir = test_ledger_dir();
    let (_, body) = call(
        app(dir.clone()),
        post_json(
            "/v1/decisions",
            capture_body(
                "Use shared admin token",
                serde_json::json!([{ "kind": "bet" }]),
            ),
        ),
    )
    .await;
    let old_id = body["decision_id"].as_str().unwrap().to_owned();
    let offset_before = ledger_offset_of(&dir).await;

    let supersession = |grounding: Value| {
        serde_json::json!({
            "title": "Use scoped service tokens",
            "rationale": "Scoped tokens preserve audit boundaries for every caller",
            "options": ["scoped-tokens"],
            "grounding": grounding,
        })
    };
    let uri = format!("/v1/decisions/{old_id}/supersessions");

    let (status, body) = call(
        app(dir.clone()),
        post_json(&uri, supersession(serde_json::json!([]))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); // ubs:ignore
    assert_eq!(ledger_offset_of(&dir).await, offset_before); // ubs:ignore

    let (status, body) = call(
        app(dir.clone()),
        post_json(
            &uri,
            supersession(serde_json::json!([
                { "kind": "evidence", "content": "the shared token leaked twice", "source": "incident 7" }
            ])),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(body["old_decision_status"], "superseded"); // ubs:ignore
    assert_eq!(body["rests_on"][0]["kind"], "evidence"); // ubs:ignore
    assert_eq!(
        body["rests_on"][0]["label"],
        "the shared token leaked twice"
    ); // ubs:ignore
}

// ---------------------------------------------------------------------------
// GET /v1/whoami (hivemind-jro0): the identity a bearer credential carries — what
// the UI's token sign-in shows as "Signed in as ...".
// ---------------------------------------------------------------------------

/// `GET /v1/whoami`, with `Authorization: Bearer <token>` when a token is given (`Some("")` is
/// the empty token). It sends no `X-HiveMind-Actor` header: the shared-key path honours one.
fn whoami_req(token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri("/v1/whoami");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

async fn whoami(app: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    call(app.clone(), whoami_req(token)).await
}

/// SQLite server with a shared key AND an admin key: the shared key, per-user tokens and agent
/// tokens are all live, and a request with no token is refused (a server with per-user tokens
/// alone stays open to token-less requests).
fn app_with_shared_and_admin_keys(
    hivemind_dir: PathBuf,
    shared_key: &str,
    admin_key: &str,
) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: Some(shared_key.to_owned()),
        database_url: None,
        admin_key: Some(admin_key.to_owned()),
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

/// A server that trusts the WorkOS stand-in at `workos_jwks_url` (when given), on Postgres when
/// `database_url` is given. `create_router` fetches the JWKS with a blocking client, so call it
/// outside any Tokio runtime or from `spawn_blocking`.
fn app_with_auth(
    hivemind_dir: PathBuf,
    database_url: Option<&str>,
    admin_key: Option<&str>,
    workos_jwks_url: Option<&str>,
) -> axum::Router {
    let workos_domain = workos_jwks_url.map(|_| workos_double::DOMAIN.to_owned());
    let config = hivemind::api::ApiConfig {
        hivemind_dir,
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: None,
        database_url: database_url.map(str::to_owned),
        admin_key: admin_key.map(str::to_owned),
        workos_domain: workos_domain.clone(),
        workos_issuer: workos_domain,
        workos_jwks_url: workos_jwks_url.map(str::to_owned),
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

/// Stands in for WorkOS: a JWKS endpoint on a local port publishing one ES256 key, and JWTs
/// signed by it (plus JWTs signed by a key it does not publish).
mod workos_double {
    use std::io::{Read as _, Write as _};

    use base64::Engine as _;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair as _, ECDSA_P256_SHA256_FIXED_SIGNING};
    use serde_json::{json, Value};

    /// The issuer the API is configured to trust (`WORKOS_DOMAIN`).
    pub const DOMAIN: &str = "https://whoami-test.authkit.app";
    const KID: &str = "whoami-test-key";

    pub struct Double {
        pub jwks_url: String,
        signing_key: Vec<u8>,
    }

    /// A fresh P-256 key: its PKCS#8 signing form and its uncompressed public point.
    fn new_key() -> (Vec<u8>, Vec<u8>) {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate P-256 key");
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .expect("load P-256 key");
        (pkcs8.as_ref().to_vec(), pair.public_key().as_ref().to_vec())
    }

    /// Starts the JWKS endpoint. Its thread serves until the test process exits.
    pub fn start() -> Double {
        let (signing_key, public_point) = new_key();
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        // Uncompressed point: 0x04, then x (32 bytes), then y (32 bytes).
        let jwks = json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": KID,
            "x": b64(&public_point[1..33]),
            "y": b64(&public_point[33..65]),
        }] })
        .to_string();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind JWKS stand-in");
        let jwks_url = format!(
            "http://{}/jwks",
            listener.local_addr().expect("JWKS stand-in address")
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                // Read the request head so closing the socket cannot reset the client mid-read.
                let _ = stream.read(&mut [0u8; 4096]);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{jwks}",
                    jwks.len()
                );
            }
        });
        Double {
            jwks_url,
            signing_key,
        }
    }

    impl Double {
        /// The access token WorkOS would issue for `claims` (`sub`, `email`, `org_id`).
        pub fn token(&self, claims: Value) -> String {
            sign(&self.signing_key, claims)
        }
    }

    /// A token with the trusted `kid` and issuer, signed by a key the JWKS does not publish.
    pub fn forged_token(claims: Value) -> String {
        sign(&new_key().0, claims)
    }

    fn sign(pkcs8: &[u8], mut claims: Value) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after the epoch")
            .as_secs();
        claims["iss"] = json!(DOMAIN);
        claims["exp"] = json!(now + 3600);
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(KID.to_owned());
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_ec_der(pkcs8),
        )
        .expect("sign test JWT")
    }
}

#[tokio::test]
async fn whoami_names_the_identity_each_credential_carries() {
    let dir = test_ledger_dir();
    let app = app_with_shared_and_admin_keys(dir.clone(), "shared-key", "admin-key");
    let offset_before = ledger_offset_of(&dir).await;

    // The shared key is no person: writes made with it are `service:api`.
    let (status, body) = whoami(&app, Some("shared-key")).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(
        body,
        serde_json::json!({ "actor_id": "service:api", "tenant_id": "local" })
    ); // ubs:ignore

    // A user token names its person, whatever the caller claims in X-HiveMind-Actor.
    let (status, user) = call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({ "email": "alice@example.com", "display_name": "Alice", "role": "member" }),
            "admin-key",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{user}"); // ubs:ignore
    let mut request = whoami_req(user["token_secret"].as_str());
    request
        .headers_mut()
        .insert("x-hivemind-actor", "agent:evil:spoofer".parse().unwrap());
    let (status, body) = call(app.clone(), request).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(
        body,
        serde_json::json!({ "actor_id": "human:alice@example.com", "tenant_id": "local" })
    ); // ubs:ignore

    // An agent token names its agent.
    let (status, agent) = call(
        app.clone(),
        admin_post(
            "/v1/agent-tokens",
            serde_json::json!({ "agent_tool": "claude", "agent_name": "whoami-test" }),
            "admin-key",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}"); // ubs:ignore
    let (status, body) = whoami(&app, agent["token_secret"].as_str()).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
    assert_eq!(
        body,
        serde_json::json!({ "actor_id": "agent:claude:whoami-test", "tenant_id": "local" })
    ); // ubs:ignore

    // Asking who you are writes nothing.
    assert_eq!(ledger_offset_of(&dir).await, offset_before); // ubs:ignore
}

#[tokio::test]
async fn whoami_refuses_missing_empty_unknown_and_revoked_tokens() {
    let app = app_with_shared_and_admin_keys(test_ledger_dir(), "shared-key", "admin-key");
    let (_, user) = call(
        app.clone(),
        admin_post(
            "/v1/users",
            serde_json::json!({ "email": "bob@example.com", "display_name": "Bob", "role": "member" }),
            "admin-key",
        ),
    )
    .await;
    let token = user["token_secret"].as_str().unwrap().to_owned();

    // Control: the token works until it is revoked.
    let (status, body) = whoami(&app, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore

    let revoke = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/v1/users/{}/tokens/{}",
            user["user_id"].as_str().unwrap(),
            user["token_id"].as_str().unwrap()
        ))
        .header("authorization", "Bearer admin-key")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(revoke).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT); // ubs:ignore

    let unknown_user_token = format!("hm_tk_{}", "0".repeat(64));
    for (what, credential) in [
        ("no token", None),
        ("empty token", Some("")),
        ("wrong shared key", Some("not-the-shared-key")),
        ("unknown user token", Some(unknown_user_token.as_str())),
        ("revoked user token", Some(token.as_str())),
    ] {
        let (status, body) = whoami(&app, credential).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}: {body}"); // ubs:ignore
        assert_eq!(body["error"]["code"], "unauthorized", "{what}: {body}"); // ubs:ignore
    }
}

// The SQLite WorkOS path only exists in builds without shared-backend-postgres; the Postgres
// build's WorkOS path is covered by whoami_postgres below.
#[cfg(not(feature = "shared-backend-postgres"))]
#[test]
fn whoami_names_a_workos_signed_in_person() {
    let double = workos_double::start();
    // create_router fetches the JWKS with a blocking client, which cannot start inside a runtime.
    let app = app_with_auth(test_ledger_dir(), None, None, Some(&double.jwks_url));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let claims =
            || serde_json::json!({ "sub": "user_01H8", "email": "sam@example.com", "org_id": "org_acme" });

        let (status, body) = whoami(&app, Some(&double.token(claims()))).await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body,
            serde_json::json!({ "actor_id": "human:sam@example.com", "tenant_id": "org_acme" })
        ); // ubs:ignore

        let (status, body) = whoami(&app, Some(&workos_double::forged_token(claims()))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "forged token: {body}"); // ubs:ignore
    });
}

#[cfg(feature = "shared-backend-postgres")]
mod whoami_postgres {
    use super::classify_queue_postgres::{
        skip_if_no_postgres, unique_tenant, TEST_DATABASE_URL_ENV,
    };
    use super::*;

    const ADMIN_KEY: &str = "whoami-test-admin-key";

    /// `create_router` opens the r2d2/postgres pool (and fetches the JWKS with a blocking
    /// client), so it runs off the Tokio worker — see classify_queue_postgres.
    async fn app_postgres(pg_url: &str, workos_jwks_url: Option<&str>) -> axum::Router {
        let pg_url = pg_url.to_owned();
        let workos_jwks_url = workos_jwks_url.map(str::to_owned);
        tokio::task::spawn_blocking(move || {
            app_with_auth(
                test_ledger_dir(),
                Some(&pg_url),
                Some(ADMIN_KEY),
                workos_jwks_url.as_deref(),
            )
        })
        .await
        .expect("spawn_blocking join must not panic")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn whoami_over_postgres_names_each_credential_and_refuses_the_rest() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping whoami Postgres test; set {TEST_DATABASE_URL_ENV}");
            return;
        };
        let app = app_postgres(&pg_url, None).await;
        let tenant_id = unique_tenant("whoami-test");

        // A tenant's provisioning token is the Postgres analogue of the shared key.
        let (status, tenant) = call(
            app.clone(),
            admin_post(
                "/v1/tenants",
                serde_json::json!({ "tenant_id": tenant_id, "display_name": "whoami test" }),
                ADMIN_KEY,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{tenant}"); // ubs:ignore
        let (status, body) = whoami(&app, tenant["token_secret"].as_str()).await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body,
            serde_json::json!({ "actor_id": "service:api", "tenant_id": tenant_id })
        ); // ubs:ignore

        let email = format!("{tenant_id}@example.com");
        let (status, user) = call(
            app.clone(),
            admin_post(
                "/v1/users",
                serde_json::json!({
                    "email": email, "display_name": "Whoami", "role": "member", "tenant_id": tenant_id
                }),
                ADMIN_KEY,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{user}"); // ubs:ignore
        let user_token = user["token_secret"].as_str().unwrap().to_owned();
        let (status, body) = whoami(&app, Some(&user_token)).await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body,
            serde_json::json!({ "actor_id": format!("human:{email}"), "tenant_id": tenant_id })
        ); // ubs:ignore

        let (status, agent) = call(
            app.clone(),
            admin_post(
                "/v1/agent-tokens",
                serde_json::json!({
                    "agent_tool": "claude", "agent_name": "whoami-test", "tenant_id": tenant_id
                }),
                ADMIN_KEY,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{agent}"); // ubs:ignore
        let (status, body) = whoami(&app, agent["token_secret"].as_str()).await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body,
            serde_json::json!({ "actor_id": "agent:claude:whoami-test", "tenant_id": tenant_id })
        ); // ubs:ignore

        let revoke = Request::builder()
            .method("DELETE")
            .uri(format!(
                "/v1/users/{}/tokens/{}?tenant_id={tenant_id}",
                user["user_id"].as_str().unwrap(),
                user["token_id"].as_str().unwrap()
            ))
            .header("authorization", format!("Bearer {ADMIN_KEY}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(revoke).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT); // ubs:ignore

        let unknown_user_token = format!("hm_tk_{}", "0".repeat(64));
        for (what, credential) in [
            ("no token", None),
            ("empty token", Some("")),
            ("unknown token", Some(unknown_user_token.as_str())),
            ("revoked user token", Some(user_token.as_str())),
        ] {
            let (status, body) = whoami(&app, credential).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}: {body}"); // ubs:ignore
            assert_eq!(body["error"]["code"], "unauthorized", "{what}: {body}");
            // ubs:ignore
        }

        // Dropping the last Router clone tears the pool down synchronously — see
        // classify_queue_postgres.
        tokio::task::spawn_blocking(move || drop(app))
            .await
            .expect("dropping app must not panic");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn whoami_over_postgres_names_a_workos_signed_in_person() {
        let Some(pg_url) = skip_if_no_postgres() else {
            eprintln!("skipping whoami Postgres test; set {TEST_DATABASE_URL_ENV}");
            return;
        };
        let double = workos_double::start();
        let app = app_postgres(&pg_url, Some(&double.jwks_url)).await;
        // The person's tenant is `oidc:<sub>`, created on first sight.
        let sub = unique_tenant("user");
        let claims = || serde_json::json!({ "sub": sub, "email": "sam@example.com" });

        let (status, body) = whoami(&app, Some(&double.token(claims()))).await;
        assert_eq!(status, StatusCode::OK, "{body}"); // ubs:ignore
        assert_eq!(
            body,
            serde_json::json!({ "actor_id": "human:sam@example.com", "tenant_id": format!("oidc:{sub}") })
        ); // ubs:ignore

        // With WorkOS configured a WorkOS JWT is the only credential.
        let opaque_token = format!("hm_tk_{}", "0".repeat(64));
        let forged = workos_double::forged_token(claims());
        for (what, credential) in [
            ("no token", None),
            ("opaque token", Some(opaque_token.as_str())),
            ("forged JWT", Some(forged.as_str())),
        ] {
            let (status, body) = whoami(&app, credential).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}: {body}"); // ubs:ignore
        }

        tokio::task::spawn_blocking(move || drop(app))
            .await
            .expect("dropping app must not panic");
    }
}
