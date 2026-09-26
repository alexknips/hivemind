//! Integration tests for the Slack HTTP front door (`/v1/slack/*`):
//! signature verification, the `url_verification` handshake, event-triggered
//! capture landing in the ledger via the queue, slash commands, the
//! interactivity endpoint (message shortcut + capture modal), reaction
//! capture, and cross-tenant isolation.
//!
//! Uses axum's tower-service test pattern (no real TCP binding) for the
//! front door itself, matching `tests/api.rs`. The two flows that call Slack
//! (`views.open`, `conversations.history`) are pointed, through
//! `ApiConfig::slack_api_base_url`, at a stand-in Slack served on a loopback
//! port that records what it was sent. Workspace installs and tenant
//! registration are done through the CLI against the same `--hivemind-dir`
//! the HTTP router reads, proving the two transports agree — same pattern
//! `tests/slack_app.rs` uses for the CLI-only surface.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, Uri};
use axum::Json;
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
    hivemind::api::create_router(&test_config(hivemind_dir))
}

/// The front door with its outbound Slack Web API calls pointed at `mock`.
fn app_with_slack_api(hivemind_dir: PathBuf, mock: &MockSlack) -> axum::Router {
    let mut config = test_config(hivemind_dir);
    config.slack_api_base_url = Some(mock.base_url.clone());
    hivemind::api::create_router(&config)
}

fn test_config(hivemind_dir: PathBuf) -> hivemind::api::ApiConfig {
    hivemind::api::ApiConfig {
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
        slack_api_base_url: None,
    }
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
    install_workspace_with_bot_token(
        hivemind_dir,
        team_id,
        signing_secret,
        &format!("bot-{}", Uuid::new_v4()),
    );
}

fn install_workspace_with_bot_token(
    hivemind_dir: &Path,
    team_id: &str,
    signing_secret: &str,
    bot_token: &str,
) {
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
            bot_token.to_owned(),
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
    // A slash command names no message to attach a capture to, so the reply
    // must not claim a modal — it points at the surfaces that do capture
    // (see api::slack::commands_handler).
    assert!(body.get("action").is_none());
    assert!(body.get("modal").is_none());
    let text = body["text"].as_str().expect("text reply");
    assert!(text.contains("Capture this thread as a decision"));
    assert!(text.contains(":hivemind:"), "names the install's own emoji");
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

// ---------------------------------------------------------------------------
// A stand-in Slack Web API for the flows that call back into Slack.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RecordedCall {
    path: String,
    authorization: Option<String>,
    body: String,
}

struct MockSlack {
    base_url: String,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
}

impl MockSlack {
    fn calls_to(&self, path: &str) -> Vec<RecordedCall> {
        self.calls
            .lock()
            .expect("mock slack lock")
            .iter()
            .filter(|call| call.path == path)
            .cloned()
            .collect()
    }

    fn call_count(&self) -> usize {
        self.calls.lock().expect("mock slack lock").len()
    }
}

#[derive(Clone)]
struct MockSlackState {
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    replies: Arc<Vec<(String, StatusCode, Value)>>,
}

async fn mock_slack_handler(
    State(state): State<MockSlackState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    state
        .calls
        .lock()
        .expect("mock slack lock")
        .push(RecordedCall {
            path: uri.path().to_owned(),
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    match state.replies.iter().find(|(path, _, _)| path == uri.path()) {
        Some((_, status, reply)) => (*status, Json(reply.clone())),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"ok": false, "error": "unknown_method"})),
        ),
    }
}

