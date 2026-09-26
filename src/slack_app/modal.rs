//! The "Capture this thread as a decision" message shortcut's modal: the view
//! `views.open` shows, and the return trip from a submitted `view_submission`
//! back to a [`SlackCaptureRequest`]. Pure JSON <-> struct functions with no
//! I/O — request-signature verification and the outbound Slack call live in
//! the HTTP front door ([`crate::api`]), so this stays deterministic and
//! LLM-free per AGENTS.md's three-layer separation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{SlackCaptureRequest, SlackCaptureSurface};
use crate::error::{CliError, CommandError};
use crate::ingest::{slack_source_ref, split_marker_list, DEFAULT_SLACK_TOPIC};
use crate::Result;

/// `callback_id` of the message shortcut declared in [`super::slack_app_manifest`].
pub const CAPTURE_SHORTCUT_CALLBACK_ID: &str = "hivemind_capture_thread";
/// `callback_id` of the modal the shortcut opens.
pub const CAPTURE_MODAL_CALLBACK_ID: &str = "hivemind_capture_decision";

const TITLE_BLOCK: &str = "title";
const RATIONALE_BLOCK: &str = "rationale";
const OPTIONS_BLOCK: &str = "options";
const CHOSEN_BLOCK: &str = "chosen";
const TOPICS_BLOCK: &str = "topics";
const INPUT_ACTION_ID: &str = "value";

/// Slack rejects a view whose `private_metadata` is longer than 3000
/// characters. Bytes are never fewer than characters, so bounding the byte
/// length is the conservative reading of that limit.
const PRIVATE_METADATA_MAX_BYTES: usize = 3000;
const TRUNCATION_MARKER: &str =
    "\n[truncated: the message was longer than a Slack modal can carry; the source_ref points at the original]";

/// What the shortcut hands the modal so the submission can be turned into a
/// capture without a second Slack call: it round-trips through Slack's
/// `private_metadata`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackCaptureModalContext {
    pub channel_id: String,
    pub message_ts: String,
    pub thread_ts: String,
    /// The shortcut message rendered by [`message_evidence`].
    pub evidence: String,
}

/// Why a `view_submission` did not become a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackModalSubmissionError {
    /// One message per offending input, keyed by its `block_id`. Slack shows
    /// them inline and keeps the modal open.
    Fields(BTreeMap<String, String>),
    /// The submission is not this app's capture modal, or its
    /// `private_metadata` did not round-trip.
    Malformed(String),
}

/// Renders one Slack message as a capture's evidence text, in the
/// `{ts} {author}: {text}` line shape `hivemind ingest slack-thread` uses, so
/// who wrote the words, and when, stays in the ledger next to the decision.
pub fn message_evidence(ts: &str, author: &str, text: &str) -> String {
    format!("{ts} {author}: {}", text.trim())
}

/// Builds the `view` for `views.open` from the message the shortcut was
/// invoked on.
pub fn capture_message_modal(context: &SlackCaptureModalContext) -> Result<Value> {
    Ok(capture_modal_view(Value::String(encode_context(context)?)))
}

pub(super) fn capture_modal_view(private_metadata: Value) -> Value {
    json!({
        "type": "modal",
        "callback_id": CAPTURE_MODAL_CALLBACK_ID,
        "title": {"type": "plain_text", "text": "HiveMind"},
        "submit": {"type": "plain_text", "text": "Capture"},
        "close": {"type": "plain_text", "text": "Cancel"},
        "private_metadata": private_metadata,
        "blocks": [
            input_block(TITLE_BLOCK, "Decision", None, false, false),
            input_block(
                RATIONALE_BLOCK,
                "Rationale",
                Some("Why this, over the alternatives?"),
                false,
                true,
            ),
            input_block(
                OPTIONS_BLOCK,
                "Options considered",
                Some("Comma-separated."),
                false,
                false,
            ),
            input_block(
                CHOSEN_BLOCK,
                "Chosen option",
                Some("Must match one of the options. Leave empty if nothing is decided yet."),
                true,
                false,
            ),
            input_block(
                TOPICS_BLOCK,
                "Topics",
                Some("Comma-separated. Defaults to 'slack'."),
                true,
                false,
            )
        ]
    })
}

fn input_block(
    block_id: &str,
    label: &str,
    hint: Option<&str>,
    optional: bool,
    multiline: bool,
) -> Value {
    let mut block = json!({
        "type": "input",
        "block_id": block_id,
        "optional": optional,
        "label": {"type": "plain_text", "text": label},
        "element": {
            "type": "plain_text_input",
            "action_id": INPUT_ACTION_ID,
            "multiline": multiline
        }
    });
    if let (Some(hint), Some(fields)) = (hint, block.as_object_mut()) {
        fields.insert(
            "hint".to_owned(),
            json!({"type": "plain_text", "text": hint}),
        );
    }
    block
}

