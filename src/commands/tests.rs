// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;

use proptest::prelude::*;
use serde_json::json;
use uuid::Uuid;

use crate::events::{
    EventProvenance, EventSource, EventType, ProjectAnchorKind, ProjectLinkKind, RelationKind,
};
use crate::ledger::{EventLedger, InMemoryEventLedger, SqliteEventLedger};

use super::{
    normalize_topic_key, Commands, DecisionProposalInput, SupersedeInput, MAX_TITLE_LEN,
    MAX_TOPIC_KEY_LEN,
};

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
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Pick queue strategy",
            rationale: "Need robust ingestion",
            topic_keys: &["Infra / Queue".to_owned()],
            option_ids: &[option_a.clone(), option_b.clone()],
            option_labels: &[],
            chosen_option_id: Some(option_b.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: std::slice::from_ref(&hypothesis_id),
            evidence_ids: std::slice::from_ref(&evidence_id),
        })
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
        let from_id = relation_event
            .payload
            .get("from_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let relation_kind = relation_event
            .payload
            .get("relation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if relation_kind == "ASSUMES" {
            assert_eq!(from_id, option_b.as_str());
        } else {
            assert_eq!(from_id, decision_id.as_str());
        }
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
            .propose_decision(DecisionProposalInput {
                actor_id,
                title: "Record direct agent provenance",
                rationale: "Agent-written decisions must be distinguishable from CLI writes",
                topic_keys: &["Integrations".to_owned()],
                option_ids: &[option_id],
                option_labels: &[],
                chosen_option_id: None,
                decided_by: None,
                still_proposed: false,
                hypothesis_ids: &[],
                evidence_ids: &[],
            })
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
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Pick one",
            rationale: "Need progress",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("propose");

    commands
        .accept_decision(&decision_id, "actor:alice")
        .expect("accept succeeds");
    assert!(commands
        .reject_decision(&decision_id, "actor:alice")
        .is_err());
}

#[test]
fn propose_decision_with_decided_by_emits_accepted_event_from_that_actor() {
    // hivemind-zdsh.3: an agent (the recorder) proposing a decision a human (the decider)
    // already made must not leave it stuck at `proposed` — decided_by drives an explicit
    // accept from the decider, distinct from the recording actor_id.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("agent:claude:scribe", "Ship it", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "agent:claude:scribe",
            title: "Human decides, agent records",
            rationale: "The human chose; the agent is only writing it down",
            topic_keys: &["governance".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: Some("human:alex"),
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("propose with decided_by");

    let events = ledger.read(0, 20).expect("read events");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    assert_eq!(proposal.actor_id, "agent:claude:scribe");

    let accepted = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionAccepted)
        .expect("decided_by must emit a decision.accepted event");
    assert_eq!(accepted.actor_id, "human:alex");
    assert_eq!(
        accepted.payload.get("decision_id").and_then(|v| v.as_str()),
        Some(decision_id.as_str())
    );
}

#[test]
fn propose_decision_decided_by_requires_chosen_option_id() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("agent:claude:scribe", "A", "Option A")
        .expect("option");
    let result = commands.propose_decision(DecisionProposalInput {
        actor_id: "agent:claude:scribe",
        title: "No chosen option yet",
        rationale: "Still an open proposal",
        topic_keys: &["governance".to_owned()],
        option_ids: &[option_id],
        option_labels: &[],
        chosen_option_id: None,
        decided_by: Some("human:alex"),
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
    });
    assert!(
        result.is_err(),
        "decided_by without a chosen_option_id must be rejected"
    );
}

#[test]
fn propose_decision_chosen_option_defaults_to_self_accepted() {
    // hivemind-zdsh.8: a chosen option means the decision was already made. Without
    // decided_by naming a different decider, propose_decision self-accepts from actor_id
    // instead of leaving the decision stuck at `proposed` forever.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Chosen option, no decided_by",
            rationale: "The proposer is the decider here",
            topic_keys: &["Core".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("propose");

    let events = ledger.read(0, 20).expect("read events");
    let accepted = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionAccepted)
        .expect("a chosen option without still_proposed must self-accept");
    assert_eq!(accepted.actor_id, "actor:alice");
    assert_eq!(
        accepted.payload.get("decision_id").and_then(|v| v.as_str()),
        Some(decision_id.as_str())
    );
}

#[test]
fn propose_decision_still_proposed_keeps_chosen_option_open() {
    // The explicit opt-out: a genuine open recommendation awaiting someone else's decision
    // stays `proposed` even though a chosen option is given.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Proposed with a leaning but not decided",
            rationale: "Awaiting someone else's decision",
            topic_keys: &["Core".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("propose");

    let events = ledger.read(0, 20).expect("read events");
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == EventType::DecisionAccepted),
        "still_proposed must mean no accept event"
    );
}

