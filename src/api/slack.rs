//! Slack front door — `/v1/slack/*` HTTP endpoints that make the existing
//! [`crate::slack_app`] library (installations, capture queue, slash-command
//! handling) reachable from Slack. Every function here is transport
//! plumbing: request-signature verification, form/JSON parsing, and thin
//! dispatch into `crate::slack_app`'s functions — no decision logic lives
//! here (see AGENTS.md's three-layer separation).
//!
//! ## Security
//!
//! Every route in this module authenticates the caller via Slack's own
//! request-signing scheme ([`verify_slack_signature`]), never via the
//! bearer-token / WorkOS-JWT path in [`super::auth`]. A request's claimed
//! `team_id` is only trusted once its signature has been verified against
//! that specific workspace's stored `signing_secret` — parsing the body to
//! find which install to check is safe (no code execution risk), but no
//! side effect (queue write, ledger read) happens before verification
//! succeeds.
//!
//! `url_verification` carries no `team_id` (Slack sends it once, when the
//! Request URL is first configured, before any workspace has necessarily
//! installed the app) — it is verified against the app-level
//! `HIVEMIND_SLACK_SIGNING_SECRET`, the same secret used to populate new
//! installs created via the OAuth callback below.
//!
//! ## Multi-tenant
//!
//! Each Slack workspace (`team_id`) maps 1:1 onto its own HiveMind tenant
//! (`TenantId::new(team_id)`), which must already be registered
//! (`hivemind tenant create <team_id>` on SQLite, or provisioned
//! automatically by the OAuth callback below). This is the seam that keeps
//! a request signed by workspace A from ever writing into workspace B's
//! ledger: `team_id` comes only from the verified body, and every ledger
//! open goes through [`ApiBackend::open_ledger_for_tenant`], which 404s on
//! an unknown tenant exactly like the bearer-token path does.
//!
//! ## Backend support
//!
//! [`SlackAppStore`]'s installs/queue storage is local-disk JSON, scoped to
//! `ApiConfig::hivemind_dir` — it has no Postgres-backed storage yet. These
//! routes are therefore only wired up when the server is running the
//! SQLite backend (`AppState::slack_store` is `None` on Postgres); see
//! docs/SLACK_APP.md for the tracked follow-up.
//!
//! ## Outbound calls
//!
//! Two flows need Slack's Web API, through [`web::SlackWebClient`] and the
//! install's bot token: the capture modal (`views.open`, see
//! [`interactivity`]) and `reaction_added` capture (`conversations.history`,
//! because the Events API payload for a reaction carries no message text).
//! Both are bounded by a short timeout and never retried.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::error::CliError;
use crate::events::TenantId;
use crate::ingest::{
    parse_decision_markers, slack_thread_source_ref, SlackDecisionMarkers, SlackMessageFixture,
    SlackThreadFixture, DEFAULT_SLACK_MENTION,
};
use crate::ledger::{AnyLedger, SqliteEventLedger, TenantScopedLedger, TenantScopedOwnedLedger};
use crate::slack_app::{
    handle_slack_command, message_evidence, SlackAppStore, SlackCaptureRequest,
    SlackCaptureSurface, SlackCommandRequest, SlackWorkspaceInstall,
};

use super::graph::get_cached_graph;
use super::{respond, ApiBackend, ApiError, ApiResult, AppState};

mod interactivity;
mod web;

pub(super) use interactivity::interactivity_handler;
use web::SlackMessage;
pub(super) use web::SlackWebClient;

const SLACK_SIGNATURE_HEADER: &str = "x-slack-signature";
const SLACK_TIMESTAMP_HEADER: &str = "x-slack-request-timestamp";
const SLACK_SIGNATURE_VERSION: &str = "v0";
const SLACK_TIMESTAMP_TOLERANCE_SECS: i64 = 300;
const SLACK_BACKEND_UNSUPPORTED: &str =
    "Slack app front door is only available on the SQLite backend (installations/queue storage is local-disk); see docs/SLACK_APP.md";
