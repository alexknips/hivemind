//! `POST /v1/slack/interactivity` — Slack's interactive-component callbacks
//! for the "Capture this thread as a decision" message shortcut and its
//! modal. Slack posts `application/x-www-form-urlencoded` with a single
//! `payload` field holding the JSON; the signature is over the raw body,
//! exactly as on the other Slack routes.
//!
//! - `message_action` (the shortcut was invoked on a message): opens the
//!   capture modal with `views.open`, carrying the message's coordinates and
//!   text in the view's `private_metadata`.
//! - `view_submission` (the modal was submitted): turns the inputs into a
//!   capture and enqueues it onto the same queue the events route feeds,
//!   attributed to the Slack user who submitted. Input the capture cannot
//!   accept is answered with `response_action: "errors"`, which keeps the
//!   modal open with the message beside the offending field — nothing is
//!   dropped silently.
//! - Anything else (`block_actions`, `view_closed`, ...) is acknowledged and
//!   ignored: the capture modal has no interactive components.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::api::{ApiError, ApiResult, AppState};
use crate::slack_app::{
    capture_from_modal_submission, capture_message_modal, message_evidence, SlackAppStore,
    SlackCaptureModalContext, SlackModalSubmissionError, SlackWorkspaceInstall,
    CAPTURE_SHORTCUT_CALLBACK_ID,
};

use super::{
    authenticate_install, required_header, SLACK_BACKEND_UNSUPPORTED, SLACK_SIGNATURE_HEADER,
    SLACK_TIMESTAMP_HEADER,
};

/// Slack omits `user` on messages posted by apps and integrations.
const UNKNOWN_AUTHOR: &str = "unknown";

#[derive(Debug, Deserialize)]
struct InteractivityForm {
    payload: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum InteractivityPayload {
    #[serde(rename = "message_action")]
    MessageAction(Box<MessageAction>),
    #[serde(rename = "view_submission")]
    ViewSubmission(Box<ViewSubmission>),
    /// `block_actions`, `view_closed`, global shortcuts, ...
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct MessageAction {
    callback_id: String,
    /// Valid for about three seconds; `views.open` must spend it in that time.
    trigger_id: String,
    channel: SlackRef,
    message: ShortcutMessage,
}

#[derive(Debug, Deserialize)]
struct ViewSubmission {
    user: SlackRef,
    view: Value,
}

#[derive(Debug, Deserialize)]
struct SlackRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ShortcutMessage {
    ts: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    thread_ts: Option<String>,
}

pub(in crate::api) async fn interactivity_handler(
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

    let payload = match parse_payload(&body) {
        Ok(payload) => payload,
        Err(error) => return error.into_response(),
    };
    // Which install's signing secret to check is read from the still
    // unverified body — safe, because nothing acts on it until the signature
    // over the exact bytes verifies against that install's own secret.
    let Some(team_id) = payload
        .pointer("/team/id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return ApiError::validation("slack interactivity payload has no team id").into_response();
    };

    let authenticated = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || {
            authenticate_install(&store, &team_id, &timestamp, &signature, &body)
        })
        .await
    };
    let install = match authenticated {
        Ok(Ok(install)) => install,
        Ok(Err(error)) => return error.into_response(),
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };

    match serde_json::from_value::<InteractivityPayload>(payload) {
        Ok(InteractivityPayload::MessageAction(action)) => {
            open_capture_modal(&state, &install, *action).await
        }
        Ok(InteractivityPayload::ViewSubmission(submission)) => {
            submit_capture_modal(store, &install, *submission).await
        }
        Ok(InteractivityPayload::Other) => StatusCode::OK.into_response(),
        Err(_) => ApiError::validation("unrecognized slack interactivity payload").into_response(),
    }
}

fn parse_payload(body: &[u8]) -> ApiResult<Value> {
    let invalid = || ApiError::validation("invalid slack interactivity payload");
    let form: InteractivityForm = serde_urlencoded::from_bytes(body).map_err(|_| invalid())?;
    serde_json::from_str(&form.payload).map_err(|_| invalid())
}

async fn open_capture_modal(
    state: &AppState,
    install: &SlackWorkspaceInstall,
    action: MessageAction,
) -> Response {
    if action.callback_id != CAPTURE_SHORTCUT_CALLBACK_ID {
        info!(
            target: "hivemind::api::slack",
            callback_id = %action.callback_id,
            "ignoring a message shortcut this app does not define"
        );
        return StatusCode::OK.into_response();
    }

    let MessageAction {
        trigger_id,
        channel,
        message,
        ..
    } = action;
    let author = message.user.as_deref().unwrap_or(UNKNOWN_AUTHOR);
    let context = SlackCaptureModalContext {
        channel_id: channel.id,
        thread_ts: message
            .thread_ts
            .clone()
            .unwrap_or_else(|| message.ts.clone()),
        evidence: message_evidence(&message.ts, author, &message.text),
        message_ts: message.ts,
    };
    let view = match capture_message_modal(&context) {
        Ok(view) => view,
        Err(error) => return ApiError::internal(error.to_string()).into_response(),
    };

    // A failure here answers non-2xx on purpose: Slack shows the user that the
    // shortcut did not work, instead of nothing happening.
    match state
        .slack_web
        .open_view(&install.bot_token, &trigger_id, &view)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => ApiError::internal(format!(
            "could not open the HiveMind capture modal: {error}"
        ))
        .into_response(),
    }
}

async fn submit_capture_modal(
    store: SlackAppStore,
    install: &SlackWorkspaceInstall,
    submission: ViewSubmission,
) -> Response {
    let capture = match capture_from_modal_submission(
        &install.team_id,
        &submission.user.id,
        &submission.view,
    ) {
        Ok(capture) => capture,
        Err(SlackModalSubmissionError::Fields(errors)) => {
            return (
                StatusCode::OK,
                Json(json!({ "response_action": "errors", "errors": errors })),
            )
                .into_response();
        }
        Err(SlackModalSubmissionError::Malformed(reason)) => {
            return ApiError::validation(reason).into_response();
        }
    };

    // An empty 200 closes the modal.
    match tokio::task::spawn_blocking(move || store.enqueue_capture(capture)).await {
        Ok(Ok(_)) => StatusCode::OK.into_response(),
        Ok(Err(error)) => ApiError::internal(error.to_string()).into_response(),
        Err(error) => ApiError::internal(error.to_string()).into_response(),
    }
}
