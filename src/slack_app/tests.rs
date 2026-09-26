// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::events::{EventSource, TenantId};
use crate::ledger::{InMemoryEventLedger, TenantScopedLedger};

#[test]
fn oauth_url_escapes_query_values() {
    let url = slack_oauth_install_url("123.abc", "https://local/callback", "state with space")
        .expect("url builds");

    assert!(url.contains("client_id=123.abc"));
    assert!(url.contains("redirect_uri=https%3A%2F%2Flocal%2Fcallback"));
    assert!(url.contains("state=state%20with%20space"));
}

#[test]
fn capture_processing_uses_mapping_when_present() {
    let scratch = std::env::temp_dir().join(format!("hivemind-slack-app-{}", Uuid::new_v4()));
    let store = SlackAppStore::new(&scratch);
    store
        .install_workspace(SlackWorkspaceInstall {
            team_id: "T123".to_owned(),
            team_name: "Example".to_owned(),
            bot_token: generated_test_secret("bot"),
            signing_secret: generated_test_secret("signing"),
            hivemind_url: "http://127.0.0.1:8787".to_owned(),
            reaction_emoji: "hivemind".to_owned(),
            actor_mappings: BTreeMap::from([("U111".to_owned(), "actor:alice".to_owned())]),
        })
        .expect("install succeeds");

    let ledger = InMemoryEventLedger::default();
    let outcome = process_capture(&ledger, &store, &capture()).expect("capture succeeds");
    assert!(matches!(outcome, SlackIngestOutcome::Imported { .. }));
    let events = ledger.read(0, 100).expect("events read");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal exists");
    assert_eq!(proposal.actor_id, "actor:alice");
    assert_eq!(proposal.source, EventSource::Slack);
    assert_eq!(
        proposal.source_ref.as_deref(),
        Some("https://example.slack.com/archives/C456/p1715970800000100")
    );

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn reaction_capture_requires_configured_emoji() {
    let scratch = std::env::temp_dir().join(format!("hivemind-slack-app-{}", Uuid::new_v4()));
    let store = SlackAppStore::new(&scratch);
    store
        .install_workspace(SlackWorkspaceInstall {
            team_id: "T123".to_owned(),
            team_name: "Example".to_owned(),
            bot_token: generated_test_secret("bot"),
            signing_secret: generated_test_secret("signing"),
            hivemind_url: "http://127.0.0.1:8787".to_owned(),
            reaction_emoji: "hivemind".to_owned(),
            actor_mappings: BTreeMap::new(),
        })
        .expect("install succeeds");

    let ledger = InMemoryEventLedger::default();
    let mut capture = capture();
    capture.surface = SlackCaptureSurface::Reaction;
    capture.reaction_emoji = Some("eyes".to_owned());

    let error =
        process_capture(&ledger, &store, &capture).expect_err("wrong emoji should be rejected");
    assert!(error.to_string().contains("configured trigger"));

    capture.reaction_emoji = Some("hivemind".to_owned());
    process_capture(&ledger, &store, &capture).expect("configured emoji captures");

    let _ = fs::remove_dir_all(scratch);
}

/// Guards the multi-tenant drain path used by the HTTP API's background
/// drain loop (`api::slack::try_spawn_drain_loop`): each queued capture
/// must be resolved to — and written into — its OWN team's ledger, never
/// another team's. `process_capture`/`import_slack_thread` write via
/// `Commands::new_with_provenance`, whose `CommandContext` defaults to
/// `TenantId::local()`; without a per-capture `TenantScopedLedger` wrapper
/// around whatever ledger the resolver returns, this would silently
/// collapse every capture into one tenant regardless of `team_id`.
#[test]
fn drain_queue_multi_tenant_resolves_each_capture_to_its_own_tenant() {
    let scratch = std::env::temp_dir().join(format!("hivemind-slack-app-{}", Uuid::new_v4()));
    let store = SlackAppStore::new(&scratch);
    store
        .install_workspace(SlackWorkspaceInstall {
            team_id: "TA".to_owned(),
            team_name: "Team A".to_owned(),
            bot_token: generated_test_secret("bot"),
            signing_secret: generated_test_secret("signing"),
            hivemind_url: "http://127.0.0.1:8787".to_owned(),
            reaction_emoji: "hivemind".to_owned(),
            actor_mappings: BTreeMap::new(),
        })
        .expect("install A succeeds");
    store
        .install_workspace(SlackWorkspaceInstall {
            team_id: "TB".to_owned(),
            team_name: "Team B".to_owned(),
            bot_token: generated_test_secret("bot"),
            signing_secret: generated_test_secret("signing"),
            hivemind_url: "http://127.0.0.1:8787".to_owned(),
            reaction_emoji: "hivemind".to_owned(),
            actor_mappings: BTreeMap::new(),
        })
        .expect("install B succeeds");

    let mut capture_a = capture();
    capture_a.team_id = "TA".to_owned();
    capture_a.permalink = "https://example.slack.com/archives/CA/pA".to_owned();
    store.enqueue_capture(capture_a).expect("enqueue A");

    let mut capture_b = capture();
    capture_b.team_id = "TB".to_owned();
    capture_b.permalink = "https://example.slack.com/archives/CB/pB".to_owned();
    store.enqueue_capture(capture_b).expect("enqueue B");

    let ledger_a = InMemoryEventLedger::default();
    let ledger_b = InMemoryEventLedger::default();

    let report = store
        .drain_queue_multi_tenant(|team_id| {
            let tenant_id = TenantId::new(team_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?;
            match team_id {
                "TA" => Ok(TenantScopedLedger::new(&ledger_a, tenant_id)),
                "TB" => Ok(TenantScopedLedger::new(&ledger_b, tenant_id)),
                other => panic!("unexpected team_id in test: {other}"),
            }
        })
        .expect("multi-tenant drain succeeds");

    assert_eq!(report.processed_count, 2);
    assert_eq!(report.failed_count, 0);
    assert_eq!(report.queued_after, 0);

    let events_a = ledger_a
        .read_for_tenant(&TenantId::new("TA").unwrap(), 0, 100)
        .expect("ledger A read");
    let events_b = ledger_b
        .read_for_tenant(&TenantId::new("TB").unwrap(), 0, 100)
        .expect("ledger B read");
    assert!(events_a
        .iter()
        .any(|event| event.event_type == EventType::DecisionProposed));
    assert!(events_b
        .iter()
        .any(|event| event.event_type == EventType::DecisionProposed));

    // Cross-tenant isolation: neither ledger saw the other team's capture.
    assert!(events_a.iter().all(
        |event| event.source_ref.as_deref() != Some("https://example.slack.com/archives/CB/pB")
    ));
    assert!(events_b.iter().all(
        |event| event.source_ref.as_deref() != Some("https://example.slack.com/archives/CA/pA")
    ));

    let _ = fs::remove_dir_all(scratch);
}

// ---------------------------------------------------------------------------
// Capture modal (message shortcut): view out, submission back.
// ---------------------------------------------------------------------------

fn modal_context() -> SlackCaptureModalContext {
    SlackCaptureModalContext {
        channel_id: "C456".to_owned(),
        message_ts: "1715970801.000200".to_owned(),
        thread_ts: "1715970800.000100".to_owned(),
        evidence: message_evidence("1715970801.000200", "U777", "  we should use Postgres  "),
    }
}

/// The `view` object of a `view_submission`, with `inputs` as the typed text
/// per `block_id` (an input left empty is simply not listed, mirroring how
/// Slack reports `value: null`).
fn submitted_view(private_metadata: &str, inputs: &[(&str, &str)]) -> Value {
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
        "callback_id": CAPTURE_MODAL_CALLBACK_ID,
        "private_metadata": private_metadata,
        "state": {"values": values}
    })
}