const DEFAULT_COMMAND_RESULT_LIMIT: usize = 5;
const DEFAULT_INSTALL_REACTION_EMOJI: &str = "hivemind";
const SLACK_DRAIN_POLL_INTERVAL: Duration = Duration::from_secs(15);
const SLACK_OAUTH_ACCESS_URL: &str = "https://slack.com/api/oauth.v2.access";

// ---------------------------------------------------------------------------
// Signature verification — the security gate every route below depends on.
// ---------------------------------------------------------------------------

/// Verifies `X-Slack-Signature` per Slack's documented scheme: HMAC-SHA256
/// over `v0:{timestamp}:{raw body}` using the workspace's signing secret,
/// constant-time compared, with the timestamp rejected outside a 5-minute
/// window (replay protection). Hand-rolled (not the `hmac` crate) to avoid
/// pulling in a second `sha2`/`digest` major-version line alongside the
/// crate's existing `sha2 = "0.10"` dependency; verified against Slack's own
/// documented example vector in `tests`.
fn verify_slack_signature(
    signing_secret: &str,
    timestamp_header: &str,
    body: &[u8],
    signature_header: &str,
) -> Result<(), &'static str> {
    let timestamp: i64 = timestamp_header
        .parse()
        .map_err(|_| "invalid x-slack-request-timestamp")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock before unix epoch")?
        .as_secs() as i64;
    if (now - timestamp).abs() > SLACK_TIMESTAMP_TOLERANCE_SECS {
        return Err("stale slack request timestamp");
    }

    let mut base = Vec::with_capacity(2 + timestamp_header.len() + 1 + body.len());
    base.extend_from_slice(SLACK_SIGNATURE_VERSION.as_bytes());
    base.push(b':');
    base.extend_from_slice(timestamp_header.as_bytes());
    base.push(b':');
    base.extend_from_slice(body);

    let expected = hmac_sha256_hex(signing_secret.as_bytes(), &base);
    let expected_header = format!("{SLACK_SIGNATURE_VERSION}={expected}");

    if constant_time_eq(expected_header.as_bytes(), signature_header.as_bytes()) {
        Ok(())
    } else {
        Err("invalid slack request signature")
    }
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    hmac_sha256(key, message)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Standard HMAC construction (RFC 2104) over `sha2::Sha256`.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    outer.finalize().into()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[allow(clippy::result_large_err)]
fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Response> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::unauthorized(format!("missing {name} header")).into_response())
}

/// The gate every workspace-scoped route passes before acting on a request:
/// finds the install for the claimed `team_id` and verifies the signature
/// against *that* install's signing secret. An unknown workspace and a bad
/// signature answer identically, so the route does not reveal which
/// workspaces are installed.
fn authenticate_install(
    store: &SlackAppStore,
    team_id: &str,
    timestamp: &str,
    signature: &str,
    body: &[u8],
) -> ApiResult<SlackWorkspaceInstall> {
    let install = store
        .installation(team_id)
        .map_err(|_| ApiError::unauthorized("invalid slack request signature"))?;
    verify_slack_signature(&install.signing_secret, timestamp, body, signature)
        .map_err(|_| ApiError::unauthorized("invalid slack request signature"))?;
    Ok(install)
}

// ---------------------------------------------------------------------------
// Events API — POST /v1/slack/events
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SlackEventEnvelope {
    #[serde(rename = "url_verification")]
    UrlVerification { challenge: String },
    #[serde(rename = "event_callback")]
    EventCallback {
        team_id: String,
        event: Box<SlackInnerEvent>,
    },
    /// Other envelope types (`app_rate_limited`, etc.) are acknowledged and ignored.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct SlackInnerEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    bot_id: Option<String>,
    #[serde(default)]
    reaction: Option<String>,
    /// What a `reaction_added` was added to.
    #[serde(default)]
    item: Option<SlackReactionItem>,
}

#[derive(Debug, Deserialize)]
struct SlackReactionItem {
    /// `message` for the only item this app acts on; defaulted so an item of
    /// any other shape is ignored instead of failing the whole event.
    #[serde(rename = "type", default)]
    item_type: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

pub(super) async fn events_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(store) = state.slack_store.clone() else {
        return ApiError::internal(SLACK_BACKEND_UNSUPPORTED).into_response();
    };

