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