#[test]
fn propose_decision_still_proposed_conflicts_with_decided_by() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let result = commands.propose_decision(DecisionProposalInput {
        actor_id: "actor:alice",
        title: "Contradictory flags",
        rationale: "still_proposed and decided_by disagree about whether this is decided",
        topic_keys: &["Core".to_owned()],
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &[],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: Some("human:alex"),
        still_proposed: true,
        hypothesis_ids: &[],
        evidence_ids: &[],
    });
    assert!(
        result.is_err(),
        "still_proposed together with decided_by must be rejected"
    );
}

#[test]
fn propose_decision_rejects_mismatched_option_labels_length() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_a = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let option_b = commands
        .record_option("actor:alice", "B", "Option B")
        .expect("option b");

    let result = commands.propose_decision(DecisionProposalInput {
        actor_id: "actor:alice",
        title: "Mismatched labels",
        rationale: "option_labels shorter than option_ids and non-empty",
        topic_keys: &["Core".to_owned()],
        option_ids: &[option_a, option_b],
        option_labels: &["Only one label".to_owned()],
        chosen_option_id: None,
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
    });
    assert!(result.is_err());
}

#[test]
fn propose_decision_rejects_six_clause_run_on_title() {
    // hivemind-zdsh.12: the actual offending title from decision-04ea64f9 (mayor audit
    // 2026-09-20..22), a 200+ character six-clause run-on. It has no terminal punctuation
    // at all -- length alone is what must reject it.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");

    let run_on_title = "Projects model: one project per decision (the common parent when a \
change spans two), visible personal projects, checked-in nearest-wins markers, current \
project for non-coders, anyone registers, parent const";
    assert!(
        run_on_title.chars().count() > MAX_TITLE_LEN,
        "fixture must actually exceed the cap to exercise the rejection"
    );

    let error = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: run_on_title,
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect_err("run-on title over the cap must be rejected");
    let message = error.to_string();
    assert!(
        message.contains(&MAX_TITLE_LEN.to_string()),
        "error must name the rule (the 120-char cap): {message}"
    );
}

#[test]
fn propose_decision_rejects_title_at_max_len_plus_one() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");

    let title: String = "x".repeat(MAX_TITLE_LEN + 1);
    let result = commands.propose_decision(DecisionProposalInput {
        actor_id: "actor:alice",
        title: &title,
        rationale: "rationale",
        topic_keys: &["Core".to_owned()],
        option_ids: &[option_id],
        option_labels: &[],
        chosen_option_id: None,
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
    });
    assert!(
        result.is_err(),
        "title one char over the cap must be rejected"
    );
}

#[test]
fn propose_decision_accepts_title_at_exactly_max_len() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");

    let title: String = "x".repeat(MAX_TITLE_LEN);
    commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: &title,
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("title exactly at the cap must be accepted");
}

#[test]
fn propose_decision_rejects_multi_sentence_title() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");

    let error = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Use SQLite for slice 1. Migrate to Postgres later.",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect_err("a title with two sentences must be rejected");
    assert!(
        error.to_string().contains("single sentence"),
        "error must name the rule: {error}"
    );
}

#[test]
fn propose_decision_rejects_numbered_list_title() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");

    // Parenthesis-style markers ("1)"/"2)"), not periods, so this exercises the
    // numbered-list rule specifically rather than tripping the sentence-count rule first.
    let error = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "1) Use SQLite 2) Add WAL mode",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect_err("a numbered-list title must be rejected");
    assert!(
        error.to_string().contains("numbered list"),
        "error must name the rule: {error}"
    );
}

#[test]
fn propose_decision_accepts_title_with_single_trailing_period_and_version_dots() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");

    // Guards against false positives: a normal trailing period, and non-terminal periods
    // inside a version number ("v1.2.3"), must not be mistaken for multiple sentences.
    commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Ship v1.2.3 to prod.",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("a single trailing period plus version dots must be accepted");
}

#[test]
fn supersede_rejects_new_title_over_max_length() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let old_decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("decision");

    let new_title: String = "y".repeat(MAX_TITLE_LEN + 1);
    let result = commands.supersede(SupersedeInput {
        actor_id: "actor:alice",
        old_decision_id: &old_decision_id,
        new_title: &new_title,
        new_rationale: "New rationale",
        topic_keys: &[],
        option_labels: &["Replacement".to_owned()],
        chosen_option_label: None,
        hypothesis_ids: &[],
        evidence_ids: &[],
    });
    assert!(
        result.is_err(),
        "supersede's new_title must honor the same cap as propose_decision's title"
    );
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
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_a],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("decision a");

    let decision_b = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision B",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_b],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("decision b");

    commands
        .supersede_decision(&decision_a, &decision_b, "actor:alice")
        .expect("supersede succeeds");

    assert!(commands
        .supersede_decision("decision-missing", &decision_b, "actor:alice")
        .is_err());
}