    let timestamp = match required_header(&headers, SLACK_TIMESTAMP_HEADER) {
        Ok(v) => v.to_owned(),
        Err(resp) => return resp,
    };
    let signature = match required_header(&headers, SLACK_SIGNATURE_HEADER) {
        Ok(v) => v.to_owned(),
        Err(resp) => return resp,
    };

    let envelope: SlackEventEnvelope = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return ApiError::validation("invalid slack events payload").into_response(),
    };

    match envelope {
        SlackEventEnvelope::UrlVerification { challenge } => {
            let Some(secret) = state.slack_app_signing_secret.clone() else {
                return ApiError::unauthorized(
                    "HIVEMIND_SLACK_SIGNING_SECRET not configured; cannot verify url_verification",
                )
                .into_response();
            };
            if verify_slack_signature(&secret, &timestamp, &body, &signature).is_err() {
                return ApiError::unauthorized("invalid slack request signature").into_response();
            }
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain")],
                challenge,
            )
                .into_response()
        }
        SlackEventEnvelope::EventCallback { team_id, event } => {
            let queue = store.clone();
            let result = tokio::task::spawn_blocking(move || {
                handle_event_callback(&store, &team_id, &event, &timestamp, &signature, &body)
            })
            .await;
            match result {
                Ok(Ok(EventFollowUp::Done)) => StatusCode::OK.into_response(),
                Ok(Ok(EventFollowUp::CaptureReaction(reaction))) => {
                    // Always acknowledged: a failed capture is logged, not
                    // answered with a 5xx, because Slack retries failed
                    // deliveries and disables subscriptions that keep failing.
                    complete_reaction_capture(&state.slack_web, queue, *reaction).await;
                    StatusCode::OK.into_response()
                }
                Ok(Err(e)) => e.into_response(),
                Err(e) => ApiError::internal(e.to_string()).into_response(),
            }
        }
        SlackEventEnvelope::Other => StatusCode::OK.into_response(),
    }
}

/// What is left to do for an authenticated event once its synchronous part is
/// done.
enum EventFollowUp {
    Done,
    /// A `reaction_added` on the install's capture emoji: the reacted-to
    /// message has to be fetched from Slack before it can be parsed.
    CaptureReaction(Box<ReactionCapture>),
}

struct ReactionCapture {
    install: SlackWorkspaceInstall,
    /// The Slack user who added the reaction.
    reactor: String,
    channel: String,
    ts: String,
}

fn handle_event_callback(
    store: &SlackAppStore,
    team_id: &str,
    event: &SlackInnerEvent,
    timestamp: &str,
    signature: &str,
    body: &[u8],
) -> ApiResult<EventFollowUp> {
    let install = authenticate_install(store, team_id, timestamp, signature, body)?;

    match event.event_type.as_str() {
        "app_mention" => {
            enqueue_marker_capture(store, &install, event, None).map(|()| EventFollowUp::Done)
        }
        // Only plain user messages: skip edits/deletes/bot echoes (`subtype`)
        // and our own bot's own posts (`bot_id`), which would otherwise loop.
        "message" if event.subtype.is_none() && event.bot_id.is_none() => {
            enqueue_marker_capture(store, &install, event, Some(DEFAULT_SLACK_MENTION))
                .map(|()| EventFollowUp::Done)
        }
        "reaction_added" => Ok(reaction_follow_up(install, event)),
        _ => Ok(EventFollowUp::Done),
    }
}

/// A reaction only starts a capture when it is the install's configured
/// emoji, on a message, from an identifiable user.
fn reaction_follow_up(install: SlackWorkspaceInstall, event: &SlackInnerEvent) -> EventFollowUp {
    if event.reaction.as_deref() != Some(install.reaction_emoji.as_str()) {
        return EventFollowUp::Done;
    }
    let (Some(reactor), Some(item)) = (&event.user, &event.item) else {
        return EventFollowUp::Done;
    };
    let (true, Some(channel), Some(ts)) = (item.item_type == "message", &item.channel, &item.ts)
    else {
        return EventFollowUp::Done;
    };
    EventFollowUp::CaptureReaction(Box::new(ReactionCapture {
        install,
        reactor: reactor.clone(),
        channel: channel.clone(),
        ts: ts.clone(),
    }))
}

