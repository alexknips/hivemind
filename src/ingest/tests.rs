// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;

#[test]
fn extracts_mentioned_thread_without_writing_events() {
    let thread = parse_slack_thread_fixture(include_str!(
        "../../tests/fixtures/slack/thread_with_mention.json"
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
        "../../tests/fixtures/slack/thread_without_mention.json"
    ))
    .expect("fixture parses");

    let error = extract_slack_decision_draft(&thread, DEFAULT_SLACK_MENTION)
        .expect_err("thread without mention rejected");

    assert!(error.to_string().contains("missing required mention"));
}
