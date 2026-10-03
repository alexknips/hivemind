// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::commands::{Commands, DecisionProposalInput, Grounding};
use crate::events::{Event, EventId, EventSource, EventType, TenantId};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::DecisionStatus;
use crate::Result;

use super::{get_decision_status_events, DecisionStatusEvents, StatusEvent, StatusEventKind};

const HUMAN: &str = "human:alice";
const OTHER_HUMAN: &str = "human:bob";
const AGENT: &str = "agent:claude:worker";

/// A decision `actor` recorded and chose `chose` for: accepted by `actor` itself, unless
/// `still_proposed` leaves it open or `delegated_by` names the human the agent decided for.
fn capture(
    commands: &Commands<'_, InMemoryEventLedger>,
    actor: &str,
    title: &str,
    chose: &str,
    still_proposed: bool,
    delegated_by: Option<&str>,
) -> Result<String> {
    let option_id = commands.record_option(actor, chose, chose)?;
    commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: actor,
        title,
        rationale: "Rationale text long enough for the readable floor",
        topic_keys: &["storage".to_owned()],
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &[chose.to_owned()],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: None,
        still_proposed,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
        delegated_by,
        project: None,
    })
}

fn status_events(ledger: &InMemoryEventLedger, decision_id: &str) -> Result<DecisionStatusEvents> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, &TenantId::local(), &graph)?;
    Ok(get_decision_status_events(&graph, ledger, decision_id)?
        .data
        .expect("the decision exists"))
}

fn kinds(read: &DecisionStatusEvents) -> Vec<StatusEventKind> {
    read.events.iter().map(|event| event.event).collect()
}

fn statuses_after(read: &DecisionStatusEvents) -> Vec<DecisionStatus> {
    read.events.iter().map(|event| event.status_after).collect()
}

fn time(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses")
        .with_timezone(&Utc)
}

fn append(
    ledger: &InMemoryEventLedger,
    actor_id: &str,
    event_type: EventType,
    payload: Value,
    timestamp: Option<&str>,
) -> Result<EventId> {
    ledger.append(Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::new_v4(),
        correlation_id: None,
        causation_event_id: None,
        event_type,
        actor_id: actor_id.to_owned(),
        source: EventSource::Cli,
        source_ref: None,
        payload,
        ts: timestamp.map(time),
    })
}

/// One `ingest.batch_classified` event holding `captures`; the decisions are
/// `capture:<returned id>:<index>`.
fn classify(
    ledger: &InMemoryEventLedger,
    timestamp: &str,
    captures: Vec<Value>,
) -> Result<EventId> {
    append(
        ledger,
        "agent:claude:classifier",
        EventType::IngestBatchClassified,
        json!({
            "batch_id": "session-1:0",
            "classifier_model": "test-classifier",
            "schema_version": "2",
            "captures": captures,
        }),
        Some(timestamp),
    )
}

fn decision_capture(title: &str, extra: Value) -> Value {
    let mut capture = json!({
        "kind": "decision",
        "title": title,
        "rationale": format!("Rationale for {title}, long enough to read on its own"),
        "topic_keys": ["storage"],
        "evidence_ids": [],
        "options": Value::Null,
        "chosen_option": Value::Null,
        "extraction_confidence": 0.9,
    });
    if let (Some(base), Some(extra)) = (capture.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    capture
}

#[test]
fn an_unknown_decision_has_no_status_events() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let graph = MemoryGraph::default();
    let response = get_decision_status_events(&graph, &ledger, "decision-nobody-made")?;
    assert_eq!(response.result_count, 0);
    assert!(!response.truncated);
    assert!(response.data.is_none());
    Ok(())
}