/// Fetches the message a matching reaction was added to and, when it carries
/// the deterministic `Decision:`/`Rationale:`/`Options:` markers, enqueues it
/// like any other capture. Every way this can end without a capture is
/// logged; none of them fails the acknowledgement of the event.
async fn complete_reaction_capture(
    web: &SlackWebClient,
    store: SlackAppStore,
    reaction: ReactionCapture,
) {
    let team_id = reaction.install.team_id.clone();
    let message = match web
        .fetch_message(&reaction.install.bot_token, &reaction.channel, &reaction.ts)
        .await
    {
        Ok(Some(message)) => message,
        Ok(None) => {
            info!(
                target: "hivemind::api::slack",
                team_id = %team_id,
                channel = %reaction.channel,
                ts = %reaction.ts,
                "reacted-to message could not be fetched by ts (a thread reply, or deleted \
                 since); nothing captured"
            );
            return;
        }
        Err(error) => {
            warn!(
                target: "hivemind::api::slack",
                team_id = %team_id,
                error = %error,
                "could not fetch the reacted-to message; nothing captured"
            );
            return;
        }
    };

    let queued =
        tokio::task::spawn_blocking(move || enqueue_reaction_capture(&store, &reaction, &message))
            .await;
    match queued {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(
            target: "hivemind::api::slack",
            team_id = %team_id,
            error = %error,
            "reaction capture could not be queued"
        ),
        Err(error) => warn!(
            target: "hivemind::api::slack",
            team_id = %team_id,
            error = %error,
            "reaction capture task panicked"
        ),
    }
}

