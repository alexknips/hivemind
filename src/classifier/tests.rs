// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;

use std::collections::HashSet;

fn batch_event(turns: serde_json::Value) -> crate::events::Event {
    crate::events::Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: uuid::Uuid::nil(),
        correlation_id: None,
        causation_event_id: None,
        event_type: crate::events::EventType::IngestBatchReceived,
        actor_id: "actor:test".to_owned(),
        source: Default::default(),
        source_ref: None,
        payload: serde_json::json!({ "batch_id": "b1", "turns": turns }),
        ts: None,
    }
}

// --- render_batch_text ---

#[test]
fn render_batch_text_no_turns_is_empty() {
    let event = batch_event(serde_json::json!([]));
    assert_eq!(render_batch_text(&event), "");
}

#[test]
fn render_batch_text_missing_turns_key_is_empty() {
    let event = crate::events::Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: uuid::Uuid::nil(),
        correlation_id: None,
        causation_event_id: None,
        event_type: crate::events::EventType::IngestBatchReceived,
        actor_id: "actor:test".to_owned(),
        source: Default::default(),
        source_ref: None,
        payload: serde_json::json!({ "batch_id": "b1" }),
        ts: None,
    };
    assert_eq!(render_batch_text(&event), "");
}

#[test]
fn render_batch_text_single_turn() {
    let event = batch_event(serde_json::json!([
        { "role": "user", "text": "hello", "truncated": false }
    ]));
    assert_eq!(render_batch_text(&event), "[user] hello\n");
}

#[test]
fn render_batch_text_truncated_appends_marker() {
    let event = batch_event(serde_json::json!([
        { "role": "assistant", "text": "partial response", "truncated": true }
    ]));
    assert_eq!(
        render_batch_text(&event),
        "[assistant] partial response [TRUNCATED]\n"
    );
}

#[test]
fn render_batch_text_multiple_turns_preserves_order() {
    let event = batch_event(serde_json::json!([
        { "role": "user", "text": "first", "truncated": false },
        { "role": "assistant", "text": "second", "truncated": false },
        { "role": "user", "text": "third", "truncated": false },
    ]));
    assert_eq!(
        render_batch_text(&event),
        "[user] first\n[assistant] second\n[user] third\n"
    );
}

#[test]
fn render_batch_text_names_each_turn_by_its_id() {
    let event = batch_event(serde_json::json!([
        { "turn_id": "t-1", "role": "assistant", "text": "which database?", "truncated": false },
        { "turn_id": "t-2", "role": "user", "text": "partial", "truncated": true },
    ]));
    assert_eq!(
        render_batch_text(&event),
        "[assistant turn t-1] which database?\n[user turn t-2] partial [TRUNCATED]\n"
    );
    assert_eq!(turn_ids(&event), vec!["t-1", "t-2"]);
}

#[test]
fn render_batch_text_unknown_role_defaults_to_unknown() {
    let event = batch_event(serde_json::json!([
        { "text": "no role field" }
    ]));
    assert_eq!(render_batch_text(&event), "[unknown] no role field\n");
}

// --- pending-batch filter ---