#[test]
fn disagree_records_reason_and_is_idempotent_for_same_actor() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("decision");

    commands
        .accept_decision(&decision_id, "actor:bob")
        .expect("accept succeeds");
    let first_event_id = commands
        .disagree("actor:carol", &decision_id, "misses auth implications")
        .expect("disagree succeeds");
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let second_event_id = commands
        .disagree("actor:carol", &decision_id, "misses auth implications")
        .expect("retry succeeds");

    assert_eq!(second_event_id, first_event_id);
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let events = ledger.read(0, 20).expect("events read");
    let rejected = events
        .iter()
        // ubs:ignore: event IDs are public ledger offsets, not secrets.
        .find(|event| event.event_id == Some(first_event_id))
        .expect("rejection event");
    assert_eq!(rejected.event_type, EventType::DecisionRejected);
    assert_eq!(
        rejected
            .payload
            .get("reason")
            .and_then(|value| value.as_str()),
        Some("misses auth implications")
    );
}

#[test]
fn supersede_proposes_replacement_marks_old_and_is_idempotent() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let old_decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("decision");

    let first = commands
        .supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "New rationale",
            topic_keys: &[],
            option_labels: &["Replacement".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("supersede succeeds");
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let second = commands
        .supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "New rationale",
            topic_keys: &[],
            option_labels: &["Replacement".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("retry succeeds");

    assert_eq!(second, first);
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first
    );

    let events = ledger.read(0, 20).expect("events read");
    let superseded = events
        .iter()
        // ubs:ignore: event IDs are public ledger offsets, not secrets.
        .find(|event| event.event_id == Some(first.superseded_event_id))
        .expect("superseded event");
    assert_eq!(superseded.event_type, EventType::DecisionSuperseded);
    assert_eq!(
        superseded
            .payload
            .get("old_decision_id")
            .and_then(|value| value.as_str()),
        Some(old_decision_id.as_str())
    );
    assert_eq!(
        superseded
            .payload
            .get("new_decision_id")
            .and_then(|value| value.as_str()),
        Some(first.new_decision_id.as_str())
    );
}

#[test]
fn first_class_disagree_and_supersede_require_existing_targets() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .disagree("actor:alice", "decision-missing", "reason")
        .is_err());
    assert!(commands
        .supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: "decision-missing",
            new_title: "Decision B",
            new_rationale: "New rationale",
            topic_keys: &["Core".to_owned()],
            option_labels: &["Replacement".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
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
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
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
fn register_project_appends_project_registered_event() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    commands
        .register_project(
            "actor:alice",
            "billing",
            Some("Billing"),
            Some("Per-seat and per-org pricing decisions"),
        )
        .expect("register succeeds");

    let events = ledger.read(0, 10).expect("read succeeds");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::ProjectRegistered);
    assert_eq!(
        events[0].payload.get("handle").and_then(|v| v.as_str()),
        Some("billing")
    );
    assert_eq!(
        events[0]
            .payload
            .get("display_name")
            .and_then(|v| v.as_str()),
        Some("Billing")
    );
}

#[test]
fn register_project_rejects_bad_handle_format() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    // Too short.
    assert!(commands
        .register_project("actor:alice", "a", None, None)
        .is_err());
    // Uppercase not allowed.
    assert!(commands
        .register_project("actor:alice", "Billing", None, None)
        .is_err());
    // Underscore not allowed.
    assert!(commands
        .register_project("actor:alice", "bill_ing", None, None)
        .is_err());
    // Longer than 40 chars.
    assert!(commands
        .register_project("actor:alice", &"b".repeat(41), None, None)
        .is_err());
    // The "personal:" prefix is reserved.
    assert!(commands
        .register_project("actor:alice", "personal:alice", None, None)
        .is_err());

    assert!(ledger.read(0, 10).expect("read succeeds").is_empty());
}

#[test]
fn register_project_rejects_duplicate_handle_naming_existing_project() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    commands
        .register_project("actor:alice", "billing", Some("Billing"), None)
        .expect("first registration succeeds");

    let error = commands
        .register_project("actor:bob", "billing", Some("Billing Team"), None)
        .expect_err("duplicate handle is refused");
    let message = error.to_string();
    assert!(message.contains("billing"), "message was: {message}");
    assert!(message.contains("Billing"), "message was: {message}");

    let events = ledger.read(0, 10).expect("read succeeds");
    assert_eq!(events.len(), 1, "duplicate registration must not append");
}

