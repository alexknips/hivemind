use chrono::{DateTime, Utc};
use hivemind::events::{Event, EventSource, EventType};
use serde_json::json;
use uuid::Uuid;

/// Fixture for the per-project decision-record golden: two registered projects with a
/// decision superseded across them, one registered project with no decisions yet, a human's
/// decision with no project stated, and two agent sessions that share one personal project.
pub fn project_layout_events() -> Vec<Event> {
    let mut events = Vec::new();
    let mut push = |event_type: EventType, actor_id: &str, payload: serde_json::Value| {
        let sequence = events.len() + 1;
        events.push(Event {
            tenant_id: Default::default(),
            event_id: None,
            event_uuid: Uuid::from_u128(u128::try_from(sequence).unwrap_or(u128::MAX)),
            correlation_id: Some("project-layout-v1".to_owned()),
            causation_event_id: None,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Api,
            source_ref: Some("project-layout-v1".to_owned()),
            payload,
            ts: Some(timestamp(sequence)),
        });
    };

    push(
        EventType::ProjectRegistered,
        "human:alice",
        json!({"handle": "platform", "display_name": "Platform", "purpose": "Shared infrastructure"}),
    );
    push(
        EventType::ProjectRegistered,
        "human:alice",
        json!({"handle": "billing"}),
    );
    push(
        EventType::ProjectRegistered,
        "human:alice",
        json!({"handle": "beadline"}),
    );
    push(
        EventType::DecisionProposed,
        "human:bob",
        decision("decision-001", "Charge per seat", Some("billing")),
    );
    push(
        EventType::DecisionProposed,
        "human:alice",
        decision(
            "decision-002",
            "Bill per organization instead",
            Some("platform"),
        ),
    );
    push(
        EventType::DecisionSuperseded,
        "human:alice",
        json!({"old_decision_id": "decision-001", "new_decision_id": "decision-002"}),
    );
    push(
        EventType::DecisionProposed,
        "human:alice",
        decision("decision-003", "Try a scratch idea", None),
    );
    push(
        EventType::DecisionProposed,
        "agent:claude:session-1",
        decision("decision-004", "Agent note", None),
    );
    push(
        EventType::DecisionProposed,
        "agent:claude:session-2",
        decision("decision-005", "Second agent note", None),
    );
    events
}

fn decision(decision_id: &str, title: &str, project: Option<&str>) -> serde_json::Value {
    let mut payload = json!({
        "decision_id": decision_id,
        "title": title,
        "rationale": format!("Rationale for {title}"),
        "topic_keys": ["billing"],
        "option_ids": [format!("{decision_id}-a"), format!("{decision_id}-b")],
        "chosen_option_id": format!("{decision_id}-b"),
    });
    if let Some(project) = project {
        payload["project"] = json!(project);
        payload["project_source"] = json!("stated");
    }
    payload
}

fn timestamp(sequence: usize) -> DateTime<Utc> {
    let seconds = i64::try_from(sequence).unwrap_or(i64::MAX);
    DateTime::from_timestamp(1_767_312_000 + seconds, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}
