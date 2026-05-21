use std::fs;

use proptest::prelude::*;
use serde_json::json;
use uuid::Uuid;

use crate::events::{EventProvenance, EventSource, EventType, RelationKind};
use crate::ledger::{EventLedger, InMemoryEventLedger, SqliteEventLedger};

use super::{normalize_topic_key, Commands, MAX_TOPIC_KEY_LEN};

#[test]
fn record_evidence_appends_evidence_recorded_event() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let evidence_id = commands
        .record_evidence("actor:alice", "Observed elevated API latency")
        .expect("record evidence succeeds");

    assert!(evidence_id.starts_with("evidence-"));
    let events = ledger.read(0, 10).expect("read succeeds");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::EvidenceRecorded);
    assert_eq!(events[0].actor_id, "actor:alice");
    assert_eq!(
        events[0]
            .payload
            .get("evidence_id")
            .and_then(|value| value.as_str()),
        Some(evidence_id.as_str())
    );
}

#[test]
fn record_hypothesis_appends_hypothesis_recorded_event() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let hypothesis_id = commands
        .record_hypothesis(
            "actor:bob",
            "API latency spike comes from db lock contention",
        )
        .expect("record hypothesis succeeds");

    assert!(hypothesis_id.starts_with("hypothesis-"));
    let events = ledger.read(0, 10).expect("read succeeds");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::HypothesisRecorded);
    assert_eq!(events[0].actor_id, "actor:bob");
    assert_eq!(
        events[0]
            .payload
            .get("hypothesis_id")
            .and_then(|value| value.as_str()),
        Some(hypothesis_id.as_str())
    );
}

#[test]
fn record_option_returns_option_id_without_writing_event() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:carol", "Use queue", "Ship async queue")
        .expect("record option succeeds");

    assert!(option_id.starts_with("option-"));
    let events = ledger.read(0, 10).expect("read succeeds");
    assert!(events.is_empty());
}

#[test]
fn actor_id_is_required_for_all_entity_commands() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands.record_evidence("", "content").is_err());
    assert!(commands.record_hypothesis("   ", "statement").is_err());
    assert!(commands.record_option("", "label", "description").is_err());
}

#[test]
fn propose_decision_fans_out_relation_events_with_causation_linkage() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_a = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let option_b = commands
        .record_option("actor:alice", "B", "Option B")
        .expect("option b");
    let evidence_id = commands
        .record_evidence("actor:alice", "Log sample")
        .expect("evidence");
    let hypothesis_id = commands
        .record_hypothesis("actor:alice", "This will improve p95")
        .expect("hypothesis");

    let decision_id = commands
        .propose_decision(
            "actor:alice",
            "Pick queue strategy",
            "Need robust ingestion",
            &["Infra / Queue".to_owned()],
            &[option_a.clone(), option_b.clone()],
            Some(option_b.as_str()),
            std::slice::from_ref(&hypothesis_id),
            std::slice::from_ref(&evidence_id),
        )
        .expect("propose decision");

    let events = ledger.read(0, 20).expect("read events");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    let proposal_id = proposal.event_id.expect("proposal event id");

    let relation_events: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == EventType::RelationAdded)
        .collect();
    assert_eq!(relation_events.len(), 5);

    for relation_event in &relation_events {
        assert_eq!(relation_event.causation_event_id, Some(proposal_id));
        assert_eq!(
            relation_event
                .payload
                .get("from_id")
                .and_then(|value| value.as_str()),
            Some(decision_id.as_str())
        );
    }

    let has_option_count = relation_events
        .iter()
        .filter(|event| event.payload.get("relation") == Some(&json!(RelationKind::HasOption)))
        .count();
    assert_eq!(has_option_count, 2);

    let chose_count = relation_events
        .iter()
        .filter(|event| event.payload.get("relation") == Some(&json!(RelationKind::Chose)))
        .count();
    assert_eq!(chose_count, 1);
}

#[test]
fn direct_agent_decision_persists_agent_provenance() {
    let dir = std::env::temp_dir().join(format!("hivemind-agent-provenance-{}", Uuid::new_v4()));
    let actor_id = "agent:codex:furiosa";
    let decision_id = {
        let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
        let commands = Commands::new_with_provenance(
            &ledger,
            EventProvenance::agent("agent:codex:furiosa/session-1"),
        );
        let option_id = commands
            .record_option(actor_id, "Keep substrate small", "Add source fields only")
            .expect("option recorded");
        commands
            .propose_decision(
                actor_id,
                "Record direct agent provenance",
                "Agent-written decisions must be distinguishable from CLI writes",
                &["Integrations".to_owned()],
                &[option_id],
                None,
                &[],
                &[],
            )
            .expect("agent decision proposed")
    };

    let ledger = SqliteEventLedger::open(&dir).expect("ledger reopens");
    let events = ledger.read(0, 20).expect("events read");
    let proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event
                    .payload
                    .get("decision_id")
                    .and_then(|value| value.as_str())
                    == Some(decision_id.as_str())
        })
        .expect("proposal persisted");

    assert_eq!(proposal.actor_id, actor_id);
    assert_eq!(proposal.source, EventSource::Agent);
    assert_eq!(
        proposal.source_ref.as_deref(),
        Some("agent:codex:furiosa/session-1")
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn accept_and_reject_invariant_for_same_actor_is_enforced() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(
            "actor:alice",
            "Pick one",
            "Need progress",
            &["Core".to_owned()],
            &[option_id],
            None,
            &[],
            &[],
        )
        .expect("propose");

    commands
        .accept_decision(&decision_id, "actor:alice")
        .expect("accept succeeds");
    assert!(commands
        .reject_decision(&decision_id, "actor:alice")
        .is_err());
}