#[test]
fn link_project_requires_both_endpoints_registered() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    commands
        .register_project("actor:alice", "billing", None, None)
        .expect("register billing");

    assert!(commands
        .link_project(
            "actor:alice",
            "billing",
            "platform",
            ProjectLinkKind::PartOf
        )
        .is_err());
    assert!(commands
        .link_project(
            "actor:alice",
            "platform",
            "billing",
            ProjectLinkKind::PartOf
        )
        .is_err());

    assert!(ledger
        .read(0, 10)
        .expect("read succeeds")
        .iter()
        .all(|event| event.event_type != EventType::ProjectLinked));
}

#[test]
fn link_project_rejects_self_link() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    commands
        .register_project("actor:alice", "billing", None, None)
        .expect("register billing");

    assert!(commands
        .link_project(
            "actor:alice",
            "billing",
            "billing",
            ProjectLinkKind::DependsOn
        )
        .is_err());
}

#[test]
fn link_project_part_of_allows_only_one_active_parent() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    for handle in ["billing", "platform", "auth"] {
        commands
            .register_project("actor:alice", handle, None, None)
            .expect("register");
    }

    commands
        .link_project(
            "actor:alice",
            "billing",
            "platform",
            ProjectLinkKind::PartOf,
        )
        .expect("first parent succeeds");

    // A second, different part_of parent is refused.
    assert!(commands
        .link_project("actor:alice", "billing", "auth", ProjectLinkKind::PartOf)
        .is_err());

    // depends_on has no such limit.
    assert!(commands
        .link_project("actor:alice", "billing", "auth", ProjectLinkKind::DependsOn)
        .is_ok());
}

#[test]
fn unlink_project_requires_an_active_link() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    for handle in ["billing", "platform"] {
        commands
            .register_project("actor:alice", handle, None, None)
            .expect("register");
    }

    assert!(commands
        .unlink_project(
            "actor:alice",
            "billing",
            "platform",
            ProjectLinkKind::PartOf
        )
        .is_err());

    commands
        .link_project(
            "actor:alice",
            "billing",
            "platform",
            ProjectLinkKind::PartOf,
        )
        .expect("link succeeds");
    commands
        .unlink_project(
            "actor:alice",
            "billing",
            "platform",
            ProjectLinkKind::PartOf,
        )
        .expect("unlink succeeds");

    // Once unlinked, a new part_of parent is accepted again.
    commands
        .register_project("actor:alice", "auth", None, None)
        .expect("register auth");
    assert!(commands
        .link_project("actor:alice", "billing", "auth", ProjectLinkKind::PartOf)
        .is_ok());
}

#[test]
fn anchor_project_requires_registered_project() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .anchor_project(
            "actor:alice",
            "billing",
            ProjectAnchorKind::Folder,
            "services/billing",
        )
        .is_err());
}

#[test]
fn anchor_project_rejects_duplicate_rig_anchor_value() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    for handle in ["hivemind", "hivemind-ui"] {
        commands
            .register_project("actor:alice", handle, None, None)
            .expect("register");
    }

    commands
        .anchor_project(
            "actor:alice",
            "hivemind",
            ProjectAnchorKind::Rig,
            "hivemind",
        )
        .expect("first rig anchor succeeds");

    // A different project claiming the same rig value is refused.
    assert!(commands
        .anchor_project(
            "actor:alice",
            "hivemind-ui",
            ProjectAnchorKind::Rig,
            "hivemind",
        )
        .is_err());

    // Folder anchors are not centrally unique — no refusal for a repeated folder value.
    commands
        .anchor_project(
            "actor:alice",
            "hivemind",
            ProjectAnchorKind::Folder,
            "services/billing",
        )
        .expect("first folder anchor succeeds");
    assert!(commands
        .anchor_project(
            "actor:alice",
            "hivemind-ui",
            ProjectAnchorKind::Folder,
            "services/billing",
        )
        .is_ok());
}

#[test]
fn unanchor_project_requires_an_active_anchor() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    commands
        .register_project("actor:alice", "billing", None, None)
        .expect("register billing");

    assert!(commands
        .unanchor_project("actor:alice", "billing", ProjectAnchorKind::Rig, "billing",)
        .is_err());

    commands
        .anchor_project("actor:alice", "billing", ProjectAnchorKind::Rig, "billing")
        .expect("anchor succeeds");
    commands
        .unanchor_project("actor:alice", "billing", ProjectAnchorKind::Rig, "billing")
        .expect("unanchor succeeds");

    // Once retracted, a different project may claim the same rig value.
    commands
        .register_project("actor:alice", "billing-two", None, None)
        .expect("register billing-two");
    assert!(commands
        .anchor_project(
            "actor:alice",
            "billing-two",
            ProjectAnchorKind::Rig,
            "billing",
        )
        .is_ok());
}

#[test]
fn propose_decision_normalizes_topic_keys() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option");
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Normalize topics",
            rationale: "Keep consistent filters",
            topic_keys: &[
                "  Crème brûlée API!!  ".to_owned(),
                "Ops___SRE   Alerts".to_owned(),
            ],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
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
