// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType};

use super::*;

fn registered(sequence: u128, handle: &str, display_name: Option<&str>) -> Event {
    let mut payload = json!({ "handle": handle });
    if let Some(name) = display_name {
        payload["display_name"] = json!(name);
    }
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::from_u128(sequence),
        correlation_id: None,
        causation_event_id: None,
        event_type: EventType::ProjectRegistered,
        actor_id: "human:alex".to_owned(),
        source: EventSource::Cli,
        source_ref: None,
        payload,
        ts: None,
    }
}

#[test]
fn a_human_personal_address_is_labelled_with_the_person() {
    assert_eq!(
        personal_project_label("personal:human:alex").as_deref(),
        Some("alex's personal project")
    );
    assert_eq!(
        personal_project_label("personal:human:alex.knips@gmail.com").as_deref(),
        Some("alex.knips@gmail.com's personal project")
    );
}

#[test]
fn an_agent_personal_address_is_labelled_with_the_tool_not_the_session() {
    assert_eq!(
        personal_project_label("personal:agent:claude").as_deref(),
        Some("claude agents' personal project")
    );
}

#[test]
fn an_untyped_personal_address_is_labelled_with_the_actor_as_written() {
    assert_eq!(
        personal_project_label("personal:actor:alice").as_deref(),
        Some("actor:alice's personal project")
    );
}

#[test]
fn a_registered_handle_is_not_a_personal_address() {
    assert_eq!(personal_project_label("billing"), None);
}

#[test]
fn a_registered_project_is_labelled_with_its_display_name_else_its_handle() {
    let events = [
        registered(1, "billing", Some("Billing")),
        registered(2, "auth", None),
        registered(3, "blank", Some("   ")),
    ];
    let labels = ProjectLabels::from_events(&events);
    assert_eq!(labels.label("billing"), "Billing");
    assert_eq!(labels.label("auth"), "auth");
    assert_eq!(labels.label("blank"), "blank");
}

#[test]
fn an_unregistered_handle_is_labelled_with_itself() {
    let labels = ProjectLabels::from_events(&[]);
    assert_eq!(labels.label("nowhere"), "nowhere");
}

#[test]
fn a_personal_address_needs_no_registry() {
    let labels = ProjectLabels::from_events(&[]);
    assert_eq!(
        labels.label("personal:human:alex"),
        "alex's personal project"
    );
}

#[test]
fn a_later_registration_of_the_same_handle_wins() {
    let events = [
        registered(1, "billing", Some("Billing")),
        registered(2, "billing", Some("Billing Team")),
    ];
    assert_eq!(
        ProjectLabels::from_events(&events).label("billing"),
        "Billing Team"
    );
}

#[test]
fn events_that_are_not_project_registrations_are_ignored() {
    let mut other = registered(1, "billing", Some("Billing"));
    other.event_type = EventType::ProjectAnchored;
    assert_eq!(
        ProjectLabels::from_events(&[other]).label("billing"),
        "billing"
    );
}