#[test]
fn pending_batch_filter_excludes_classified_ids() {
    let received = vec![
        PendingBatch {
            batch_id: "a".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 1,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
        PendingBatch {
            batch_id: "b".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 2,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
        PendingBatch {
            batch_id: "c".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 3,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
    ];
    let mut classified: HashSet<String> = HashSet::new();
    classified.insert("b".to_owned());

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].batch_id, "a");
    assert_eq!(pending[1].batch_id, "c");
}

#[test]
fn pending_batch_filter_all_classified_yields_empty() {
    let received = vec![PendingBatch {
        batch_id: "x".to_owned(),
        submitted_at: None,
        actor_id: "actor:test".to_owned(),
        turn_count: 0,
        session_id: String::new(),
        agent_tool: String::new(),
        batch_text: String::new(),
    }];
    let mut classified: HashSet<String> = HashSet::new();
    classified.insert("x".to_owned());

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert!(pending.is_empty());
}

// --- classifier model override ---

#[test]
fn resolve_classifier_model_defaults_when_no_override() {
    assert_eq!(resolve_classifier_model(None), CLASSIFIER_MODEL);
}

#[test]
fn resolve_classifier_model_uses_override_when_present() {
    assert_eq!(
        resolve_classifier_model(Some("claude-sonnet-5".to_owned())),
        "claude-sonnet-5"
    );
}

#[test]
fn pending_batch_filter_no_classified_returns_all() {
    let received = vec![
        PendingBatch {
            batch_id: "p".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 5,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
        PendingBatch {
            batch_id: "q".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 5,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
    ];
    let classified: HashSet<String> = HashSet::new();

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert_eq!(pending.len(), 2);
}

// --- session_agent_actor ---

#[test]
fn session_agent_actor_prefers_already_agent_shaped_token() {
    // A token minted through the agent-token path (hivemind-zdsh.19) is already
    // the stable agent identity; use it as-is instead of synthesizing another one.
    assert_eq!(
        session_agent_actor("agent:gastown:crew", "claude"),
        Some("agent:gastown:crew".to_owned())
    );
}

#[test]
fn session_agent_actor_falls_back_to_tool_for_human_token() {
    // A human's own personal token still runs an agent tool on their behalf;
    // that tool needs an identity distinct from the human (hivemind-zdsh.19).
    assert_eq!(
        session_agent_actor("human:alex@example.com", "claude"),
        Some("agent:claude:hook".to_owned())
    );
}

#[test]
fn session_agent_actor_is_stable_across_different_session_ids() {
    // Same physical agent, two different (synthetic) runs -- the identity must
    // not depend on session_id at all, or the same agent looks like a different
    // actor after every restart (hivemind-zdsh.9).
    let first = session_agent_actor("human:alex@example.com", "codex");
    let second = session_agent_actor("human:alex@example.com", "codex");
    assert_eq!(first, second);
}

#[test]
fn session_agent_actor_none_when_no_tool_and_not_agent_token() {
    assert_eq!(session_agent_actor("human:alex@example.com", ""), None);
}

// --- source turn and question (hivemind-bbnw.8) ---

fn parsed_capture(extra: serde_json::Value) -> CaptureItem {
    let mut capture = serde_json::json!({
        "kind": "decision",
        "title": "Use Postgres for the hosted MVP",
        "rationale": "Several friends write at once",
        "topic_keys": ["hosted-mvp"],
        "evidence_ids": [],
        "options": null,
        "chosen_option": null,
        "extraction_confidence": 0.9,
        "expressed_confidence": null,
        "supersedes_id": null,
        "actor_id": null,
        "blocked_actor_id": null,
        "decision_id": null,
    });
    capture
        .as_object_mut()
        .expect("capture is an object")
        .extend(extra.as_object().expect("extra is an object").clone());
    let response = serde_json::json!({ "captures": [capture] }).to_string();
    parse_capture_response(&response)
        .expect("response parses")
        .remove(0)
}

#[test]
fn the_schema_asks_for_a_source_turn_and_a_question_on_every_capture() {
    let schema = capture_schema();
    let items = &schema["properties"]["captures"]["items"];
    for field in ["source_turn_id", "question"] {
        assert!(
            items["properties"].get(field).is_some(),
            "{field} is a property"
        );
        assert!(
            items["required"]
                .as_array()
                .expect("required list")
                .iter()
                .any(|name| name == field),
            "{field} is required (nullable) under structured outputs"
        );
    }
}

#[test]
fn a_response_without_the_new_fields_still_parses() {
    let capture = parsed_capture(serde_json::json!({}));
    assert_eq!(capture.source_turn_id, None);
    assert_eq!(capture.question, None);
}

#[test]
fn the_evaluator_path_keeps_the_question_but_no_turn() {
    // No batch, so no turn the model could honestly name.
    let capture = parsed_capture(serde_json::json!({
        "source_turn_id": "t-1",
        "question": "Which database should the hosted MVP use?"
    }));
    assert_eq!(capture.source_turn_id, None);
    assert_eq!(
        capture.question.as_deref(),
        Some("Which database should the hosted MVP use?")
    );
}

#[test]
fn a_turn_the_batch_does_not_hold_is_dropped_before_submission() {
    let mut named = parsed_capture(serde_json::json!({}));
    named.source_turn_id = Some("t-2".to_owned());
    let mut invented = parsed_capture(serde_json::json!({}));
    invented.source_turn_id = Some("t-99".to_owned());
    let mut captures = vec![named, invented];

    conform_to_batch(&mut captures, &["t-1".to_owned(), "t-2".to_owned()]);

    assert_eq!(captures[0].source_turn_id.as_deref(), Some("t-2"));
    assert_eq!(captures[1].source_turn_id, None);
}

#[test]
fn a_question_the_write_path_would_refuse_is_dropped_before_submission() {
    let mut kept = parsed_capture(serde_json::json!({}));
    kept.question = Some("Which database should the hosted MVP use?".to_owned());
    let mut request = parsed_capture(serde_json::json!({}));
    request.kind = "decision-request".to_owned();
    request.question = Some("Which database?".to_owned());
    let mut wordless = parsed_capture(serde_json::json!({}));
    wordless.question = Some(" ? ".to_owned());
    let mut on_evidence = parsed_capture(serde_json::json!({}));
    on_evidence.kind = "evidence".to_owned();
    on_evidence.question = Some("Which database?".to_owned());
    let mut captures = vec![kept, request, wordless, on_evidence];

    conform_to_batch(&mut captures, &[]);

    assert!(captures[0].question.is_some());
    assert!(captures[1].question.is_some());
    assert_eq!(captures[2].question, None);
    assert_eq!(captures[3].question, None);
}