/// The capture is attributed to the user who *reacted* — the actor who took
/// the action — while the message's own author and timestamp are kept in the
/// evidence text ([`message_evidence`]), so who wrote the words is not lost.
fn enqueue_reaction_capture(
    store: &SlackAppStore,
    reaction: &ReactionCapture,
    message: &SlackMessage,
) -> ApiResult<()> {
    let install = &reaction.install;
    let Some(author) = message.user.as_deref() else {
        info!(
            target: "hivemind::api::slack",
            team_id = %install.team_id,
            "reacted-to message has no user author (posted by an app?); nothing captured"
        );
        return Ok(());
    };

    let fixture = SlackThreadFixture {
        team_id: install.team_id.clone(),
        channel_id: reaction.channel.clone(),
        thread_ts: message
            .thread_ts
            .clone()
            .unwrap_or_else(|| message.ts.clone()),
        messages: vec![SlackMessageFixture {
            user_id: author.to_owned(),
            ts: message.ts.clone(),
            text: message.text.clone(),
        }],
    };
    let markers: SlackDecisionMarkers = match parse_decision_markers(&fixture) {
        Ok(markers) => markers,
        Err(error) => {
            info!(
                target: "hivemind::api::slack",
                team_id = %install.team_id,
                reason = %error,
                "reacted-to message is not a decision capture; nothing captured"
            );
            return Ok(());
        }
    };

    store
        .enqueue_capture(SlackCaptureRequest {
            team_id: install.team_id.clone(),
            user_id: reaction.reactor.clone(),
            channel_id: reaction.channel.clone(),
            message_ts: message.ts.clone(),
            thread_ts: fixture.thread_ts.clone(),
            permalink: slack_thread_source_ref(&fixture),
            surface: SlackCaptureSurface::Reaction,
            reaction_emoji: Some(install.reaction_emoji.clone()),
            title: markers.title,
            rationale: markers.rationale,
            topic_keys: markers.topic_keys,
            option_labels: markers.option_labels,
            chosen_option_label: markers.chosen_option_label,
            thread_text: message_evidence(&message.ts, author, &message.text),
        })
        .map(|_| ())
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// Builds a single-message [`SlackThreadFixture`] from a live event and
/// reuses [`parse_decision_markers`] — the same deterministic, LLM-free
/// Decision:/Rationale:/Options: grammar `hivemind ingest slack-thread`
/// already uses — to decide whether the message is a capture. No markers,
/// no mention (when required), or missing fields: ack and do nothing.
fn enqueue_marker_capture(
    store: &SlackAppStore,
    install: &SlackWorkspaceInstall,
    event: &SlackInnerEvent,
    required_mention: Option<&str>,
) -> ApiResult<()> {
    let (Some(channel), Some(user), Some(text), Some(ts)) =
        (&event.channel, &event.user, &event.text, &event.ts)
    else {
        return Ok(());
    };

    if let Some(mention) = required_mention {
        if !text.contains(mention) {
            return Ok(());
        }
    }

    let fixture = SlackThreadFixture {
        team_id: install.team_id.clone(),
        channel_id: channel.clone(),
        thread_ts: event.thread_ts.clone().unwrap_or_else(|| ts.clone()),
        messages: vec![SlackMessageFixture {
            user_id: user.clone(),
            ts: ts.clone(),
            text: text.clone(),
        }],
    };

    let markers: SlackDecisionMarkers = match parse_decision_markers(&fixture) {
        Ok(markers) => markers,
        Err(_) => return Ok(()), // no Decision:/Rationale:/Options: markers — not a capture
    };

    let capture = SlackCaptureRequest {
        team_id: install.team_id.clone(),
        user_id: user.clone(),
        channel_id: channel.clone(),
        message_ts: ts.clone(),
        thread_ts: fixture.thread_ts.clone(),
        permalink: slack_thread_source_ref(&fixture),
        surface: SlackCaptureSurface::EventMention,
        reaction_emoji: None,
        title: markers.title,
        rationale: markers.rationale,
        topic_keys: markers.topic_keys,
        option_labels: markers.option_labels,
        chosen_option_label: markers.chosen_option_label,
        thread_text: text.clone(),
    };

    store
        .enqueue_capture(capture)
        .map(|_| ())
        .map_err(|e| ApiError::internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// Slash commands — POST /v1/slack/commands
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SlackCommandForm {
    team_id: String,
    user_id: String,
    #[serde(default)]
    text: String,
}

pub(super) async fn commands_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(store) = state.slack_store.clone() else {
        return ApiError::internal(SLACK_BACKEND_UNSUPPORTED).into_response();
    };
    let timestamp = match required_header(&headers, SLACK_TIMESTAMP_HEADER) {
        Ok(v) => v.to_owned(),
        Err(resp) => return resp,
    };
    let signature = match required_header(&headers, SLACK_SIGNATURE_HEADER) {
        Ok(v) => v.to_owned(),
        Err(resp) => return resp,
    };

    let form: SlackCommandForm = match serde_urlencoded::from_bytes(&body) {
        Ok(v) => v,
        Err(_) => return ApiError::validation("invalid slack command payload").into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<serde_json::Value> {
        let install = authenticate_install(&store, &form.team_id, &timestamp, &signature, &body)?;

        let tenant_id = TenantId::new(&form.team_id)
            .map_err(|_| ApiError::validation("team_id must not be empty"))?;
        let ledger = backend.open_ledger_for_tenant(&tenant_id)?;
        // get_cached_graph/project_from_ledger_for_tenant thread tenant_id
        // explicitly, so the raw ledger is correct here. handle_slack_command
        // internally uses EventLedger::read()'s bare, tenant-less default
        // (via slack_app.rs's decision_citations) — pin it to this team's
        // tenant with the same TenantScopedLedger wrapper the CLI's
        // `slack-app command` path already uses for exactly this reason.
        let graph = get_cached_graph(&ledger, &tenant_id, &cache)?;
        let scoped_ledger = TenantScopedLedger::new(&ledger, tenant_id.clone());

        let mut response = handle_slack_command(
            &scoped_ledger,
            &*graph,
            &store,
            &SlackCommandRequest {
                team_id: form.team_id,
                user_id: form.user_id,
                text: form.text,
                limit: DEFAULT_COMMAND_RESULT_LIMIT,
            },
        )
        .map_err(|e| ApiError::internal(e.to_string()))?;

        // `handle_slack_command`'s bare "capture" reply hands back a modal
        // descriptor — meaningful for the CLI/testing shim, but a slash
        // command names no message to attach a capture to, so this route
        // opens no modal. Point at the surfaces that do have one, instead of
        // claiming a modal that never opens.
        if response.action.as_deref() == Some("open_modal") {
            response.text = format!(
                "To capture a decision from Slack, use the *Capture this thread as a decision* \
                 message shortcut on the message (its ⋯ menu), or react to a message with \
                 :{}: when it carries `Decision:`, `Rationale:` and `Options:` lines. From a \
                 terminal, use `hivemind emit decision.capture`. Browse decisions with \
                 `/hivemind query <topic>` or `/hivemind show <id>`.",
                install.reaction_emoji
            );
            response.action = None;
            response.modal = None;
        }

        serde_json::to_value(response).map_err(|e| ApiError::internal(e.to_string()))
    })
    .await;

    respond(result, StatusCode::OK)
}

// ---------------------------------------------------------------------------
// OAuth install — GET /v1/slack/oauth/callback
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(super) struct OauthCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SlackOauthAccessResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    team: Option<SlackOauthTeam>,
}

#[derive(Debug, Deserialize)]
struct SlackOauthTeam {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

pub(super) async fn oauth_callback_handler(
    State(state): State<AppState>,
    Query(params): Query<OauthCallbackParams>,
    headers: HeaderMap,
) -> Response {
    if let Some(error) = params.error {
        return ApiError::validation(format!("slack oauth error: {error}")).into_response();
    }
    let Some(code) = params.code else {
        return ApiError::validation("missing code parameter").into_response();
    };
    // Full CSRF nonce tracking (issue + store + single-use verify) is a
    // separate persistence subsystem this slice deliberately does not add —
    // callers that need it layer their own short-lived state store around
    // `state` today; we only require it be present and non-empty.
    if params.state.as_deref().unwrap_or("").trim().is_empty() {
        return ApiError::validation("missing state parameter").into_response();
    }

    let Some(store) = state.slack_store.clone() else {
        return ApiError::internal(SLACK_BACKEND_UNSUPPORTED).into_response();
    };
    let (Some(client_id), Some(client_secret), Some(signing_secret)) = (
        state.slack_client_id.clone(),
        state.slack_client_secret.clone(),
        state.slack_app_signing_secret.clone(),
    ) else {
        return ApiError::internal(
            "HIVEMIND_SLACK_CLIENT_ID / HIVEMIND_SLACK_CLIENT_SECRET / \
             HIVEMIND_SLACK_SIGNING_SECRET must be configured for OAuth install",
        )
        .into_response();
    };

    let base_url = derive_self_base_url(&headers);
    let redirect_uri = format!("{base_url}/v1/slack/oauth/callback");

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };

    let exchange = client
        .post(SLACK_OAUTH_ACCESS_URL)
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await;

    let response = match exchange {
        Ok(r) => r,
        Err(e) => {
            return ApiError::internal(format!("slack oauth exchange failed: {e}")).into_response()
        }
    };

    let payload: SlackOauthAccessResponse = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            return ApiError::internal(format!("slack oauth response invalid: {e}")).into_response()
        }
    };

    if !payload.ok {
        let reason = payload.error.unwrap_or_else(|| "unknown_error".to_owned());
        return ApiError::validation(format!("slack oauth exchange rejected: {reason}"))
            .into_response();
    }
    let Some(team) = payload.team else {
        return ApiError::internal("slack oauth response missing team").into_response();
    };
    let Some(bot_token) = payload.access_token else {
        return ApiError::internal("slack oauth response missing access_token").into_response();
    };

    let backend = Arc::clone(&state.backend);
    let team_id = team.id;
    let team_name = team.name.clone().unwrap_or_else(|| team_id.clone());
    let result = tokio::task::spawn_blocking(move || -> ApiResult<serde_json::Value> {
        provision_slack_tenant(&backend, &team_id, &team_name)?;
        let summary = store
            .install_workspace(SlackWorkspaceInstall {
                team_id: team_id.clone(),
                team_name,
                bot_token,
                signing_secret,
                hivemind_url: base_url,
                reaction_emoji: DEFAULT_INSTALL_REACTION_EMOJI.to_owned(),
                actor_mappings: Default::default(),
            })
            .map_err(|e| ApiError::internal(e.to_string()))?;
        serde_json::to_value(summary).map_err(|e| ApiError::internal(e.to_string()))
    })
    .await;

    respond(result, StatusCode::CREATED)
}