fn modal_metadata(context: &SlackCaptureModalContext) -> String {
    let view = capture_message_modal(context).expect("modal builds");
    view["private_metadata"]
        .as_str()
        .expect("private_metadata is a string")
        .to_owned()
}

#[test]
fn capture_modal_carries_the_message_through_private_metadata() {
    let context = modal_context();
    let view = capture_message_modal(&context).expect("modal builds");

    assert_eq!(view["type"], "modal");
    assert_eq!(view["callback_id"], CAPTURE_MODAL_CALLBACK_ID);
    // Slack requires a string here; the CLI-only descriptor's object form is
    // not something `views.open` would accept.
    let metadata = view["private_metadata"]
        .as_str()
        .expect("private_metadata is a string");
    assert!(metadata.len() <= 3000);
    let round_tripped: SlackCaptureModalContext =
        serde_json::from_str(metadata).expect("metadata parses back");
    assert_eq!(round_tripped, context);
    assert_eq!(
        context.evidence, "1715970801.000200 U777: we should use Postgres",
        "evidence is `ts author: text` with the text trimmed"
    );

    let blocks = view["blocks"].as_array().expect("blocks");
    let layout: Vec<(&str, bool)> = blocks
        .iter()
        .map(|block| {
            (
                block["block_id"].as_str().expect("block_id"),
                block["optional"].as_bool().expect("optional"),
            )
        })
        .collect();
    assert_eq!(
        layout,
        vec![
            ("title", false),
            ("rationale", false),
            ("options", false),
            ("chosen", true),
            ("topics", true),
        ]
    );
}

