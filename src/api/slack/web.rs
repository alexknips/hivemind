//! Outbound Slack Web API client — the calls the HTTP front door makes *to*
//! Slack, authenticated with an install's bot token: `views.open` for the
//! capture modal and `conversations.history` to fetch the message a
//! `reaction_added` event points at.
//!
//! Every call carries a short timeout: Slack gives a `trigger_id` about three
//! seconds to be used and expects an event or interaction to be acknowledged
//! within the same window, so a slow Slack must fail fast here instead of
//! holding the request open. Nothing is retried — a failed call is reported
//! to the caller, which decides what the user or the log should see.

use std::time::Duration;

use reqwest::redirect::Policy;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::CliError;

const DEFAULT_SLACK_API_BASE_URL: &str = "https://slack.com/api";
const SLACK_WEB_TIMEOUT: Duration = Duration::from_secs(2);

/// Why an outbound Slack Web API call failed. Never carries the bot token.
#[derive(Debug, thiserror::Error)]
pub(super) enum SlackWebError {
    #[error("slack web api request failed: {0}")]
    Transport(String),
    /// A non-2xx answer — notably `429` when Slack rate-limits the app.
    #[error("slack web api answered HTTP {0}")]
    Http(u16),
    /// Slack answered `ok: false`; carries Slack's error code
    /// (`expired_trigger_id`, `not_in_channel`, `missing_scope`, ...).
    #[error("slack web api error: {0}")]
    Api(String),
    #[error("slack web api response was not understood: {0}")]
    Malformed(String),
}

/// A message as `conversations.history` reports it — only the fields a
/// capture needs.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct SlackMessage {
    pub(super) ts: String,
    #[serde(default)]
    pub(super) user: Option<String>,
    #[serde(default)]
    pub(super) text: String,
    #[serde(default)]
    pub(super) thread_ts: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HistoryResponse {
    #[serde(default)]
    messages: Vec<SlackMessage>,
}

#[derive(Debug, Clone)]
pub(in crate::api) struct SlackWebClient {
    http: reqwest::Client,
    base_url: String,
}

impl SlackWebClient {
    /// `base_url` overrides Slack's public API root (tests point it at a
    /// local stand-in; see `ApiConfig::slack_api_base_url`).
    pub(in crate::api) fn new(base_url: Option<&str>) -> crate::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(SLACK_WEB_TIMEOUT)
            // The bot token rides in a header; Slack's API never redirects.
            .redirect(Policy::none())
            .build()
            .map_err(|error| {
                CliError::InvalidInput(format!("could not build the Slack Web API client: {error}"))
            })?;
        Ok(Self {
            http,
            base_url: base_url
                .unwrap_or(DEFAULT_SLACK_API_BASE_URL)
                .trim_end_matches('/')
                .to_owned(),
        })
    }

    /// `views.open`: shows `view` to the user whose action minted `trigger_id`.
    pub(super) async fn open_view(
        &self,
        bot_token: &str,
        trigger_id: &str,
        view: &Value,
    ) -> Result<(), SlackWebError> {
        let request = self
            .http
            .post(format!("{}/views.open", self.base_url))
            .bearer_auth(bot_token)
            .json(&json!({ "trigger_id": trigger_id, "view": view }));
        self.send(request).await.map(|_| ())
    }

    /// Fetches the single message posted in `channel_id` at exactly `ts`.
    ///
    /// `conversations.history` with `latest = ts` answers with the newest
    /// message at or before `ts`, so the answer only counts when its `ts`
    /// matches. A thread reply never does — history lists top-level messages
    /// only, and a reaction event carries no `thread_ts` to look the reply up
    /// by — and neither does a message deleted since the reaction. Both come
    /// back as `Ok(None)`.
    pub(super) async fn fetch_message(
        &self,
        bot_token: &str,
        channel_id: &str,
        ts: &str,
    ) -> Result<Option<SlackMessage>, SlackWebError> {
        let request = self
            .http
            .post(format!("{}/conversations.history", self.base_url))
            .bearer_auth(bot_token)
            .form(&[
                ("channel", channel_id),
                ("latest", ts),
                ("inclusive", "true"),
                ("limit", "1"),
            ]);
        let payload = self.send(request).await?;
        let history: HistoryResponse = serde_json::from_value(payload)
            .map_err(|error| SlackWebError::Malformed(error.to_string()))?;
        Ok(history
            .messages
            .into_iter()
            .find(|message| message.ts == ts))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, SlackWebError> {
        let response = request
            .send()
            .await
            .map_err(|error| SlackWebError::Transport(error.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SlackWebError::Http(status.as_u16()));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|error| SlackWebError::Malformed(error.without_url().to_string()))?;
        if payload.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(payload)
        } else {
            Err(SlackWebError::Api(
                payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_error")
                    .to_owned(),
            ))
        }
    }
}