#[test]
fn a_proposal_and_its_acceptance_are_listed_newest_first_with_who_and_when() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let id = capture(&commands, AGENT, "Use SQLite", "sqlite", false, None)?;

    let read = status_events(&ledger, &id)?;

    assert_eq!(read.decision_id, id);
    assert_eq!(
        kinds(&read),
        vec![StatusEventKind::Accepted, StatusEventKind::Proposed]
    );
    assert_eq!(
        statuses_after(&read),
        vec![DecisionStatus::Accepted, DecisionStatus::Proposed]
    );
    assert_eq!(read.status, DecisionStatus::Accepted);
    assert!(
        read.events[0].offset > read.events[1].offset,
        "newest first: {:?}",
        read.events
    );
    for event in &read.events {
        let actor = event.actor.as_ref().expect("both name the agent");
        assert_eq!((actor.id.as_str(), actor.kind), (AGENT, "agent"));
        assert!(event.occurred_at.is_some());
        assert_eq!(event.delegated_by, None);
        assert_eq!(event.superseded_by, None);
    }
    Ok(())
}

#[test]
fn a_decision_nobody_has_accepted_has_only_its_proposal() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let id = capture(&commands, AGENT, "Use SQLite", "sqlite", true, None)?;

    let read = status_events(&ledger, &id)?;

    assert_eq!(kinds(&read), vec![StatusEventKind::Proposed]);
    assert_eq!(read.status, DecisionStatus::Proposed);
    assert_eq!(read.events[0].status_after, DecisionStatus::Proposed);
    Ok(())
}

#[test]
fn a_rejection_makes_it_contested_and_a_supersession_makes_it_superseded() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old = capture(&commands, AGENT, "Use SQLite", "sqlite", false, None)?;
    let new = capture(&commands, HUMAN, "Use Postgres", "postgres", false, None)?;
    commands.reject_decision(&old, OTHER_HUMAN)?;
    commands.supersede_decision(&old, &new, HUMAN)?;

    let read = status_events(&ledger, &old)?;

    assert_eq!(
        kinds(&read),
        vec![
            StatusEventKind::Superseded,
            StatusEventKind::Rejected,
            StatusEventKind::Accepted,
            StatusEventKind::Proposed,
        ]
    );
    assert_eq!(
        statuses_after(&read),
        vec![
            DecisionStatus::Superseded,
            DecisionStatus::Contested,
            DecisionStatus::Accepted,
            DecisionStatus::Proposed,
        ]
    );
    assert_eq!(read.status, DecisionStatus::Superseded);
    assert_eq!(read.status, read.events[0].status_after);

    let superseded = &read.events[0];
    assert_eq!(superseded.superseded_by.as_deref(), Some(new.as_str()));
    let actor = superseded.actor.as_ref().expect("the superseder");
    assert_eq!((actor.id.as_str(), actor.kind), (HUMAN, "human"));
    let rejected = read.events[1].actor.as_ref().expect("the rejecter");
    assert_eq!(
        (rejected.id.as_str(), rejected.kind),
        (OTHER_HUMAN, "human")
    );

    // The replacing decision is not itself superseded: it only gained a link.
    let newer = status_events(&ledger, &new)?;
    assert_eq!(
        kinds(&newer),
        vec![StatusEventKind::Accepted, StatusEventKind::Proposed]
    );
    assert_eq!(newer.status, DecisionStatus::Accepted);
    Ok(())
}

#[test]
fn an_agents_acceptance_under_a_delegation_names_who_delegated() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let id = capture(&commands, AGENT, "Use SQLite", "sqlite", false, Some(HUMAN))?;

    let read = status_events(&ledger, &id)?;

    assert_eq!(read.events[0].event, StatusEventKind::Accepted);
    assert_eq!(read.events[0].delegated_by.as_deref(), Some(HUMAN));
    assert_eq!(read.events[1].delegated_by, None);
    let json = serde_json::to_value(&read).expect("status events serialize");
    assert_eq!(json["events"][0]["delegated_by"], HUMAN);
    assert!(
        json["events"][1].get("delegated_by").is_none(),
        "absent, not null: {json}"
    );
    Ok(())
}