#[test]
fn capture_modal_marks_a_message_too_long_for_private_metadata() {
    // Quotes and newlines double in size when JSON-escaped, and `é` is two
    // bytes: the cut has to respect both, and land on a character boundary.
    let mut context = modal_context();
    context.evidence = "\"é\n".repeat(4000);
    let metadata = modal_metadata(&context);

    assert!(
        metadata.len() <= 3000,
        "fits Slack's private_metadata limit"
    );
    let carried: SlackCaptureModalContext =
        serde_json::from_str(&metadata).expect("metadata parses back");
    assert!(carried.evidence.len() < context.evidence.len());
    assert!(
        carried.evidence.contains("[truncated"),
        "a cut message says so instead of looking complete"
    );
    // The cut keeps as much of the message as fits — not just the marker —
    // and stops within one character (at most 2 escaped bytes here) of the limit.
    assert!(
        carried.evidence.chars().count() > 1000,
        "kept {} chars",
        carried.evidence.chars().count()
    );
    assert!(
        metadata.len() >= 3000 - 2,
        "the cut wastes room: {} of 3000 bytes used",
        metadata.len()
    );
    // Bytes 0..8 are two whole `"é\n` groups, so 8 is a character boundary.
    assert!(carried
        .evidence
        .starts_with(context.evidence.get(..8).expect("boundary")));

    // A message that fits is carried whole, with no marker.
    let short = modal_metadata(&modal_context());
    let carried: SlackCaptureModalContext = serde_json::from_str(&short).expect("parses");
    assert_eq!(carried, modal_context());
}

#[test]
fn modal_submission_becomes_a_message_action_capture() {
    let metadata = modal_metadata(&modal_context());
    let view = submitted_view(
        &metadata,
        &[
            ("title", "  Use Postgres  "),
            ("rationale", "Concurrent writers need it"),
            ("options", "SQLite, Postgres | DuckDB"),
            ("chosen", "postgres"),
        ],
    );

    let capture =
        capture_from_modal_submission("T123", "U999", &view).expect("valid submission captures");

    assert_eq!(capture.team_id, "T123");
    assert_eq!(capture.user_id, "U999", "the submitter is the actor");
    assert_eq!(capture.surface, SlackCaptureSurface::MessageAction);
    assert_eq!(capture.reaction_emoji, None);
    assert_eq!(capture.channel_id, "C456");
    assert_eq!(capture.message_ts, "1715970801.000200");
    assert_eq!(capture.thread_ts, "1715970800.000100");
    // One decision per thread: same key the mention and reaction surfaces use.
    assert_eq!(capture.permalink, "slack://T123/C456/1715970800.000100");
    assert_eq!(capture.title, "Use Postgres");
    assert_eq!(capture.rationale, "Concurrent writers need it");
    assert_eq!(capture.option_labels, vec!["SQLite", "Postgres", "DuckDB"]);
    assert_eq!(
        capture.chosen_option_label.as_deref(),
        Some("Postgres"),
        "the chosen option matches case-insensitively and takes the option's own spelling"
    );
    assert_eq!(capture.topic_keys, vec!["slack"], "topics default to slack");
    assert_eq!(capture.thread_text, modal_context().evidence);
}

#[test]
fn modal_submission_without_a_chosen_option_leaves_the_decision_open() {
    let view = submitted_view(
        &modal_metadata(&modal_context()),
        &[
            ("title", "Pick a database"),
            ("rationale", "Still evaluating"),
            ("options", "SQLite, Postgres"),
            ("topics", "storage, infra"),
        ],
    );

    let capture = capture_from_modal_submission("T123", "U999", &view).expect("captures");

    assert_eq!(capture.chosen_option_label, None);
    assert_eq!(capture.topic_keys, vec!["storage", "infra"]);
}

#[test]
fn modal_submission_reports_every_invalid_field_at_once() {
    let view = submitted_view(
        &modal_metadata(&modal_context()),
        &[("title", "   "), ("options", "a, b"), ("chosen", "c")],
    );

    let error = capture_from_modal_submission("T123", "U999", &view)
        .expect_err("invalid submission is rejected");

    let SlackModalSubmissionError::Fields(errors) = error else {
        panic!("expected field errors, got {error:?}");
    };
    assert_eq!(
        errors.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["chosen", "rationale", "title"],
        "the blank title, the missing rationale and the unmatched choice are all reported"
    );

    let view = submitted_view(
        &modal_metadata(&modal_context()),
        &[("title", "t"), ("rationale", "r"), ("options", " , | ")],
    );
    let error = capture_from_modal_submission("T123", "U999", &view).expect_err("no options");
    assert!(matches!(
        error,
        SlackModalSubmissionError::Fields(ref errors) if errors.contains_key("options")
    ));
}

