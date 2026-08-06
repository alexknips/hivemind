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
        },
        PendingBatch {
            batch_id: "b".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 2,
        },
        PendingBatch {
            batch_id: "c".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 3,
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
    }];
    let mut classified: HashSet<String> = HashSet::new();
    classified.insert("x".to_owned());

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert!(pending.is_empty());
}

#[test]
fn pending_batch_filter_no_classified_returns_all() {
    let received = vec![
        PendingBatch {
            batch_id: "p".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 5,
        },
        PendingBatch {
            batch_id: "q".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 5,
        },
    ];
    let classified: HashSet<String> = HashSet::new();

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert_eq!(pending.len(), 2);
}