#[test]
fn concurrent_supersessions_each_get_an_entry_and_both_stand() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old = capture(&commands, AGENT, "Use SQLite", "sqlite", false, None)?;
    let first = capture(&commands, HUMAN, "Use Postgres", "postgres", false, None)?;
    let second = capture(&commands, HUMAN, "Use DuckDB", "duckdb", false, None)?;
    commands.supersede_decision(&old, &first, HUMAN)?;
    commands.supersede_decision(&old, &second, HUMAN)?;

    let read = status_events(&ledger, &old)?;

    assert_eq!(
        kinds(&read),
        vec![
            StatusEventKind::Superseded,
            StatusEventKind::Superseded,
            StatusEventKind::Accepted,
            StatusEventKind::Proposed,
        ]
    );
    let replacements: Vec<&str> = read
        .events
        .iter()
        .filter_map(|event| event.superseded_by.as_deref())
        .collect();
    assert_eq!(replacements, vec![second.as_str(), first.as_str()]);
    assert!(read.events[..2]
        .iter()
        .all(|event| event.status_after == DecisionStatus::Superseded));
    assert_eq!(read.status, DecisionStatus::Superseded);
    Ok(())
}

#[test]
fn an_event_with_no_time_or_actor_is_null_on_the_wire_not_absent() {
    // The in-memory ledger stamps every event it appends, so an undated one (an event imported
    // without a time) cannot be built through it; the wire shape is pinned on the entry itself.
    let event = StatusEvent {
        event: StatusEventKind::Proposed,
        occurred_at: None,
        offset: 7,
        actor: None,
        delegated_by: None,
        superseded_by: None,
        status_after: DecisionStatus::Proposed,
    };

    let json = serde_json::to_value(&event).expect("a status event serializes");

    assert_eq!(
        json,
        json!({
            "event": "proposed",
            "occurred_at": null,
            "offset": 7,
            "actor": null,
            "status_after": "proposed",
        })
    );
}

#[test]
fn a_capture_that_states_its_acceptance_shows_both_at_the_same_time() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let origin = classify(
        &ledger,
        "2026-09-23T12:00:00Z",
        vec![decision_capture(
            "Use SQLite",
            json!({
                "actor_id": AGENT,
                "accepted_by": [HUMAN],
                "source_ts": "2026-09-22T08:30:00Z",
            }),
        )],
    )?;
    let id = format!("capture:{origin}:0");

    let read = status_events(&ledger, &id)?;

    assert_eq!(
        kinds(&read),
        vec![StatusEventKind::Accepted, StatusEventKind::Proposed]
    );
    assert_eq!(
        statuses_after(&read),
        vec![DecisionStatus::Accepted, DecisionStatus::Proposed]
    );
    assert_eq!(read.status, DecisionStatus::Accepted);
    // One capture: one offset, and the time of the turn it came from, not the batch's.
    for event in &read.events {
        assert_eq!(event.offset, origin);
        assert_eq!(event.occurred_at, Some(time("2026-09-22T08:30:00Z")));
    }
    let accepted = read.events[0].actor.as_ref().expect("the named acceptor");
    assert_eq!((accepted.id.as_str(), accepted.kind), (HUMAN, "human"));
    let proposed = read.events[1].actor.as_ref().expect("the named proposer");
    assert_eq!((proposed.id.as_str(), proposed.kind), (AGENT, "agent"));
    Ok(())
}