#[test]
fn modal_submission_that_is_not_ours_or_lost_its_metadata_is_malformed() {
    let mut foreign = submitted_view(&modal_metadata(&modal_context()), &[]);
    foreign["callback_id"] = json!("someone_elses_modal");
    assert!(matches!(
        capture_from_modal_submission("T123", "U999", &foreign),
        Err(SlackModalSubmissionError::Malformed(_))
    ));

    let garbled = submitted_view("not json", &[("title", "t")]);
    assert!(matches!(
        capture_from_modal_submission("T123", "U999", &garbled),
        Err(SlackModalSubmissionError::Malformed(_))
    ));
}

#[test]
fn modal_capture_is_accepted_by_the_queue_and_written_under_the_submitter() {
    let scratch = std::env::temp_dir().join(format!("hivemind-slack-app-{}", Uuid::new_v4()));
    let store = SlackAppStore::new(&scratch);
    store
        .install_workspace(SlackWorkspaceInstall {
            team_id: "T123".to_owned(),
            team_name: "Example".to_owned(),
            bot_token: generated_test_secret("bot"),
            signing_secret: generated_test_secret("signing"),
            hivemind_url: "http://127.0.0.1:8787".to_owned(),
            reaction_emoji: "hivemind".to_owned(),
            actor_mappings: BTreeMap::new(),
        })
        .expect("install succeeds");
    let view = submitted_view(
        &modal_metadata(&modal_context()),
        &[
            ("title", "Use Postgres"),
            ("rationale", "Concurrent writers need it"),
            ("options", "SQLite, Postgres"),
            ("chosen", "Postgres"),
        ],
    );
    let capture = capture_from_modal_submission("T123", "U999", &view).expect("captures");

    store.enqueue_capture(capture).expect("queue accepts it");

    let ledger = InMemoryEventLedger::default();
    let report = store.drain_queue(&ledger).expect("drain succeeds");
    assert_eq!(report.processed_count, 1);
    let events = ledger.read(0, 100).expect("events read");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal exists");
    assert_eq!(proposal.actor_id, "slack:T123:U999");
    assert_eq!(
        proposal.source_ref.as_deref(),
        Some("slack://T123/C456/1715970800.000100")
    );

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn manifest_gives_interactivity_its_own_url_and_defaults_it_to_the_request_url() {
    let manifest = slack_app_manifest(
        "https://h.example/v1/slack/commands",
        Some("https://h.example/v1/slack/events"),
        Some("https://h.example/v1/slack/interactivity"),
        None,
    )
    .expect("manifest builds");
    assert_eq!(
        manifest["settings"]["interactivity"]["request_url"],
        "https://h.example/v1/slack/interactivity"
    );
    assert_eq!(
        manifest["features"]["slash_commands"][0]["url"],
        "https://h.example/v1/slack/commands"
    );
    assert_eq!(
        manifest["features"]["shortcuts"][0]["callback_id"],
        CAPTURE_SHORTCUT_CALLBACK_ID
    );

    let local = slack_app_manifest("https://tunnel.example/slack", None, None, None)
        .expect("manifest builds");
    assert_eq!(
        local["settings"]["interactivity"]["request_url"],
        "https://tunnel.example/slack"
    );
}

fn capture() -> SlackCaptureRequest {
    SlackCaptureRequest {
        team_id: "T123".to_owned(),
        user_id: "U111".to_owned(),
        channel_id: "C456".to_owned(),
        message_ts: "1715970800.000100".to_owned(),
        thread_ts: "1715970800.000100".to_owned(),
        permalink: "https://example.slack.com/archives/C456/p1715970800000100".to_owned(),
        surface: SlackCaptureSurface::MessageAction,
        reaction_emoji: None,
        title: "Use the local Slack app".to_owned(),
        rationale: "It preserves reviewed human decisions before hosted service work".to_owned(),
        topic_keys: vec!["slack".to_owned(), "integrations".to_owned()],
        option_labels: vec!["local-first".to_owned(), "hosted-first".to_owned()],
        chosen_option_label: Some("local-first".to_owned()),
        thread_text: "Decision reviewed in Slack".to_owned(),
    }
}

fn generated_test_secret(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}
