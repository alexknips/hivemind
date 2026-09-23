// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;

use proptest::prelude::*;
use serde_json::json;
use uuid::Uuid;

use crate::events::{
    EventProvenance, EventSource, EventType, HypothesisKind, ProjectAnchorKind, ProjectLinkKind,
    RelationKind,
};
use crate::ledger::{EventLedger, InMemoryEventLedger, SqliteEventLedger};

use super::{
    normalize_topic_key, Commands, DecisionProposalInput, GroundInput, Grounding, SupersedeInput,
    MAX_TITLE_LEN, MAX_TOPIC_KEY_LEN,
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
fn record_option_id_is_opaque_not_derived_from_label() {
    // hivemind-zdsh.10: the id used to be `option-<slugified-label>-<uuid>`, which made the id
    // and the label the same data wearing two hats (and produced 90+ char ids). The id must
    // now be an opaque handle with no trace of the label text.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_id = commands
        .record_option(
            "actor:carol",
            "Adopt a fully managed queue",
            "Ship async queue",
        )
        .expect("record option succeeds");

    assert!(!option_id.contains("adopt"));
    assert!(!option_id.contains("managed"));
    assert!(option_id.len() < 60, "id should be short: {option_id}");
}

#[test]
fn record_option_rejects_overlong_label() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let label = "x".repeat(81);
    assert!(commands
        .record_option("actor:carol", &label, "description")
        .is_err());
}

#[test]
fn record_option_rejects_label_that_encodes_the_choice() {
    // hivemind-zdsh.10 FINDING 3: the choice must never be encoded in the label — CHOSE is the
    // only representation of which option was picked.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .record_option(
            "actor:carol",
            "Personal project default: agent as PM bridge - chosen",
            "description"
        )
        .is_err());
}

#[test]
fn record_option_rejects_rejected_options_bucket_label() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .record_option("actor:carol", "Other rejected options", "description")
        .is_err());
}

#[test]
fn record_option_rejects_numbered_answer_bundle_label() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .record_option(
            "actor:carol",
            "Alex's answers 1a-2-3a-4a-5a-6a",
            "description"
        )
        .is_err());
}

#[test]
fn record_option_allows_a_short_clean_label() {
    // A lone numbered reference or version-looking token must not trip the bundling heuristic.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .record_option("actor:carol", "Ship phase 1 with v2 API", "description")
        .is_ok());
}

#[test]
fn record_option_handles_non_ascii_label_without_panicking() {
    // hivemind-zdsh.10 regression: the numbered-answer-token scan must not byte-index into a
    // token derived from a multi-byte UTF-8 label. Two numbered-looking tokens stay under the
    // bundling threshold, so this must succeed rather than panic or misfire.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert!(commands
        .record_option("actor:carol", "café 1a 2", "description")
        .is_ok());
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Pick queue strategy",
            rationale: "Need robust ingestion that survives a burst of traffic",
            topic_keys: &["Infra / Queue".to_owned()],
            option_ids: &[option_a.clone(), option_b.clone()],
            option_labels: &["A".to_owned(), "B".to_owned()],
            chosen_option_id: Some(option_b.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: std::slice::from_ref(&hypothesis_id),
            evidence_ids: std::slice::from_ref(&evidence_id),
            quote: None,
            question: None,
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
fn propose_decision_stores_paired_quote_and_question() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Personal projects are visible tenant-wide",
            rationale: "Spelled out: a personal project is visible to the whole tenant.",
            topic_keys: &["projects".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: Some("1a"),
            question: Some("Should a personal project be visible to the whole tenant?"),
        })
        .expect("propose decision with paired quote/question");

    let events = ledger.read(0, 20).expect("read events");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    assert_eq!(
        proposal.payload.get("quote").and_then(|v| v.as_str()),
        Some("1a")
    );
    assert_eq!(
        proposal.payload.get("question").and_then(|v| v.as_str()),
        Some("Should a personal project be visible to the whole tenant?")
    );
}