#[test]
fn supersede_requires_both_decisions_to_exist() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_a = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let option_b = commands
        .record_option("actor:alice", "B", "Option B")
        .expect("option b");

    let decision_a = commands
        .propose_decision(
            "actor:alice",
            "Decision A",
            "rationale",
            &["Core".to_owned()],
            &[option_a],
            None,
            &[],
            &[],
        )
        .expect("decision a");

    let decision_b = commands
        .propose_decision(
            "actor:alice",
            "Decision B",
            "rationale",
            &["Core".to_owned()],
            &[option_b],
            None,
            &[],
            &[],
        )
        .expect("decision b");

    commands
        .supersede_decision(&decision_a, &decision_b, "actor:alice")
        .expect("supersede succeeds");

    assert!(commands
        .supersede_decision("decision-missing", &decision_b, "actor:alice")
        .is_err());
}

#[test]
fn attach_evidence_requires_existing_endpoints() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(
            "actor:alice",
            "Decision A",
            "rationale",
            &["Core".to_owned()],
            &[option_id],
            None,
            &[],
            &[],
        )
        .expect("decision");
    let evidence_id = commands
        .record_evidence("actor:alice", "evidence")
        .expect("evidence");

    assert!(commands
        .attach_evidence(&decision_id, &evidence_id, "actor:alice")
        .is_ok());
    assert!(commands
        .attach_evidence("missing-decision", &evidence_id, "actor:alice")
        .is_err());
}

#[test]
fn relate_evidence_to_hypothesis_requires_supports_or_refutes_and_is_idempotent() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let evidence_id = commands
        .record_evidence("actor:alice", "evidence")
        .expect("evidence");
    let hypothesis_id = commands
        .record_hypothesis("actor:alice", "hypothesis")
        .expect("hypothesis");

    let first = commands
        .relate_evidence_to_hypothesis(
            &evidence_id,
            &hypothesis_id,
            RelationKind::Supports,
            "actor:alice",
        )
        .expect("first relation");
    let second = commands
        .relate_evidence_to_hypothesis(
            &evidence_id,
            &hypothesis_id,
            RelationKind::Supports,
            "actor:alice",
        )
        .expect("duplicate relation");
    assert_eq!(first, second);

    assert!(commands
        .relate_evidence_to_hypothesis(
            &evidence_id,
            &hypothesis_id,
            RelationKind::BasedOn,
            "actor:alice"
        )
        .is_err());

    let relation_events = ledger
        .read(0, 50)
        .expect("read events")
        .into_iter()
        .filter(|event| {
            event.event_type == EventType::RelationAdded
                && event.payload.get("relation") == Some(&json!(RelationKind::Supports))
        })
        .count();
    assert_eq!(relation_events, 1);
}

#[test]
fn propose_decision_normalizes_topic_keys() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(
            "actor:alice",
            "Normalize topics",
            "Keep consistent filters",
            &[
                "  Crème brûlée API!!  ".to_owned(),
                "Ops___SRE   Alerts".to_owned(),
            ],
            &[option_id],
            None,
            &[],
            &[],
        )
        .expect("propose");

    let events = ledger.read(0, 10).expect("read events");
    let proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event
                    .payload
                    .get("decision_id")
                    .and_then(|value| value.as_str())
                    == Some(decision_id.as_str())
        })
        .expect("proposal event");

    let topics = proposal
        .payload
        .get("topic_keys")
        .and_then(|value| value.as_array())
        .expect("topic keys array")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(topics, vec!["creme-brulee-api", "ops-sre-alerts"]);
}

#[test]
fn normalize_topic_key_handles_unicode_whitespace_punctuation_and_length_cap() {
    assert_eq!(
        normalize_topic_key("  Crème brûlée API!!  "),
        "creme-brulee-api"
    );
    assert_eq!(normalize_topic_key("Ops___SRE   Alerts"), "ops-sre-alerts");

    let input = "a".repeat(MAX_TOPIC_KEY_LEN + 40);
    let normalized = normalize_topic_key(&input);
    assert_eq!(normalized.len(), MAX_TOPIC_KEY_LEN);
    assert!(normalized.chars().all(|character| character == 'a'));
}

proptest! {
    #[test]
    fn normalize_topic_key_is_idempotent(input in ".*") {
        let normalized = normalize_topic_key(&input);
        prop_assert_eq!(normalize_topic_key(&normalized), normalized);
    }

    #[test]
    fn normalize_topic_key_outputs_ascii_slug(input in ".*") {
        let normalized = normalize_topic_key(&input);

        prop_assert!(normalized.is_ascii());
        prop_assert!(normalized.len() <= MAX_TOPIC_KEY_LEN);
        prop_assert!(!normalized.starts_with('-'));
        prop_assert!(!normalized.ends_with('-'));
        prop_assert!(normalized
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'));
        prop_assert!(!normalized.contains("--"));
    }
}