/// Serializes `context` for `private_metadata`. A message too long to fit is
/// cut with an explicit marker in the evidence text — never silently — and
/// the capture's `source_ref` still points at the original.
fn encode_context(context: &SlackCaptureModalContext) -> Result<String> {
    let whole = encode_with_evidence(context, &context.evidence)?;
    if whole.len() <= PRIVATE_METADATA_MAX_BYTES {
        return Ok(whole);
    }

    // Keep the longest prefix of the evidence (counted in characters, so a
    // multibyte one is never split) that still fits next to the marker. JSON
    // escaping makes a character cost between one and six bytes, so the fit
    // cannot be computed from lengths alone; the encoded size only grows with
    // the prefix, so bisect on it.
    let total = context.evidence.chars().count();
    let mut fitting = encode_with_prefix(context, 0)?;
    if fitting.len() > PRIVATE_METADATA_MAX_BYTES {
        return Err(CommandError::Validation(
            "slack message identifiers do not fit in a modal".to_owned(),
        )
        .into());
    }
    // `kept` characters fit; `total` (the whole message, marker on top) does not.
    let (mut kept, mut too_many) = (0, total);
    while too_many - kept > 1 {
        let middle = kept + (too_many - kept) / 2;
        let candidate = encode_with_prefix(context, middle)?;
        if candidate.len() <= PRIVATE_METADATA_MAX_BYTES {
            kept = middle;
            fitting = candidate;
        } else {
            too_many = middle;
        }
    }
    Ok(fitting)
}

/// `context` with its evidence cut to `chars` characters and the truncation
/// marker appended.
fn encode_with_prefix(context: &SlackCaptureModalContext, chars: usize) -> Result<String> {
    let prefix: String = context.evidence.chars().take(chars).collect();
    encode_with_evidence(context, &format!("{prefix}{TRUNCATION_MARKER}"))
}

fn encode_with_evidence(context: &SlackCaptureModalContext, evidence: &str) -> Result<String> {
    let carried = SlackCaptureModalContext {
        channel_id: context.channel_id.clone(),
        message_ts: context.message_ts.clone(),
        thread_ts: context.thread_ts.clone(),
        evidence: evidence.to_owned(),
    };
    serde_json::to_string(&carried).map_err(|error| {
        CliError::InvalidInput(format!("json serialization failed: {error}")).into()
    })
}

/// Turns the `view` of a `view_submission` into a capture, attributed to the
/// Slack user who submitted it. `team_id` is the workspace whose signature
/// was verified — never anything read from the modal itself.
pub fn capture_from_modal_submission(
    team_id: &str,
    user_id: &str,
    view: &Value,
) -> std::result::Result<SlackCaptureRequest, SlackModalSubmissionError> {
    if view.get("callback_id").and_then(Value::as_str) != Some(CAPTURE_MODAL_CALLBACK_ID) {
        return Err(SlackModalSubmissionError::Malformed(
            "view_submission is not the HiveMind capture modal".to_owned(),
        ));
    }
    let context: SlackCaptureModalContext = view
        .get("private_metadata")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .ok_or_else(|| {
            SlackModalSubmissionError::Malformed(
                "capture modal private_metadata is missing or invalid".to_owned(),
            )
        })?;

    let mut errors = BTreeMap::new();
    let title = required_input(view, TITLE_BLOCK, "Enter the decision.", &mut errors);
    let rationale = required_input(view, RATIONALE_BLOCK, "Enter the rationale.", &mut errors);
    let option_labels = input(view, OPTIONS_BLOCK)
        .map(split_marker_list)
        .unwrap_or_default();
    if option_labels.is_empty() {
        errors.insert(
            OPTIONS_BLOCK.to_owned(),
            "Enter at least one option.".to_owned(),
        );
    }
    let chosen_option_label = match input(view, CHOSEN_BLOCK) {
        None => None,
        Some(chosen) => {
            let matched = option_labels
                .iter()
                .find(|option| option.eq_ignore_ascii_case(chosen));
            if matched.is_none() {
                errors.insert(
                    CHOSEN_BLOCK.to_owned(),
                    "Must match one of the options.".to_owned(),
                );
            }
            matched.cloned()
        }
    };
    let mut topic_keys = input(view, TOPICS_BLOCK)
        .map(split_marker_list)
        .unwrap_or_default();
    if topic_keys.is_empty() {
        topic_keys.push(DEFAULT_SLACK_TOPIC.to_owned());
    }

    let (Some(title), Some(rationale), true) = (title, rationale, errors.is_empty()) else {
        return Err(SlackModalSubmissionError::Fields(errors));
    };

    Ok(SlackCaptureRequest {
        team_id: team_id.to_owned(),
        user_id: user_id.to_owned(),
        permalink: slack_source_ref(team_id, &context.channel_id, &context.thread_ts),
        channel_id: context.channel_id,
        message_ts: context.message_ts,
        thread_ts: context.thread_ts,
        surface: SlackCaptureSurface::MessageAction,
        reaction_emoji: None,
        title,
        rationale,
        topic_keys,
        option_labels,
        chosen_option_label,
        thread_text: context.evidence,
    })
}

/// The trimmed text typed into `block_id`, or `None` when it was left empty.
fn input<'a>(view: &'a Value, block_id: &str) -> Option<&'a str> {
    view.pointer(&format!("/state/values/{block_id}/{INPUT_ACTION_ID}/value"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_input(
    view: &Value,
    block_id: &str,
    message: &str,
    errors: &mut BTreeMap<String, String>,
) -> Option<String> {
    let value = input(view, block_id).map(ToOwned::to_owned);
    if value.is_none() {
        errors.insert(block_id.to_owned(), message.to_owned());
    }
    value
}