#[test]
fn propose_decision_rejects_quote_without_question() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision with an unexplained quote",
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: Some("1a"),
            question: None,
        })
        .expect_err("quote without question must be refused");
    assert!(
        error
            .to_string()
            .contains("quote and question must be given together"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_rejects_question_without_quote() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision with a question but no quote",
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: Some("Should a personal project be visible to the whole tenant?"),
        })
        .expect_err("question without quote must be refused");
    assert!(
        error
            .to_string()
            .contains("quote and question must be given together"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_rejects_rationale_shorter_than_the_minimum_length() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision with a stub rationale",
            rationale: "Because yes",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect_err("a too-short rationale must be refused");
    assert!(
        error.to_string().contains("must be at least 20 characters"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_rejects_rationale_with_too_few_words() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision with a fragment rationale",
            rationale: "Obviously-the-right-call",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect_err("a rationale with too few words must be refused");
    assert!(
        error.to_string().contains("must be at least 4 words"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_rejects_rationale_with_a_bare_list_reference() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision citing an external list",
            rationale: "Per the notes: verbatim 1a, 2. a clearer alternative was rejected",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect_err("a bare list-item reference must be refused without quote/question");
    assert!(
        error
            .to_string()
            .contains("rationale references a bare list item"),
        "unexpected error: {error}"
    );
    assert!(
        error.to_string().contains("pair quote with question"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_allows_a_list_shaped_rationale_when_quote_and_question_are_given() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Decision citing an external list, explained inline",
            rationale: "Per the notes: verbatim 1a, 2. a clearer alternative was rejected",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: Some("1a"),
            question: Some("Should the numbered alternative be adopted instead?"),
        })
        .expect("quote/question pair is the escape hatch for a list-shaped rationale");
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
                grounding: Grounding::NotAsked,
                expressed_confidence: None,
                actor_id,
                title: "Record direct agent provenance",
                rationale: "Agent-written decisions must be distinguishable from CLI writes",
                topic_keys: &["Integrations".to_owned()],
                option_ids: &[option_id],
                option_labels: &["Keep substrate small".to_owned()],
                chosen_option_id: None,
                decided_by: None,
                still_proposed: false,
                hypothesis_ids: &[],
                evidence_ids: &[],
                quote: None,
                question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Pick one",
            rationale: "Need to make progress on this before the deadline",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "agent:claude:scribe",
            title: "Human decides, agent records",
            rationale: "The human chose; the agent is only writing it down",
            topic_keys: &["governance".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Ship it".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: Some("human:alex"),
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: "agent:claude:scribe",
        title: "No chosen option yet",
        rationale: "Still an open proposal",
        topic_keys: &["governance".to_owned()],
        option_ids: &[option_id],
        option_labels: &["A".to_owned()],
        chosen_option_id: None,
        decided_by: Some("human:alex"),
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Chosen option, no decided_by",
            rationale: "The proposer is the decider here",
            topic_keys: &["Core".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Proposed with a leaning but not decided",
            rationale: "Awaiting someone else's decision",
            topic_keys: &["Core".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
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
        quote: None,
        question: None,
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
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
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
        quote: None,
        question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
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
            quote: None,
            question: None,
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
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
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
        quote: None,
        question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: &title,
            rationale: "This rationale is long enough to pass the minimum checks.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
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
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
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
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Ship v1.2.3 to prod.",
            rationale: "This rationale is long enough to pass the minimum checks.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This rationale is long enough to pass the minimum checks.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("decision");

    let new_title: String = "y".repeat(MAX_TITLE_LEN + 1);
    let result = commands.supersede(SupersedeInput {
        actor_id: "actor:alice",
        old_decision_id: &old_decision_id,
        new_title: &new_title,
        new_rationale: "This new rationale is long enough to pass the minimum checks.",
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
fn propose_decision_persists_option_descriptions_on_the_event() {
    // hivemind-zdsh.10: a description captured alongside an option's label used to be
    // discarded after `record_option` validated it — it never reached the ledger. It must now
    // land on the `DecisionProposed` event, index-aligned with option_ids/option_labels.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let option_a = commands
        .record_option(
            "actor:alice",
            "Amazon SQS",
            "Fully managed, less ops burden",
        )
        .expect("option a");
    let option_b = commands
        .record_option("actor:alice", "Kafka", "More control, more ops burden")
        .expect("option b");

    commands
        .propose_decision(DecisionProposalInput {
            actor_id: "actor:alice",
            title: "Pick a queue",
            rationale: "Need durable delivery with minimal operational overhead",
            topic_keys: &["infra".to_owned()],
            option_ids: &[option_a, option_b],
            option_labels: &["Amazon SQS".to_owned(), "Kafka".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose decision");

    let events = ledger.read(0, 20).expect("read events");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    let descriptions = proposal
        .payload
        .get("option_descriptions")
        .and_then(|value| value.as_array())
        .expect("option_descriptions array")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        descriptions,
        vec![
            "Fully managed, less ops burden",
            "More control, more ops burden",
        ]
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_a],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("decision a");

    let decision_b = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision B",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_b],
            option_labels: &["B".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("decision");

    let first = commands
        .supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "New rationale that explains the replacement decision.",
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
            new_rationale: "New rationale that explains the replacement decision.",
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
            new_rationale: "New rationale that explains the replacement decision.",
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Normalize topics",
            rationale: "Keep topic filters consistent across every capture surface",
            topic_keys: &[
                "  Crème brûlée API!!  ".to_owned(),
                "Ops___SRE   Alerts".to_owned(),
            ],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
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

// ── Grounding (hivemind-gwhr.1): FOLLOWS_FROM, hypothesis kind/check_by/would_change_if ──

fn propose_minimal_decision(commands: &Commands<'_, InMemoryEventLedger>, title: &str) -> String {
    let option_id = commands
        .record_option("actor:alice", "Only option", "The only option")
        .expect("record option");
    commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title,
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose minimal decision")
}

#[test]
fn propose_decision_with_declared_grounding_creates_follows_from_edge_with_causation() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let premise_id = propose_minimal_decision(&commands, "Prior goal decision");
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let premise_ids = vec![premise_id.clone()];

    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::Declared {
                premise_decision_ids: &premise_ids,
                evidence_ids: &[],
                hypothesis_ids: &[],
            },
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Follows from the prior goal",
            rationale: "Consistent with the earlier decision",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose decision with declared grounding");

    let events = ledger.read(0, 20).expect("read events");
    let proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(|v| v.as_str())
                    == Some(decision_id.as_str())
        })
        .expect("proposal event");
    let proposal_id = proposal.event_id.expect("proposal event id");

    let follows_from = events
        .iter()
        .find(|event| {
            event.event_type == EventType::RelationAdded
                && event.payload.get("relation") == Some(&json!(RelationKind::FollowsFrom))
        })
        .expect("FOLLOWS_FROM relation event present");
    assert_eq!(
        follows_from.payload.get("from_id").and_then(|v| v.as_str()),
        Some(decision_id.as_str())
    );
    assert_eq!(
        follows_from.payload.get("to_id").and_then(|v| v.as_str()),
        Some(premise_id.as_str())
    );
    assert_eq!(follows_from.causation_event_id, Some(proposal_id));
}

#[test]
fn propose_decision_rejects_declared_grounding_with_nothing_named() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::Declared {
                premise_decision_ids: &[],
                evidence_ids: &[],
                hypothesis_ids: &[],
            },
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision with declared-but-empty grounding",
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect_err("declared-but-empty grounding must be refused");
    assert!(
        error.to_string().contains(
            "grounding must name at least one premise decision, evidence item or hypothesis"
        ),
        "unexpected error: {error}"
    );

    // record_option only stages an id in in-memory CommandState (no ledger event of its
    // own — see the option_ids field); nothing else has been written, so "nothing written
    // on refusal" means the ledger is still empty here.
    assert_eq!(ledger.read(0, 20).expect("read events").len(), 0);
}

#[test]
fn propose_decision_rejects_self_premise() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    // Self-premise can only be caught inside propose_decision_with_id, where the real
    // decision id already exists — use it directly with a matching id.
    let decision_id = "decision-self-premise";
    let premise_ids = vec![decision_id.to_owned()];
    let error = commands
        .propose_decision_with_id(
            DecisionProposalInput {
                grounding: Grounding::Declared {
                    premise_decision_ids: &premise_ids,
                    evidence_ids: &[],
                    hypothesis_ids: &[],
                },
                expressed_confidence: None,
                actor_id: "actor:alice",
                title: "A decision naming itself as a premise",
                rationale: "Rationale text",
                topic_keys: &["topic".to_owned()],
                option_ids: std::slice::from_ref(&option_id),
                option_labels: &[],
                chosen_option_id: None,
                decided_by: None,
                still_proposed: true,
                hypothesis_ids: &[],
                evidence_ids: &[],
                quote: None,
                question: None,
            },
            decision_id,
            super::DecisionProposalEventUuids {
                proposal: Uuid::new_v4(),
                has_option: vec![Uuid::new_v4()],
                chose: None,
                assumes: Vec::new(),
                based_on: Vec::new(),
                follows_from: vec![Uuid::new_v4()],
            },
        )
        .expect_err("self-premise must be refused");
    assert!(
        error
            .to_string()
            .contains("a decision cannot be its own premise"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_rejects_nonexistent_premise() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let premise_ids = vec!["decision-does-not-exist".to_owned()];

    let error = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::Declared {
                premise_decision_ids: &premise_ids,
                evidence_ids: &[],
                hypothesis_ids: &[],
            },
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision naming a premise that doesn't exist",
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect_err("nonexistent premise must be refused");
    assert!(
        error.to_string().contains("decision does not exist"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_reports_stale_premise_when_superseded() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let premise_id = propose_minimal_decision(&commands, "Premise that will be superseded");
    let successor_id = propose_minimal_decision(&commands, "Successor decision");
    commands
        .supersede_decision(&premise_id, &successor_id, "actor:alice")
        .expect("supersede premise");

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let premise_ids = vec![premise_id.clone()];
    let decision_id = "decision-with-stale-premise";
    let result = commands
        .propose_decision_with_id(
            DecisionProposalInput {
                grounding: Grounding::Declared {
                    premise_decision_ids: &premise_ids,
                    evidence_ids: &[],
                    hypothesis_ids: &[],
                },
                expressed_confidence: None,
                actor_id: "actor:alice",
                title: "Rests on a since-superseded decision",
                rationale: "Rationale text",
                topic_keys: &["topic".to_owned()],
                option_ids: std::slice::from_ref(&option_id),
                option_labels: &[],
                chosen_option_id: None,
                decided_by: None,
                still_proposed: true,
                hypothesis_ids: &[],
                evidence_ids: &[],
                quote: None,
                question: None,
            },
            decision_id,
            super::DecisionProposalEventUuids {
                proposal: Uuid::new_v4(),
                has_option: vec![Uuid::new_v4()],
                chose: None,
                assumes: Vec::new(),
                based_on: Vec::new(),
                follows_from: vec![Uuid::new_v4()],
            },
        )
        .expect("append-only: a stale premise is still linked, not refused");

    assert_eq!(result.premise_stale, vec![premise_id]);
}

#[test]
fn propose_decision_validates_expressed_confidence_vocabulary() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let error = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: Some("very high"),
            actor_id: "actor:alice",
            title: "Decision with an invalid confidence word",
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect_err("invalid expressed_confidence must be refused");
    assert!(
        error
            .to_string()
            .contains("expressed_confidence must be low, medium, or high"),
        "unexpected error: {error}"
    );
}

#[test]
fn propose_decision_stores_expressed_confidence_from_input() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: Some("medium"),
            actor_id: "actor:alice",
            title: "Decision with a stated confidence",
            rationale: "Rationale text",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose decision with expressed_confidence");

    let events = ledger.read(0, 20).expect("read events");
    let proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(|v| v.as_str())
                    == Some(decision_id.as_str())
        })
        .expect("proposal event");
    assert_eq!(
        proposal
            .payload
            .get("expressed_confidence")
            .and_then(|v| v.as_str()),
        Some("medium")
    );
}

#[test]
fn record_hypothesis_defaults_to_assumption_kind() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let hypothesis_id = commands
        .record_hypothesis("actor:alice", "The cache will help")
        .expect("record hypothesis");

    let events = ledger.read(0, 5).expect("read events");
    let event = events
        .iter()
        .find(|event| {
            event.event_type == EventType::HypothesisRecorded
                && event.payload.get("hypothesis_id").and_then(|v| v.as_str())
                    == Some(hypothesis_id.as_str())
        })
        .expect("hypothesis event");
    assert_eq!(
        event.payload.get("kind").and_then(|v| v.as_str()),
        Some("assumption"),
        "kind is #[serde(default)] (missing on old events defaults to assumption on replay), \
         not skip_serializing_if — new events always state it explicitly"
    );
}

#[test]
fn record_hypothesis_with_kind_records_bet_fields() {
    use chrono::TimeZone;

    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let check_by = chrono::Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();

    let hypothesis_id = commands
        .record_hypothesis_with_kind(
            "actor:alice",
            "Latency stays under 50ms at 10x load",
            HypothesisKind::Bet,
            Some(check_by),
            Some("A 10x load test shows p99 above 50ms"),
        )
        .expect("record bet hypothesis");

    let events = ledger.read(0, 5).expect("read events");
    let event = events
        .iter()
        .find(|event| {
            event.event_type == EventType::HypothesisRecorded
                && event.payload.get("hypothesis_id").and_then(|v| v.as_str())
                    == Some(hypothesis_id.as_str())
        })
        .expect("hypothesis event");
    assert_eq!(
        event.payload.get("kind").and_then(|v| v.as_str()),
        Some("bet")
    );
    assert!(event
        .payload
        .get("check_by")
        .and_then(|v| v.as_str())
        .is_some());
    assert_eq!(
        event
            .payload
            .get("would_change_if")
            .and_then(|v| v.as_str()),
        Some("A 10x load test shows p99 above 50ms")
    );
}

#[test]
fn record_hypothesis_with_kind_rejects_empty_would_change_if() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let error = commands
        .record_hypothesis_with_kind(
            "actor:alice",
            "A bet with a blank would_change_if",
            HypothesisKind::Bet,
            None,
            Some("   "),
        )
        .expect_err("empty would_change_if must be refused");
    assert!(
        error.to_string().contains("would_change_if"),
        "unexpected error: {error}"
    );
}

#[test]
fn link_follows_from_creates_edge_without_causation() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let premise_id = propose_minimal_decision(&commands, "Premise decision");
    let decision_id = propose_minimal_decision(&commands, "Decision that follows from it");

    let event_id = commands
        .link_follows_from(&decision_id, &premise_id, "actor:bob")
        .expect("link follows_from");

    let events = ledger.read(0, 20).expect("read events");
    let relation = events
        .iter()
        .find(|event| event.event_id == Some(event_id))
        .expect("relation event present");
    assert_eq!(relation.causation_event_id, None);
    assert_eq!(
        relation.payload.get("relation"),
        Some(&json!(RelationKind::FollowsFrom))
    );
}

#[test]
fn link_follows_from_rejects_self_premise() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose_minimal_decision(&commands, "A decision");

    let error = commands
        .link_follows_from(&decision_id, &decision_id, "actor:bob")
        .expect_err("self-premise must be refused");
    assert!(
        error
            .to_string()
            .contains("a decision cannot be its own premise"),
        "unexpected error: {error}"
    );
}

#[test]
fn attach_evidence_names_follows_from_when_target_is_a_decision() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose_minimal_decision(&commands, "Decision A");
    let other_decision_id = propose_minimal_decision(&commands, "Decision B");

    let error = commands
        .attach_evidence(&decision_id, &other_decision_id, "actor:bob")
        .expect_err("BASED_ON must keep refusing a decision target");
    assert!(
        error.to_string().contains("FOLLOWS_FROM"),
        "error should name FOLLOWS_FROM as the right relation: {error}"
    );
}

#[test]
fn ground_decision_appends_grounding_without_causation() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let premise_id = propose_minimal_decision(&commands, "Premise decision");
    let decision_id = propose_minimal_decision(&commands, "Decision to ground later");
    let evidence_id = commands
        .record_evidence("actor:bob", "Later-attached evidence")
        .expect("record evidence");
    let hypothesis_id = commands
        .record_hypothesis("actor:bob", "Later-attached assumption")
        .expect("record hypothesis");

    let premise_ids = vec![premise_id.clone()];
    let evidence_ids = vec![evidence_id.clone()];
    let hypothesis_ids = vec![hypothesis_id.clone()];
    let event_ids = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: &decision_id,
            premise_decision_ids: &premise_ids,
            evidence_ids: &evidence_ids,
            hypothesis_ids: &hypothesis_ids,
        })
        .expect("ground decision");
    assert_eq!(event_ids.len(), 3);

    let events = ledger.read(0, 30).expect("read events");
    for event_id in &event_ids {
        let event = events
            .iter()
            .find(|event| event.event_id == Some(*event_id))
            .expect("grounding event present");
        assert_eq!(
            event.causation_event_id, None,
            "later grounding never carries causation"
        );
    }
}

#[test]
fn ground_decision_rejects_nothing_named() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose_minimal_decision(&commands, "Decision to ground");

    let error = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: &decision_id,
            premise_decision_ids: &[],
            evidence_ids: &[],
            hypothesis_ids: &[],
        })
        .expect_err("empty grounding must be refused");
    assert!(
        error.to_string().contains(
            "grounding must name at least one premise decision, evidence item or hypothesis"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn ground_decision_rejects_self_premise() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose_minimal_decision(&commands, "Decision to ground");
    let premise_ids = vec![decision_id.clone()];

    let error = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: &decision_id,
            premise_decision_ids: &premise_ids,
            evidence_ids: &[],
            hypothesis_ids: &[],
        })
        .expect_err("self-premise must be refused");
    assert!(
        error
            .to_string()
            .contains("a decision cannot be its own premise"),
        "unexpected error: {error}"
    );
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
