use serde::Deserialize;

use crate::error::CommandError;
use crate::Result;

pub const DEFAULT_SLACK_MENTION: &str = "@hivemind";

#[derive(Debug, Clone, Deserialize)]
pub struct SlackThreadFixture {
    #[serde(alias = "workspace_id")]
    pub team_id: String,
    pub channel_id: String,
    pub thread_ts: String,
    pub messages: Vec<SlackMessageFixture>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackMessageFixture {
    #[serde(alias = "user")]
    pub user_id: String,
    pub ts: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackDecisionDraft {
    pub actor_id: String,
    pub source_ref: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub option_labels: Vec<String>,
    pub chosen_option_label: Option<String>,
    pub thread_context: String,
}

pub fn parse_slack_thread_fixture(input: &str) -> Result<SlackThreadFixture> {
    serde_json::from_str(input).map_err(|error| {
        CommandError::Validation(format!("invalid slack thread fixture: {error}")).into()
    })
}

pub fn extract_slack_decision_draft(
    thread: &SlackThreadFixture,
    mention: &str,
) -> Result<SlackDecisionDraft> {
    validate_thread(thread)?;
    let mention = mention.trim();
    require_non_empty("mention", mention)?;

    if !thread
        .messages
        .iter()
        .any(|message| message.text.contains(mention))
    {
        return Err(CommandError::Validation(format!(
            "slack thread is missing required mention '{mention}'"
        ))
        .into());
    }

    let markers = parse_decision_markers(thread)?;
    let source_ref = slack_thread_source_ref(thread);

    Ok(SlackDecisionDraft {
        actor_id: markers.actor_id,
        source_ref: source_ref.clone(),
        title: markers.title,
        rationale: markers.rationale,
        topic_keys: markers.topic_keys,
        option_labels: markers.option_labels,
        chosen_option_label: markers.chosen_option_label,
        thread_context: render_thread_context(thread, &source_ref),
    })
}

pub fn slack_thread_source_ref(thread: &SlackThreadFixture) -> String {
    format!(
        "slack://{}/{}/{}",
        thread.team_id, thread.channel_id, thread.thread_ts
    )
}

fn validate_thread(thread: &SlackThreadFixture) -> Result<()> {
    require_non_empty("team_id", &thread.team_id)?;
    require_non_empty("channel_id", &thread.channel_id)?;
    require_non_empty("thread_ts", &thread.thread_ts)?;
    if thread.messages.is_empty() {
        return Err(CommandError::Validation("messages must not be empty".to_owned()).into());
    }

    for message in &thread.messages {
        require_non_empty("message.user_id", &message.user_id)?;
        require_non_empty("message.ts", &message.ts)?;
        require_non_empty("message.text", &message.text)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlackDecisionMarkers {
    actor_id: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    option_labels: Vec<String>,
    chosen_option_label: Option<String>,
}

fn parse_decision_markers(thread: &SlackThreadFixture) -> Result<SlackDecisionMarkers> {
    let mut actor_user_id = None;
    let mut title = None;
    let mut rationale = None;
    let mut topic_keys = Vec::new();
    let mut option_labels = Vec::new();
    let mut chosen_option_label = None;

    for message in &thread.messages {
        for line in message.text.lines() {
            let line = line.trim();
            if let Some(value) = marker_value(line, "Decision") {
                title = Some(value.to_owned());
                actor_user_id.get_or_insert_with(|| message.user_id.clone());
            } else if let Some(value) = marker_value(line, "Rationale") {
                rationale = Some(value.to_owned());
            } else if let Some(value) = marker_value(line, "Options") {
                option_labels = split_marker_list(value);
            } else if let Some(value) =
                marker_value(line, "Topics").or_else(|| marker_value(line, "Topic"))
            {
                topic_keys = split_marker_list(value);
            } else if let Some(value) =
                marker_value(line, "Chosen").or_else(|| marker_value(line, "Chose"))
            {
                chosen_option_label = Some(value.to_owned());
            }
        }
    }

    let title = required_marker(title, "Decision")?;
    let rationale = required_marker(rationale, "Rationale")?;
    if option_labels.is_empty() {
        return Err(CommandError::Validation(
            "Options marker must contain at least one option".to_owned(),
        )
        .into());
    }
    if topic_keys.is_empty() {
        topic_keys.push("slack".to_owned());
    }

    let actor_user_id = actor_user_id.ok_or_else(|| {
        CommandError::Validation("Decision marker must identify an author".to_owned())
    })?;
    let actor_id = format!("slack:{}:{}", thread.team_id, actor_user_id);

    Ok(SlackDecisionMarkers {
        actor_id,
        title,
        rationale,
        topic_keys,
        option_labels,
        chosen_option_label,
    })
}

fn marker_value<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case(marker)
        .then_some(value.trim())
        .filter(|value| !value.is_empty())
}

fn split_marker_list(value: &str) -> Vec<String> {
    value
        .split([',', '|'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn required_marker(value: Option<String>, marker: &'static str) -> Result<String> {
    value.ok_or_else(|| {
        CommandError::Validation(format!(
            "{marker} marker is required for slack thread draft extraction"
        ))
        .into()
    })
}

fn render_thread_context(thread: &SlackThreadFixture, source_ref: &str) -> String {
    let mut context = format!("Slack thread {source_ref}\n");
    for message in &thread.messages {
        context.push_str(&format!(
            "{} {}: {}\n",
            message.ts,
            message.user_id,
            message.text.trim()
        ));
    }
    context
}

fn require_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_mentioned_thread_without_writing_events() {
        let thread = parse_slack_thread_fixture(include_str!(
            "../tests/fixtures/slack/thread_with_mention.json"
        ))
        .expect("fixture parses");

        let draft =
            extract_slack_decision_draft(&thread, DEFAULT_SLACK_MENTION).expect("thread extracts");

        assert_eq!(draft.actor_id, "slack:T123:U111");
        assert_eq!(draft.source_ref, "slack://T123/C456/1715970800.000100");
        assert_eq!(draft.title, "Use local fake Slack ingest first");
        assert_eq!(
            draft.rationale,
            "It verifies mention-gated capture without Slack credentials"
        );
        assert_eq!(draft.topic_keys, vec!["integrations", "slack"]);
        assert_eq!(
            draft.option_labels,
            vec!["Build local fixture ingest", "Wait for hosted Slack app"]
        );
        assert_eq!(
            draft.chosen_option_label.as_deref(),
            Some("Build local fixture ingest")
        );
        assert!(draft.thread_context.contains("Thread context is safe"));
    }

    #[test]
    fn rejects_thread_without_mention() {
        let thread = parse_slack_thread_fixture(include_str!(
            "../tests/fixtures/slack/thread_without_mention.json"
        ))
        .expect("fixture parses");

        let error = extract_slack_decision_draft(&thread, DEFAULT_SLACK_MENTION)
            .expect_err("thread without mention rejected");

        assert!(error.to_string().contains("missing required mention"));
    }
}