#[test]
fn a_capture_that_names_a_rejecter_and_a_replaced_decision_replays_to_the_current_status(
) -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let origin = classify(
        &ledger,
        "2026-09-23T12:00:00Z",
        vec![
            decision_capture(
                "Use SQLite",
                json!({
                    "actor_id": AGENT,
                    "accepted_by": [HUMAN],
                    "rejected_by": [OTHER_HUMAN],
                    "source_ts": "2026-09-22T08:30:00Z",
                }),
            ),
            decision_capture(
                "Use Postgres",
                json!({
                    "actor_id": OTHER_HUMAN,
                    "supersedes_id": "Use SQLite",
                    "source_ts": "2026-09-22T09:00:00Z",
                }),
            ),
        ],
    )?;
    let old = format!("capture:{origin}:0");
    let new = format!("capture:{origin}:1");

    let read = status_events(&ledger, &old)?;

    assert_eq!(
        kinds(&read),
        vec![
            StatusEventKind::Superseded,
            StatusEventKind::Rejected,
            StatusEventKind::Accepted,
            StatusEventKind::Proposed,
        ]
    );
    assert_eq!(
        statuses_after(&read),
        vec![
            DecisionStatus::Superseded,
            DecisionStatus::Contested,
            DecisionStatus::Accepted,
            DecisionStatus::Proposed,
        ]
    );
    assert_eq!(read.status, read.events[0].status_after);

    // The replacement is dated when the replacing decision was recorded, and credited to its
    // proposer; the others are dated when this decision was.
    let superseded = &read.events[0];
    assert_eq!(superseded.superseded_by.as_deref(), Some(new.as_str()));
    assert_eq!(superseded.occurred_at, Some(time("2026-09-22T09:00:00Z")));
    assert_eq!(
        superseded.actor.as_ref().map(|actor| actor.id.as_str()),
        Some(OTHER_HUMAN)
    );
    for event in &read.events[1..] {
        assert_eq!(event.occurred_at, Some(time("2026-09-22T08:30:00Z")));
    }

    // The replacing decision only gained a link: it is still proposed.
    let newer = status_events(&ledger, &new)?;
    assert_eq!(kinds(&newer), vec![StatusEventKind::Proposed]);
    assert_eq!(newer.status, DecisionStatus::Proposed);
    Ok(())
}

#[test]
fn a_decision_a_capture_proposed_can_be_decided_later_by_its_own_event() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let origin = classify(
        &ledger,
        "2026-09-23T12:00:00Z",
        vec![decision_capture("Use SQLite", json!({ "actor_id": AGENT }))],
    )?;
    let id = format!("capture:{origin}:0");
    let accepted = append(
        &ledger,
        HUMAN,
        EventType::DecisionAccepted,
        json!({ "decision_id": id }),
        Some("2026-09-24T10:00:00Z"),
    )?;

    let read = status_events(&ledger, &id)?;

    assert_eq!(
        kinds(&read),
        vec![StatusEventKind::Accepted, StatusEventKind::Proposed]
    );
    assert_eq!(read.events[0].offset, accepted);
    assert_eq!(
        read.events[0].occurred_at,
        Some(time("2026-09-24T10:00:00Z"))
    );
    assert_eq!(read.events[1].offset, origin);
    assert_eq!(read.status, DecisionStatus::Accepted);
    Ok(())
}

#[test]
fn the_newest_status_after_is_the_status_now_for_every_decision_in_a_mixed_ledger() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let kept = capture(&commands, AGENT, "Use SQLite", "sqlite", false, None)?;
    let open = capture(&commands, AGENT, "Use Redis", "redis", true, None)?;
    let disputed = capture(&commands, AGENT, "Use Kafka", "kafka", false, None)?;
    commands.reject_decision(&disputed, HUMAN)?;
    let origin = classify(
        &ledger,
        "2026-09-23T12:00:00Z",
        vec![
            decision_capture("Use Spanner", json!({ "accepted_by": [HUMAN] })),
            decision_capture("Use Cockroach", json!({ "rejected_by": [HUMAN] })),
        ],
    )?;
    let ids = [
        kept,
        open,
        disputed,
        format!("capture:{origin}:0"),
        format!("capture:{origin}:1"),
    ];

    for id in &ids {
        let read = status_events(&ledger, id)?;
        assert_eq!(
            read.status, read.events[0].status_after,
            "{id}: {:?}",
            read.events
        );
        let offsets: Vec<EventId> = read.events.iter().map(|event| event.offset).collect();
        assert!(
            offsets.windows(2).all(|pair| pair[0] >= pair[1]),
            "{id} is newest first: {offsets:?}"
        );
    }
    Ok(())
}
