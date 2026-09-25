use chrono::{DateTime, Utc};
use hivemind::events::{Event, EventSource, EventType};
use serde_json::json;
use uuid::Uuid;

/// Fixture for the attribution golden (hivemind-zdsh.6, hivemind-o7p2): Alex's three cases side
/// by side in one registered project — a human decided, an agent decided within a human's
/// delegation, and an agent decided alone. Only the middle one carries `delegated_by`, on the
/// agent's own `decision.accepted`.
pub fn delegation_cases_events() -> Vec<Event> {
    let mut events = Vec::new();
    let mut push = |event_type: EventType, actor_id: &str, payload: serde_json::Value| {
        let sequence = events.len() + 1;
        events.push(Event {
            tenant_id: Default::default(),
            event_id: None,
            event_uuid: Uuid::from_u128(u128::try_from(sequence).unwrap_or(u128::MAX)),
            correlation_id: Some("delegation-cases-v1".to_owned()),
            causation_event_id: None,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Api,
            source_ref: Some("delegation-cases-v1".to_owned()),
            payload,
            ts: Some(timestamp(sequence)),
        });
    };

    push(
        EventType::ProjectRegistered,
        "human:alex",
        json!({"handle": "governance", "display_name": "Governance"}),
    );

    // Case 1: the agent asked, the human chose. The agent only recorded it.
    push(
        EventType::DecisionProposed,
        "agent:claude:scribe",
        decision("decision-001", "Human chose the license"),
    );
    push(
        EventType::DecisionAccepted,
        "human:alex",
        json!({"decision_id": "decision-001"}),
    );

    // Case 2: a human delegated dependency bumps; the agent decided within that scope.
    push(
        EventType::DecisionProposed,
        "agent:claude:builder",
        decision("decision-002", "Agent bumped the dependency"),
    );
    push(
        EventType::DecisionAccepted,
        "agent:claude:builder",
        json!({"decision_id": "decision-002", "delegated_by": "human:alex"}),
    );

    // Case 3: the agent decided alone; nothing says a human sanctioned it.
    push(
        EventType::DecisionProposed,
        "agent:claude:builder",
        decision("decision-003", "Agent picked the log format"),
    );
    push(
        EventType::DecisionAccepted,
        "agent:claude:builder",
        json!({"decision_id": "decision-003"}),
    );
    events
}

fn decision(decision_id: &str, title: &str) -> serde_json::Value {
    json!({
        "decision_id": decision_id,
        "title": title,
        "rationale": format!("Rationale for {title}"),
        "topic_keys": ["governance"],
        "option_ids": [format!("{decision_id}-a"), format!("{decision_id}-b")],
        "chosen_option_id": format!("{decision_id}-b"),
        "project": "governance",
        "project_source": "stated",
    })
}

fn timestamp(sequence: usize) -> DateTime<Utc> {
    let seconds = i64::try_from(sequence).unwrap_or(i64::MAX);
    DateTime::from_timestamp(1_767_312_000 + seconds, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}