fn derive_self_base_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

fn provision_slack_tenant(backend: &ApiBackend, team_id: &str, team_name: &str) -> ApiResult<()> {
    // Only read on the shared-backend-postgres arm below; unused without that feature.
    #[cfg(not(feature = "shared-backend-postgres"))]
    let _ = team_name;
    let tenant_id =
        TenantId::new(team_id).map_err(|_| ApiError::validation("team_id must not be empty"))?;
    match backend {
        ApiBackend::Sqlite(dir) => {
            let ledger = SqliteEventLedger::open(dir.as_ref())
                .map_err(|e| ApiError::internal(e.to_string()))?;
            match ledger.create_tenant(&tenant_id) {
                Ok(()) => Ok(()),
                // Idempotent re-install of an already-provisioned workspace.
                Err(e) if e.to_string().contains("already exists") => Ok(()),
                Err(e) => Err(ApiError::internal(e.to_string())),
            }
        }
        #[cfg(feature = "shared-backend-postgres")]
        ApiBackend::Postgres { tenant_store, .. } => tenant_store
            .provision_tenant(team_id, team_name)
            .map(|_| ())
            .map_err(|e| ApiError::internal(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Background capture-queue drain — multi-tenant, spawned once at server start.
// ---------------------------------------------------------------------------

/// Spawns the background task that drains `SlackAppStore`'s capture queue
/// into each capture's own tenant ledger, on a fixed poll interval. Mirrors
/// the `crate::classifier`/`crate::scorer` `try_spawn` pattern: optional,
/// and the rest of the system stays fully correct without it (captures
/// simply wait in the queue). No-op when the backend has no `slack_store`
/// (Postgres — see module docs).
pub(super) fn try_spawn_drain_loop(state: &AppState) {
    let Some(store) = state.slack_store.clone() else {
        return;
    };
    let backend = Arc::clone(&state.backend);
    tokio::spawn(async move {
        info!(target: "hivemind::api::slack", "slack capture queue drain worker started");
        loop {
            let store = store.clone();
            let backend = Arc::clone(&backend);
            let result = tokio::task::spawn_blocking(move || {
                store.drain_queue_multi_tenant(|team_id| resolve_tenant_ledger(&backend, team_id))
            })
            .await;

            match result {
                Ok(Ok(report)) if report.processed_count > 0 || report.failed_count > 0 => {
                    info!(
                        target: "hivemind::api::slack",
                        processed = report.processed_count,
                        failed = report.failed_count,
                        remaining = report.queued_after,
                        "slack capture queue drained"
                    );
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    warn!(target: "hivemind::api::slack", error = %e, "slack queue drain failed")
                }
                Err(e) => warn!(
                    target: "hivemind::api::slack",
                    error = %e,
                    "slack queue drain task panicked"
                ),
            }

            tokio::time::sleep(SLACK_DRAIN_POLL_INTERVAL).await;
        }
    });
}

/// Resolves a ledger pinned to `team_id`'s tenant. `process_capture` /
/// `import_slack_thread` write via `Commands::new_with_provenance`, whose
/// `CommandContext` defaults to `TenantId::local()` — so without this
/// wrapper, every drained capture would land in the "local" tenant
/// regardless of which workspace it came from. `TenantScopedOwnedLedger`
/// overrides every `EventLedger` call (append AND the idempotency-check
/// read in `find_existing_slack_decision`) to the real tenant instead.
fn resolve_tenant_ledger(
    backend: &ApiBackend,
    team_id: &str,
) -> crate::Result<TenantScopedOwnedLedger<AnyLedger>> {
    let tenant_id = TenantId::new(team_id).map_err(|e| CliError::InvalidInput(e.to_string()))?;
    let ledger = backend
        .open_ledger_for_tenant(&tenant_id)
        .map_err(|e| CliError::InvalidInput(e.to_string()))?;
    Ok(TenantScopedOwnedLedger::new(ledger, tenant_id))
}

#[cfg(test)]
mod tests;
