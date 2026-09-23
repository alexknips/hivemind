//! Integration tests for the Slack HTTP front door (`/v1/slack/*`):
//! signature verification, the `url_verification` handshake, event-triggered
//! capture landing in the ledger via the queue, slash commands, and
//! cross-tenant isolation.
//!
//! Uses axum's tower-service test pattern (no real TCP binding), matching
//! `tests/api.rs`. Workspace installs and tenant registration are done
//! through the CLI against the same `--hivemind-dir` the HTTP router reads,
//! proving the two transports agree — same pattern `tests/slack_app.rs`
//! uses for the CLI-only surface.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use hivemind::cli::{run, Cli};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt as _;
use uuid::Uuid;

const APP_SIGNING_SECRET: &str = "app-level-test-secret";

fn test_ledger_dir() -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-slack-http-test-{}", Uuid::new_v4()))
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
        slack_signing_secret: Some(APP_SIGNING_SECRET.to_owned()),
    };
    hivemind::api::create_router(&config)
}

/// Independent HMAC-SHA256 (RFC 2104) — deliberately NOT calling into
/// `hivemind::api`'s (private) implementation, so a passing test proves the
/// server computes the same signature an independent Slack client would.
fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut base = format!("v0:{timestamp}:").into_bytes();
    base.extend_from_slice(body);

    const BLOCK: usize = 64;
    let key = secret.as_bytes();
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(&base);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let digest = outer.finalize();
    format!(
        "v0={}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn now_ts() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}

fn signed_request(uri: &str, secret: &str, content_type: &str, body: Vec<u8>) -> Request<Body> {
    let ts = now_ts();
    let sig = sign(secret, &ts, &body);
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", content_type)
        .header("x-slack-request-timestamp", ts)
        .header("x-slack-signature", sig)
        .body(Body::from(body))
        .unwrap()
}

fn signed_json_request(uri: &str, secret: &str, body: &Value) -> Request<Body> {
    signed_request(
        uri,
        secret,
        "application/json",
        serde_json::to_vec(body).unwrap(),
    )
}

fn signed_form_request(uri: &str, secret: &str, form: &[(&str, &str)]) -> Request<Body> {
    let body = serde_urlencoded::to_string(form).unwrap().into_bytes();
    signed_request(uri, secret, "application/x-www-form-urlencoded", body)
}

async fn call(app: axum::Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = app.oneshot(req).await.expect("handler error");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body read error")
        .to_bytes();
    (status, bytes.to_vec())
}

async fn call_json(app: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let (status, bytes) = call(app, req).await;
    let body: Value = serde_json::from_slice(&bytes).expect("body is JSON");
    (status, body)
}

fn run_cli_json(hivemind_dir: &Path, args: Vec<String>) -> Value {
    let mut argv = vec![
        "hivemind".to_owned(),
        "--json".to_owned(),
        "--hivemind-dir".to_owned(),
        hivemind_dir.display().to_string(),
    ];
    argv.extend(args);
    let cli = Cli::parse_from(argv);
    let output = run(&cli).expect("cli command succeeds");
    serde_json::from_str(&output).expect("cli output is JSON")
}

fn install_workspace(hivemind_dir: &Path, team_id: &str, signing_secret: &str) {
    run_cli_json(
        hivemind_dir,
        vec!["tenant".to_owned(), "create".to_owned(), team_id.to_owned()],
    );
    run_cli_json(
        hivemind_dir,
        vec![
            "slack-app".to_owned(),
            "install".to_owned(),
            "--team-id".to_owned(),
            team_id.to_owned(),
            "--team-name".to_owned(),
            format!("Team {team_id}"),
            "--bot-token".to_owned(),
            format!("bot-{}", Uuid::new_v4()),
            "--signing-secret".to_owned(),
            signing_secret.to_owned(),
            "--hivemind-url".to_owned(),
            "http://127.0.0.1:8787".to_owned(),
            "--reaction-emoji".to_owned(),
            "hivemind".to_owned(),
        ],
    );
}

// ---------------------------------------------------------------------------
// url_verification handshake
// ---------------------------------------------------------------------------

#[tokio::test]
async fn url_verification_round_trips_the_challenge() {
    let dir = test_ledger_dir();
    let body = json!({"type": "url_verification", "token": "x", "challenge": "abc123"});
    let req = signed_json_request("/v1/slack/events", APP_SIGNING_SECRET, &body);
    let (status, bytes) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"abc123");
}