/// Serves `replies` (`/method` -> status + JSON answer) on a loopback port and
/// records every request it receives. A method with no reply answers
/// `unknown_method`.
async fn mock_slack(replies: Vec<(&str, StatusCode, Value)>) -> MockSlack {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = MockSlackState {
        calls: Arc::clone(&calls),
        replies: Arc::new(
            replies
                .into_iter()
                .map(|(path, status, reply)| (path.to_owned(), status, reply))
                .collect(),
        ),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock slack");
    let addr = listener.local_addr().expect("mock slack address");
    let router = axum::Router::new()
        .fallback(mock_slack_handler)
        .with_state(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    MockSlack {
        base_url: format!("http://{addr}"),
        calls,
    }
}

fn ok_reply() -> Value {
    json!({"ok": true})
}

fn queued_captures(hivemind_dir: &Path) -> Vec<Value> {
    let Ok(queued) = std::fs::read_to_string(hivemind_dir.join("slack-app").join("queue.jsonl"))
    else {
        return Vec::new();
    };
    queued
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("queue line is JSON"))
        .collect()
}

/// Drains the workspace's queue through the CLI's tenant-scoped path and
/// returns `(actor_id, source_ref)` of the one decision it wrote.
fn drain_one_decision(hivemind_dir: &Path, team_id: &str) -> (String, Option<String>) {
    let drain = run_cli_json(
        hivemind_dir,
        vec![
            "--tenant".to_owned(),
            team_id.to_owned(),
            "slack-app".to_owned(),
            "drain".to_owned(),
        ],
    );
    assert_eq!(drain["processed_count"], 1);
    assert_eq!(drain["queued_after"], 0);

    use hivemind::ledger::EventLedger as _;
    let ledger = hivemind::ledger::SqliteEventLedger::open(hivemind_dir).expect("ledger opens");
    let tenant_id = hivemind::events::TenantId::new(team_id).expect("tenant id");
    let events = ledger
        .read_for_tenant(&tenant_id, 0, 100)
        .expect("events read");
    let proposal = events
        .iter()
        .find(|event| event.event_type == hivemind::events::EventType::DecisionProposed)
        .expect("decision proposed event exists");
    (proposal.actor_id.clone(), proposal.source_ref.clone())
}

// ---------------------------------------------------------------------------
// Interactivity: message shortcut -> capture modal -> queued capture
// ---------------------------------------------------------------------------

fn signed_interactivity_request(secret: &str, payload: &Value) -> Request<Body> {
    let payload = payload.to_string();
    signed_form_request(
        "/v1/slack/interactivity",
        secret,
        &[("payload", payload.as_str())],
    )
}

fn message_action_payload(team_id: &str, callback_id: &str) -> Value {
    json!({
        "type": "message_action",
        "callback_id": callback_id,
        "trigger_id": "trigger-123.456",
        "team": {"id": team_id, "domain": "example"},
        "user": {"id": "U-INVOKER", "name": "ada"},
        "channel": {"id": "C1", "name": "eng"},
        "message_ts": "1715970801.000200",
        "message": {
            "type": "message",
            "user": "U-AUTHOR",
            "ts": "1715970801.000200",
            "thread_ts": "1715970800.000100",
            "text": "we should move to Postgres"
        }
    })
}

fn view_submission_payload(
    team_id: &str,
    user_id: &str,
    private_metadata: &str,
    inputs: &[(&str, &str)],
) -> Value {
    let values: serde_json::Map<String, Value> = inputs
        .iter()
        .map(|(block_id, text)| {
            (
                (*block_id).to_owned(),
                json!({"value": {"type": "plain_text_input", "value": text}}),
            )
        })
        .collect();
    json!({
        "type": "view_submission",
        "team": {"id": team_id, "domain": "example"},
        "user": {"id": user_id, "name": "grace"},
        "view": {
            "id": "V1",
            "callback_id": "hivemind_capture_decision",
            "private_metadata": private_metadata,
            "state": {"values": values}
        }
    })
}

/// Invokes the shortcut and returns the `private_metadata` Slack would hand
/// back on submission — read from the `views.open` call the front door made.
async fn open_modal_and_take_private_metadata(
    router: &axum::Router,
    mock: &MockSlack,
    team_id: &str,
    signing_secret: &str,
) -> String {
    let req = signed_interactivity_request(
        signing_secret,
        &message_action_payload(team_id, "hivemind_capture_thread"),
    );
    let (status, bytes) = call(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(bytes.is_empty(), "an empty 200 acknowledges the shortcut");

    let opened = mock.calls_to("/views.open");
    assert_eq!(opened.len(), 1, "exactly one views.open call");
    let body: Value = serde_json::from_str(&opened[0].body).expect("views.open body is JSON");
    body["view"]["private_metadata"]
        .as_str()
        .expect("private_metadata is a string")
        .to_owned()
}

#[tokio::test]
async fn message_shortcut_opens_the_capture_modal_with_the_messages_context() {
    let dir = test_ledger_dir();
    let (team_id, signing_secret) = ("T-SHORTCUT", "shortcut-secret");
    install_workspace_with_bot_token(&dir, team_id, signing_secret, "xoxb-shortcut-token");
    let mock = mock_slack(vec![("/views.open", StatusCode::OK, ok_reply())]).await;
    let router = app_with_slack_api(dir, &mock);

    let private_metadata =
        open_modal_and_take_private_metadata(&router, &mock, team_id, signing_secret).await;

    let opened = mock.calls_to("/views.open");
    assert_eq!(
        opened[0].authorization.as_deref(),
        Some("Bearer xoxb-shortcut-token"),
        "the install's own bot token authenticates the call"
    );
    let body: Value = serde_json::from_str(&opened[0].body).expect("views.open body is JSON");
    assert_eq!(body["trigger_id"], "trigger-123.456");
    assert_eq!(body["view"]["type"], "modal");
    assert_eq!(body["view"]["callback_id"], "hivemind_capture_decision");

    let context: Value = serde_json::from_str(&private_metadata).expect("metadata is JSON");
    assert_eq!(context["channel_id"], "C1");
    assert_eq!(context["message_ts"], "1715970801.000200");
    assert_eq!(
        context["thread_ts"], "1715970800.000100",
        "a message in a thread captures the thread"
    );
    let evidence = context["evidence"].as_str().expect("evidence");
    assert!(evidence.contains("U-AUTHOR"), "keeps who wrote it");
    assert!(evidence.contains("we should move to Postgres"));
}

#[tokio::test]
async fn submitting_the_capture_modal_queues_a_capture_that_drains_into_the_ledger() {
    let dir = test_ledger_dir();
    let (team_id, signing_secret) = ("T-SUBMIT", "submit-secret");
    install_workspace(&dir, team_id, signing_secret);
    let mock = mock_slack(vec![("/views.open", StatusCode::OK, ok_reply())]).await;
    let router = app_with_slack_api(dir.clone(), &mock);
    let private_metadata =
        open_modal_and_take_private_metadata(&router, &mock, team_id, signing_secret).await;

    let submission = view_submission_payload(
        team_id,
        "U-SUBMITTER",
        &private_metadata,
        &[
            ("title", "Move to Postgres"),
            ("rationale", "Concurrent writers need it"),
            ("options", "sqlite, postgres"),
            ("chosen", "Postgres"),
        ],
    );
    let (status, bytes) = call(
        router,
        signed_interactivity_request(signing_secret, &submission),
    )
    .await;

    // An empty 200 closes the modal; the capture is queued, not written inline.
    assert_eq!(status, StatusCode::OK);
    assert!(bytes.is_empty());
    let queued = queued_captures(&dir);
    assert_eq!(queued.len(), 1);
    let capture = &queued[0]["capture"];
    assert_eq!(capture["surface"], "message_action");
    assert_eq!(capture["user_id"], "U-SUBMITTER");
    assert_eq!(capture["title"], "Move to Postgres");
    assert_eq!(capture["chosen_option_label"], "postgres");
    assert_eq!(
        capture["permalink"],
        "slack://T-SUBMIT/C1/1715970800.000100"
    );

    let (actor_id, source_ref) = drain_one_decision(&dir, team_id);
    assert_eq!(actor_id, "slack:T-SUBMIT:U-SUBMITTER");
    assert_eq!(
        source_ref.as_deref(),
        Some("slack://T-SUBMIT/C1/1715970800.000100")
    );
}

#[tokio::test]
async fn an_invalid_modal_submission_keeps_the_modal_open_and_queues_nothing() {
    let dir = test_ledger_dir();
    let (team_id, signing_secret) = ("T-INVALID", "invalid-secret");
    install_workspace(&dir, team_id, signing_secret);
    let mock = mock_slack(vec![("/views.open", StatusCode::OK, ok_reply())]).await;
    let router = app_with_slack_api(dir.clone(), &mock);
    let private_metadata =
        open_modal_and_take_private_metadata(&router, &mock, team_id, signing_secret).await;

    let submission = view_submission_payload(
        team_id,
        "U-SUBMITTER",
        &private_metadata,
        &[
            ("title", "Move to Postgres"),
            ("options", "sqlite, postgres"),
            ("chosen", "mysql"),
        ],
    );
    let (status, body) = call_json(
        router,
        signed_interactivity_request(signing_secret, &submission),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["response_action"], "errors");
    let errors = body["errors"].as_object().expect("errors object");
    assert!(errors.contains_key("rationale"), "missing rationale named");
    assert!(errors.contains_key("chosen"), "unmatched choice named");
    assert!(
        !errors.contains_key("title"),
        "valid fields are not flagged"
    );
    assert!(queued_captures(&dir).is_empty(), "nothing is queued");
}

#[tokio::test]
async fn a_failed_views_open_is_reported_to_slack_instead_of_swallowed() {
    let dir = test_ledger_dir();
    let (team_id, signing_secret) = ("T-EXPIRED", "expired-secret");
    install_workspace(&dir, team_id, signing_secret);
    let mock = mock_slack(vec![(
        "/views.open",
        StatusCode::OK,
        json!({"ok": false, "error": "expired_trigger_id"}),
    )])
    .await;

    let req = signed_interactivity_request(
        signing_secret,
        &message_action_payload(team_id, "hivemind_capture_thread"),
    );
    let (status, body) = call_json(app_with_slack_api(dir, &mock), req).await;

    // A non-2xx makes Slack tell the user the shortcut failed; a 200 would
    // leave them staring at nothing.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let message = body["error"]["message"].as_str().expect("error message");
    assert!(message.contains("expired_trigger_id"), "{message}");
}

#[tokio::test]
async fn interactivity_verifies_the_signature_before_calling_slack_or_queueing() {
    let dir = test_ledger_dir();
    install_workspace(&dir, "T-A", "secret-a");
    install_workspace(&dir, "T-B", "secret-b");
    let mock = mock_slack(vec![("/views.open", StatusCode::OK, ok_reply())]).await;
    let router = app_with_slack_api(dir.clone(), &mock);

    // Workspace B's team id, signed with workspace A's secret.
    let forged = message_action_payload("T-B", "hivemind_capture_thread");
    let (status, _) = call(
        router.clone(),
        signed_interactivity_request("secret-a", &forged),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A team that never installed the app.
    let unknown = message_action_payload("T-UNKNOWN", "hivemind_capture_thread");
    let (status, _) = call(
        router.clone(),
        signed_interactivity_request("secret-a", &unknown),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A forged modal submission must not reach the queue either.
    let submission = view_submission_payload("T-B", "U1", "{}", &[("title", "t")]);
    let (status, _) = call(
        router.clone(),
        signed_interactivity_request("secret-a", &submission),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // No signature headers at all.
    let bare = Request::builder()
        .method("POST")
        .uri("/v1/slack/interactivity")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("payload=%7B%7D"))
        .expect("request builds");
    let (status, _) = call(router.clone(), bare).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(mock.call_count(), 0, "Slack was never called");
    assert!(queued_captures(&dir).is_empty());
}

#[tokio::test]
async fn interactivity_rejects_bodies_that_are_not_a_slack_payload() {
    let dir = test_ledger_dir();
    install_workspace(&dir, "T-SHAPE", "shape-secret");
    let router = app(dir);

    // Form body without a `payload` field.
    let req = signed_form_request("/v1/slack/interactivity", "shape-secret", &[("x", "y")]);
    let (status, _) = call(router.clone(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // `payload` that is not JSON.
    let req = signed_form_request(
        "/v1/slack/interactivity",
        "shape-secret",
        &[("payload", "not json")],
    );
    let (status, _) = call(router.clone(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // JSON naming no workspace.
    let req = signed_interactivity_request("shape-secret", &json!({"type": "block_actions"}));
    let (status, _) = call(router, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn interactions_the_app_does_not_define_are_acknowledged_and_ignored() {
    let dir = test_ledger_dir();
    let (team_id, signing_secret) = ("T-OTHER", "other-secret");
    install_workspace(&dir, team_id, signing_secret);
    let mock = mock_slack(vec![("/views.open", StatusCode::OK, ok_reply())]).await;
    let router = app_with_slack_api(dir.clone(), &mock);

    let block_actions = json!({
        "type": "block_actions",
        "team": {"id": team_id},
        "user": {"id": "U1"},
        "actions": []
    });
    let (status, _) = call(
        router.clone(),
        signed_interactivity_request(signing_secret, &block_actions),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let other_shortcut = message_action_payload(team_id, "some_other_shortcut");
    let (status, _) = call(
        router,
        signed_interactivity_request(signing_secret, &other_shortcut),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(
        mock.call_count(),
        0,
        "no modal for a shortcut that is not ours"
    );
    assert!(queued_captures(&dir).is_empty());
}

// ---------------------------------------------------------------------------
// reaction_added: fetch the reacted-to message, then capture it
// ---------------------------------------------------------------------------

fn reaction_event(team_id: &str, reactor: &str, emoji: &str, item: &Value) -> Value {
    json!({
        "type": "event_callback",
        "team_id": team_id,
        "event": {
            "type": "reaction_added",
            "user": reactor,
            "reaction": emoji,
            "item": item,
            "item_user": "U-AUTHOR",
            "event_ts": "1715970900.000300"
        }
    })
}

fn message_item(ts: &str) -> Value {
    json!({"type": "message", "channel": "C1", "ts": ts})
}

fn history_reply(messages: &Value) -> Value {
    json!({"ok": true, "messages": messages})
}

const DECISION_TEXT: &str = "Decision: Adopt Postgres\nRationale: Concurrent writers need it\nOptions: sqlite,postgres\nChosen: postgres";

#[tokio::test]
async fn reaction_added_fetches_the_message_and_captures_it_under_the_reactor() {
    let dir = test_ledger_dir();
    let (team_id, signing_secret) = ("T-REACT", "react-secret");
    install_workspace_with_bot_token(&dir, team_id, signing_secret, "xoxb-react-token");
    let mock = mock_slack(vec![(
        "/conversations.history",
        StatusCode::OK,
        history_reply(&json!([{
            "type": "message",
            "user": "U-AUTHOR",
            "ts": "1715970800.000100",
            "text": DECISION_TEXT
        }])),
    )])
    .await;

    let event = reaction_event(
        team_id,
        "U-REACTOR",
        "hivemind",
        &message_item("1715970800.000100"),
    );
    let (status, _) = call(
        app_with_slack_api(dir.clone(), &mock),
        signed_json_request("/v1/slack/events", signing_secret, &event),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let fetches = mock.calls_to("/conversations.history");
    assert_eq!(fetches.len(), 1);
    assert_eq!(
        fetches[0].authorization.as_deref(),
        Some("Bearer xoxb-react-token")
    );
    let asked: Vec<(String, String)> =
        serde_urlencoded::from_str(&fetches[0].body).expect("history request is a form");
    for expected in [
        ("channel", "C1"),
        ("latest", "1715970800.000100"),
        ("inclusive", "true"),
        ("limit", "1"),
    ] {
        assert!(
            asked
                .iter()
                .any(|(k, v)| (k.as_str(), v.as_str()) == expected),
            "history request carries {expected:?}: {asked:?}"
        );
    }

    let queued = queued_captures(&dir);
    assert_eq!(queued.len(), 1);
    let capture = &queued[0]["capture"];
    assert_eq!(capture["surface"], "reaction");
    assert_eq!(capture["reaction_emoji"], "hivemind");
    assert_eq!(capture["user_id"], "U-REACTOR");
    assert_eq!(capture["title"], "Adopt Postgres");
    assert_eq!(capture["permalink"], "slack://T-REACT/C1/1715970800.000100");
    let evidence = capture["thread_text"].as_str().expect("evidence text");
    assert!(
        evidence.contains("U-AUTHOR"),
        "the message's author survives in the evidence: {evidence}"
    );

    let (actor_id, source_ref) = drain_one_decision(&dir, team_id);
    assert_eq!(actor_id, "slack:T-REACT:U-REACTOR");
    assert_eq!(
        source_ref.as_deref(),
        Some("slack://T-REACT/C1/1715970800.000100")
    );
}

#[tokio::test]
async fn a_reaction_that_cannot_become_a_capture_is_acknowledged_and_captures_nothing() {
    let ts = "1715970800.000100";
    let no_markers = history_reply(&json!([
        {"type": "message", "user": "U-AUTHOR", "ts": ts, "text": "lunch at noon?"}
    ]));
    let a_different_message = history_reply(&json!([
        {"type": "message", "user": "U-AUTHOR", "ts": "1715970000.000001", "text": DECISION_TEXT}
    ]));
    let posted_by_an_app = history_reply(&json!([
        {"type": "message", "bot_id": "B1", "ts": ts, "text": DECISION_TEXT}
    ]));
    let history_error = json!({"ok": false, "error": "not_in_channel"});
    let rate_limited = json!({"ok": false, "error": "ratelimited"});

    // (case, emoji, item, Slack's answer, whether history is expected to be asked)
    let cases = [
        (
            "a different emoji",
            "eyes",
            message_item(ts),
            StatusCode::OK,
            no_markers.clone(),
            false,
        ),
        (
            "a reaction on a file",
            "hivemind",
            json!({"type": "file", "file": "F1"}),
            StatusCode::OK,
            no_markers.clone(),
            false,
        ),
        (
            "a message without markers",
            "hivemind",
            message_item(ts),
            StatusCode::OK,
            no_markers,
            true,
        ),
        (
            "a thread reply Slack cannot return by ts",
            "hivemind",
            message_item(ts),
            StatusCode::OK,
            a_different_message,
            true,
        ),
        (
            "a message posted by an app",
            "hivemind",
            message_item(ts),
            StatusCode::OK,
            posted_by_an_app,
            true,
        ),
        (
            "the bot not being in the channel",
            "hivemind",
            message_item(ts),
            StatusCode::OK,
            history_error,
            true,
        ),
        (
            "Slack rate-limiting the app",
            "hivemind",
            message_item(ts),
            StatusCode::TOO_MANY_REQUESTS,
            rate_limited,
            true,
        ),
    ];

    for (case, emoji, item, slack_status, slack_reply, history_asked) in cases {
        let dir = test_ledger_dir();
        let (team_id, signing_secret) = ("T-NOCAPTURE", "nocapture-secret");
        install_workspace(&dir, team_id, signing_secret);
        let mock = mock_slack(vec![("/conversations.history", slack_status, slack_reply)]).await;

        let event = reaction_event(team_id, "U-REACTOR", emoji, &item);
        let (status, _) = call(
            app_with_slack_api(dir.clone(), &mock),
            signed_json_request("/v1/slack/events", signing_secret, &event),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::OK,
            "{case}: the event is still acknowledged"
        );
        assert_eq!(
            mock.calls_to("/conversations.history").len(),
            usize::from(history_asked),
            "{case}: whether Slack was asked for the message"
        );
        assert!(
            queued_captures(&dir).is_empty(),
            "{case}: nothing is queued"
        );
    }
}
