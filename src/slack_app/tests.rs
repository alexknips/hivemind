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
        author_user_id: Some("U777".to_owned()),
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
    assert_eq!(capture.user_id, "U999", "the submitter is the recorder");
    assert_eq!(
        capture.decided_by_user_id.as_deref(),
        Some("U777"),
        "the shortcut message's author is the decider, not the submitter"
    );
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

/// A scratch store with one installed workspace, `T123`, mapping `actor_mappings`.
fn installed_store(actor_mappings: BTreeMap<String, String>) -> (PathBuf, SlackAppStore) {
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
            actor_mappings,
        })
        .expect("install succeeds");
    (scratch, store)
}

/// The actors of the events a drained capture wrote: the recorder's (`evidence`, `proposed`)
/// and, when the decision was decided, whoever accepted it.
struct CaptureActors {
    evidence: String,
    proposed: String,
    accepted: Vec<String>,
}

fn drain_capture_actors(store: &SlackAppStore) -> CaptureActors {
    let ledger = InMemoryEventLedger::default();
    let report = store.drain_queue(&ledger).expect("drain succeeds");
    assert_eq!(report.processed_count, 1);
    let events = ledger.read(0, 100).expect("events read");
    let actor_of = |event_type: EventType| {
        events
            .iter()
            .find(|event| event.event_type == event_type)
            .map(|event| event.actor_id.clone())
            .expect("the capture wrote this event")
    };
    CaptureActors {
        evidence: actor_of(EventType::EvidenceRecorded),
        proposed: actor_of(EventType::DecisionProposed),
        accepted: events
            .iter()
            .filter(|event| event.event_type == EventType::DecisionAccepted)
            .map(|event| event.actor_id.clone())
            .collect(),
    }
}

fn modal_capture(context: &SlackCaptureModalContext, chosen: Option<&str>) -> SlackCaptureRequest {
    let mut inputs = vec![
        ("title", "Use Postgres"),
        ("rationale", "Concurrent writers need it"),
        ("options", "SQLite, Postgres"),
    ];
    inputs.extend(chosen.map(|chosen| ("chosen", chosen)));
    let view = submitted_view(&modal_metadata(context), &inputs);
    capture_from_modal_submission("T123", "U999", &view).expect("captures")
}

#[test]
fn modal_capture_is_recorded_by_the_submitter_and_decided_by_the_message_author() {
    let (scratch, store) = installed_store(BTreeMap::new());
    store
        .enqueue_capture(modal_capture(&modal_context(), Some("Postgres")))
        .expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.evidence, "slack:T123:U999", "the submitter records");
    assert_eq!(actors.proposed, "slack:T123:U999", "the submitter records");
    assert_eq!(
        actors.accepted,
        vec!["slack:T123:U777"],
        "the author of the shortcut's message decided; the submitter did not accept it"
    );

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn modal_capture_by_the_message_author_is_recorded_and_decided_by_them() {
    let (scratch, store) = installed_store(BTreeMap::new());
    let context = SlackCaptureModalContext {
        author_user_id: Some("U999".to_owned()),
        ..modal_context()
    };
    store
        .enqueue_capture(modal_capture(&context, Some("Postgres")))
        .expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.proposed, "slack:T123:U999");
    assert_eq!(actors.accepted, vec!["slack:T123:U999"]);

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn modal_capture_of_an_app_message_has_no_other_decider_so_the_submitter_decides() {
    let (scratch, store) = installed_store(BTreeMap::new());
    let context = SlackCaptureModalContext {
        author_user_id: None,
        ..modal_context()
    };
    let capture = modal_capture(&context, Some("Postgres"));
    assert_eq!(capture.decided_by_user_id, None);
    store.enqueue_capture(capture).expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.proposed, "slack:T123:U999");
    assert_eq!(actors.accepted, vec!["slack:T123:U999"]);

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn modal_capture_with_no_chosen_option_records_a_proposal_nobody_has_accepted() {
    let (scratch, store) = installed_store(BTreeMap::new());
    store
        .enqueue_capture(modal_capture(&modal_context(), None))
        .expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.proposed, "slack:T123:U999");
    assert!(
        actors.accepted.is_empty(),
        "nothing was chosen, so nobody decided: {:?}",
        actors.accepted
    );

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn reaction_capture_is_recorded_by_the_reactor_and_decided_by_the_author() {
    let (scratch, store) = installed_store(BTreeMap::new());
    store
        .enqueue_capture(SlackCaptureRequest {
            user_id: "U111".to_owned(),
            decided_by_user_id: Some("U222".to_owned()),
            surface: SlackCaptureSurface::Reaction,
            reaction_emoji: Some("hivemind".to_owned()),
            ..capture()
        })
        .expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.evidence, "slack:T123:U111", "the reactor records");
    assert_eq!(actors.proposed, "slack:T123:U111", "the reactor records");
    assert_eq!(
        actors.accepted,
        vec!["slack:T123:U222"],
        "the message's author decided; the reactor did not accept it"
    );

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn reaction_by_the_author_of_the_message_records_and_decides_as_one_actor() {
    let (scratch, store) = installed_store(BTreeMap::new());
    store
        .enqueue_capture(SlackCaptureRequest {
            user_id: "U111".to_owned(),
            decided_by_user_id: Some("U111".to_owned()),
            surface: SlackCaptureSurface::Reaction,
            reaction_emoji: Some("hivemind".to_owned()),
            ..capture()
        })
        .expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.proposed, "slack:T123:U111");
    assert_eq!(actors.accepted, vec!["slack:T123:U111"]);

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn reaction_capture_of_an_unchosen_decision_names_no_decider() {
    let (scratch, store) = installed_store(BTreeMap::new());
    store
        .enqueue_capture(SlackCaptureRequest {
            user_id: "U111".to_owned(),
            decided_by_user_id: Some("U222".to_owned()),
            chosen_option_label: None,
            surface: SlackCaptureSurface::Reaction,
            reaction_emoji: Some("hivemind".to_owned()),
            ..capture()
        })
        .expect("a decider with nothing chosen is not an error");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.proposed, "slack:T123:U111");
    assert!(actors.accepted.is_empty(), "{:?}", actors.accepted);

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn recorder_and_decider_are_each_mapped_through_the_installs_actor_mappings() {
    let (scratch, store) = installed_store(BTreeMap::from([
        ("U111".to_owned(), "human:reactor".to_owned()),
        ("U222".to_owned(), "human:author".to_owned()),
    ]));
    store
        .enqueue_capture(SlackCaptureRequest {
            user_id: "U111".to_owned(),
            decided_by_user_id: Some("U222".to_owned()),
            surface: SlackCaptureSurface::Reaction,
            reaction_emoji: Some("hivemind".to_owned()),
            ..capture()
        })
        .expect("queue accepts it");

    let actors = drain_capture_actors(&store);

    assert_eq!(actors.proposed, "human:reactor");
    assert_eq!(actors.accepted, vec!["human:author"]);

    let _ = fs::remove_dir_all(scratch);
}

#[test]
fn a_blank_decider_is_refused_by_the_queue() {
    let (scratch, store) = installed_store(BTreeMap::new());

    let error = store
        .enqueue_capture(SlackCaptureRequest {
            decided_by_user_id: Some("  ".to_owned()),
            ..capture()
        })
        .expect_err("a blank decider names nobody");

    assert!(error.to_string().contains("decided_by_user_id"), "{error}");

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
        decided_by_user_id: None,
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