#[tokio::test]
async fn url_verification_rejects_a_bad_signature() {
    let dir = test_ledger_dir();
    let body = json!({"type": "url_verification", "token": "x", "challenge": "abc123"});
    let req = signed_json_request("/v1/slack/events", "wrong-secret", &body);
    let (status, _) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn url_verification_rejects_a_replayed_old_timestamp() {
    let dir = test_ledger_dir();
    let body = json!({"type": "url_verification", "token": "x", "challenge": "abc123"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let stale_ts = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 3600)
        .to_string();
    let sig = sign(APP_SIGNING_SECRET, &stale_ts, &body_bytes);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/slack/events")
        .header("content-type", "application/json")
        .header("x-slack-request-timestamp", stale_ts)
        .header("x-slack-signature", sig)
        .body(Body::from(body_bytes))
        .unwrap();
    let (status, _) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn events_rejects_a_body_tampered_after_signing() {
    let dir = test_ledger_dir();
    let signed_body = json!({"type": "url_verification", "token": "x", "challenge": "abc123"});
    let ts = now_ts();
    let signed_bytes = serde_json::to_vec(&signed_body).unwrap();
    let sig = sign(APP_SIGNING_SECRET, &ts, &signed_bytes);
    // Same signature/timestamp, different body — the signature was computed
    // over the original bytes and must not validate the tampered ones.
    let tampered_body =
        json!({"type": "url_verification", "token": "x", "challenge": "attacker-swapped"});
    let req = Request::builder()
        .method("POST")
        .uri("/v1/slack/events")
        .header("content-type", "application/json")
        .header("x-slack-request-timestamp", ts)
        .header("x-slack-signature", sig)
        .body(Body::from(serde_json::to_vec(&tampered_body).unwrap()))
        .unwrap();
    let (status, _) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Events round trip: app_mention marker capture lands in the ledger via the
// queue (enqueue over HTTP, drain via the CLI's tenant-scoped drain path).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn app_mention_marker_capture_enqueues_and_drains_into_the_ledger() {
    let dir = test_ledger_dir();
    let team_id = "T-EVENTS";
    let signing_secret = "events-secret";
    install_workspace(&dir, team_id, signing_secret);

    let body = json!({
        "type": "event_callback",
        "team_id": team_id,
        "event": {
            "type": "app_mention",
            "channel": "C1",
            "user": "U1",
            "ts": "1715970800.000100",
            "text": "Decision: Use the HTTP front door\nRationale: Slack cannot reach the CLI\nOptions: http,cli\nChosen: http",
        },
    });
    let req = signed_json_request("/v1/slack/events", signing_secret, &body);
    let (status, _) = call(app(dir.clone()), req).await;
    // Acknowledged immediately — the capture is queued, not written inline.
    assert_eq!(status, StatusCode::OK);

    let queue_path = dir.join("slack-app").join("queue.jsonl");
    let queued = std::fs::read_to_string(&queue_path).expect("queue file exists");
    assert!(queued.contains("\"event_mention\""));
    assert!(queued.contains("Use the HTTP front door"));

    // Drain via the CLI's existing tenant-scoped path (same `drain_queue`
    // the HTTP server's background loop's multi-tenant drain builds on;
    // this proves the queued capture is well-formed and writes cleanly).
    let drain = run_cli_json(
        &dir,
        vec![
            "--tenant".to_owned(),
            team_id.to_owned(),
            "slack-app".to_owned(),
            "drain".to_owned(),
        ],
    );
    assert_eq!(drain["processed_count"], 1);
    assert_eq!(drain["queued_after"], 0);

    let ledger = hivemind::ledger::SqliteEventLedger::open(&dir).expect("ledger opens");
    use hivemind::ledger::EventLedger as _;
    let tenant_id = hivemind::events::TenantId::new(team_id).unwrap();
    let events = ledger
        .read_for_tenant(&tenant_id, 0, 100)
        .expect("events read");
    let proposal = events
        .iter()
        .find(|event| event.event_type == hivemind::events::EventType::DecisionProposed)
        .expect("decision proposed event exists");
    assert_eq!(proposal.actor_id, format!("slack:{team_id}:U1"));
}

#[tokio::test]
async fn app_mention_without_decision_markers_acks_without_enqueueing() {
    let dir = test_ledger_dir();
    let team_id = "T-NOMARK";
    let signing_secret = "nomark-secret";
    install_workspace(&dir, team_id, signing_secret);

    let body = json!({
        "type": "event_callback",
        "team_id": team_id,
        "event": {
            "type": "app_mention",
            "channel": "C1",
            "user": "U1",
            "ts": "1715970800.000100",
            "text": "hey <@BOTID> what's up",
        },
    });
    let req = signed_json_request("/v1/slack/events", signing_secret, &body);
    let (status, _) = call(app(dir.clone()), req).await;
    assert_eq!(status, StatusCode::OK);

    let queue_path = dir.join("slack-app").join("queue.jsonl");
    assert!(!queue_path.exists(), "no markers present — nothing queued");
}

// ---------------------------------------------------------------------------
// Slash commands
// ---------------------------------------------------------------------------

#[tokio::test]
async fn slash_command_query_round_trips_over_http() {
    let dir = test_ledger_dir();
    let team_id = "T-CMD";
    let signing_secret = "cmd-secret";
    install_workspace(&dir, team_id, signing_secret);

    let req = signed_form_request(
        "/v1/slack/commands",
        signing_secret,
        &[
            ("team_id", team_id),
            ("user_id", "U1"),
            ("text", "query anything"),
        ],
    );
    let (status, body) = call_json(app(dir), req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["response_type"], "ephemeral");
    assert!(body["text"]
        .as_str()
        .unwrap()
        .contains("No HiveMind decisions matched"));
}

#[tokio::test]
async fn slash_command_bare_capture_does_not_claim_a_modal_will_open() {
    let dir = test_ledger_dir();
    let team_id = "T-MODAL";
    let signing_secret = "modal-secret";
    install_workspace(&dir, team_id, signing_secret);

    let req = signed_form_request(
        "/v1/slack/commands",
        signing_secret,
        &[("team_id", team_id), ("user_id", "U1"), ("text", "")],
    );
    let (status, body) = call_json(app(dir), req).await;
    assert_eq!(status, StatusCode::OK);
    // A synchronous slash-command HTTP response cannot open a Slack modal —
    // the reply must not claim to (see api::slack::commands_handler).
    assert!(body.get("action").is_none());
    assert!(body.get("modal").is_none());
}

// ---------------------------------------------------------------------------
// Cross-tenant safety
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_claiming_another_workspaces_team_id_fails_signature_verification() {
    let dir = test_ledger_dir();
    install_workspace(&dir, "T-A", "secret-a");
    install_workspace(&dir, "T-B", "secret-b");

    // Body claims to be from workspace B, but is signed with workspace A's
    // secret — as if an attacker who knows A's secret tried to forge a
    // request into B's tenant.
    let req = signed_form_request(
        "/v1/slack/commands",
        "secret-a",
        &[("team_id", "T-B"), ("user_id", "U1"), ("text", "query x")],
    );
    let (status, _) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unregistered_team_id_is_rejected_uniformly_as_unauthorized() {
    let dir = test_ledger_dir();
    // No install for this team at all.
    let req = signed_form_request(
        "/v1/slack/commands",
        "whatever-secret",
        &[
            ("team_id", "T-UNKNOWN"),
            ("user_id", "U1"),
            ("text", "query x"),
        ],
    );
    let (status, _) = call(app(dir), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
