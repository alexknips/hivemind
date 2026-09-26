// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fs;

use proptest::prelude::*;
use serde_json::json;
use uuid::Uuid;

use crate::events::{
    EventProvenance, EventSource, EventType, HypothesisKind, ProjectAnchorKind, ProjectLinkKind,
    ProjectSource, RelationKind,
};
use crate::ledger::{EventLedger, InMemoryEventLedger, SqliteEventLedger};

use super::{
    agent_actor_session, normalize_topic_key, personal_project_handle, Commands, DecisionPlacement,
    DecisionProposalInput, DeterminedProject, GroundInput, Grounding, GroundingPlan, NewBet,
    NewEvidence, RestsOnKind, SupersedeInput, MAX_TITLE_LEN, MAX_TOPIC_KEY_LEN,
    PERSONAL_FALLBACK_NOTICE,
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
            project: None,
            actor_id: "actor:alice",
            title: "Pick queue strategy",
            rationale: "Need robust ingestion that survives a burst of traffic",
            topic_keys: &["Infra / Queue".to_owned()],
            option_ids: &[option_a.clone(), option_b.clone()],
            option_labels: &["A".to_owned(), "B".to_owned()],
            chosen_option_id: Some(option_b.as_str()),
            decided_by: None,
            delegated_by: None,
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
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: Some("1a"),
            question: Some("Should a personal project be visible to the whole tenant?"),
            project: None,
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
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: Some("1a"),
            question: None,
            project: None,
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
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: Some("Should a personal project be visible to the whole tenant?"),
            project: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision with a stub rationale",
            rationale: "Because yes",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision with a fragment rationale",
            rationale: "Obviously-the-right-call",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision citing an external list",
            rationale: "Per the notes: verbatim 1a, 2. a clearer alternative was rejected",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision citing an external list, explained inline",
            rationale: "Per the notes: verbatim 1a, 2. a clearer alternative was rejected",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: Some("1a"),
            question: Some("Should the numbered alternative be adopted instead?"),
        })
        .expect("quote/question pair is the escape hatch for a list-shaped rationale");
}

#[test]
fn propose_decision_refuses_unregistered_project_handle() {
    // Write rule (approved record shape, item 2): a stated handle must be registered,
    // else a refusal that names the handle and the register command.
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
            title: "File under an unregistered project",
            rationale: "Billing owns this call so the record belongs under its project",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Option A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project: Some(DeterminedProject::stated("billing")),
        })
        .expect_err("unregistered project handle must be refused");
    let message = error.to_string();
    assert!(message.contains("billing"), "unexpected error: {message}");
    assert!(
        message.contains("hivemind project register"),
        "refusal must name the register command: {message}"
    );
    assert!(ledger.read(0, 10).expect("read succeeds").is_empty());
}

#[test]
fn propose_decision_accepts_registered_project_handle_and_records_stated_source() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands
        .register_project("actor:alice", "billing", Some("Billing"), None)
        .expect("register succeeds");
    commands
        .declare_project_topic("actor:alice", "billing", "topic")
        .expect("declare topic");
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");

    commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "File under billing",
            rationale: "Billing owns this call so the record belongs under its project",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Option A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project: Some(DeterminedProject::stated("billing")),
        })
        .expect("propose decision with a registered project succeeds");

    let events = ledger.read(0, 10).expect("read succeeds");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    assert_eq!(
        proposal.payload.get("project").and_then(|v| v.as_str()),
        Some("billing")
    );
    assert_eq!(
        proposal
            .payload
            .get("project_source")
            .and_then(|v| v.as_str()),
        Some("stated")
    );
}

#[test]
fn propose_decision_without_project_records_personal_fallback_and_no_handle() {
    // No handle given: the write layer records project_source = personal_fallback and
    // leaves `project` unset on the payload -- the projector derives the personal address
    // from actor_id, the write layer never stores it (approved record shape, item 2).
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
            title: "File with no project stated",
            rationale: "No project was named so the record lands in the personal project",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Option A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project: None,
        })
        .expect("propose decision without a project succeeds");

    let events = ledger.read(0, 10).expect("read succeeds");
    let proposal = events
        .iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    assert!(
        proposal.payload.get("project").is_none(),
        "no project field must be written when no handle was given"
    );
    assert_eq!(
        proposal
            .payload
            .get("project_source")
            .and_then(|v| v.as_str()),
        Some("personal_fallback")
    );
}

#[test]
fn propose_decision_rejects_reserved_personal_prefix_as_stated_project() {
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
            title: "State a personal project directly",
            rationale: "Personal addresses are derived from the actor and never typed",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Option A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project: Some(DeterminedProject::stated("personal:alice")),
        })
        .expect_err("a stated personal: handle must be refused");
    assert!(
        error.to_string().contains("personal:"),
        "unexpected error: {error}"
    );
    assert!(ledger.read(0, 10).expect("read succeeds").is_empty());
}

#[test]
fn supersede_inherits_old_decision_project_when_not_restated() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands
        .register_project("actor:alice", "billing", None, None)
        .expect("register succeeds");
    commands
        .declare_project_topic("actor:alice", "billing", "topic")
        .expect("declare topic");
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let old_decision_id = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "Decision A is filed under billing so later readers can find it",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Option A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project: Some(DeterminedProject::stated("billing")),
        })
        .expect("propose decision A");

    let outcome = commands
        .supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "Decision B replaces decision A because the billing plan changed",
            topic_keys: &["topic".to_owned()],
            option_labels: &["Option A".to_owned()],
            chosen_option_label: Some("Option A"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project: None,
            grounding: None,
            expressed_confidence: None,
        })
        .expect("supersede succeeds");

    let events = ledger.read(0, 20).expect("read succeeds");
    let new_proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(|v| v.as_str())
                    == Some(outcome.new_decision_id.as_str())
        })
        .expect("new proposal event present");
    assert_eq!(
        new_proposal.payload.get("project").and_then(|v| v.as_str()),
        Some("billing"),
        "supersede must inherit the old decision's project when not restated"
    );
    assert_eq!(
        new_proposal
            .payload
            .get("project_source")
            .and_then(|v| v.as_str()),
        Some("stated"),
        "the inherited project_source must carry over verbatim, not be overwritten"
    );
}

#[test]
fn supersede_overrides_project_when_explicitly_stated() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands
        .register_project("actor:alice", "billing", None, None)
        .expect("register billing succeeds");
    commands
        .declare_project_topic("actor:alice", "billing", "topic")
        .expect("declare topic");
    commands
        .register_project("actor:alice", "payments", None, None)
        .expect("register payments succeeds");
    commands
        .declare_project_topic("actor:alice", "payments", "topic")
        .expect("declare topic");
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let old_decision_id = commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "Decision A is filed under billing so later readers can find it",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Option A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project: Some(DeterminedProject::stated("billing")),
        })
        .expect("propose decision A");

    let outcome = commands
        .supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "Decision B replaces decision A because the billing plan changed",
            topic_keys: &["topic".to_owned()],
            option_labels: &["Option A".to_owned()],
            chosen_option_label: Some("Option A"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project: Some(DeterminedProject::stated("payments")),
            grounding: None,
            expressed_confidence: None,
        })
        .expect("supersede succeeds");

    let events = ledger.read(0, 20).expect("read succeeds");
    let new_proposal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && event.payload.get("decision_id").and_then(|v| v.as_str())
                    == Some(outcome.new_decision_id.as_str())
        })
        .expect("new proposal event present");
    assert_eq!(
        new_proposal.payload.get("project").and_then(|v| v.as_str()),
        Some("payments"),
        "an explicit project on supersede must override the inherited one"
    );
}

/// One registered-handles ledger fixture for the placement tests below: owns the option and
/// topic data a `DecisionProposalInput` borrows, so a proposal can be built in one call.
struct PlacementFixture {
    option_id: String,
    option_labels: [String; 1],
    topic_keys: [String; 1],
}

impl PlacementFixture {
    /// Registers `handles` and records one option.
    fn new(commands: &Commands<'_, InMemoryEventLedger>, handles: &[&str]) -> Self {
        for handle in handles {
            commands
                .register_project("actor:alice", handle, None, None)
                .expect("register succeeds");
            commands
                .declare_project_topic("actor:alice", handle, "topic")
                .expect("declare topic");
        }
        let option_id = commands
            .record_option("actor:alice", "A", "Option A")
            .expect("option a");
        Self {
            option_id,
            option_labels: ["Option A".to_owned()],
            topic_keys: ["topic".to_owned()],
        }
    }

    fn proposal<'a>(
        &'a self,
        actor_id: &'a str,
        title: &'a str,
        project: Option<DeterminedProject<'a>>,
    ) -> DecisionProposalInput<'a> {
        DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id,
            title,
            rationale: "Billing owns this call so the record belongs under its project",
            topic_keys: &self.topic_keys,
            option_ids: std::slice::from_ref(&self.option_id),
            option_labels: &self.option_labels,
            chosen_option_id: Some(self.option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            project,
        }
    }
}

#[test]
fn propose_decision_placed_reports_where_the_decision_landed() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);

    let (_, stated) = commands
        .propose_decision_placed(fixture.proposal(
            "agent:claude:session-1",
            "Stated project",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("stated project succeeds");
    assert_eq!(stated.project, "billing");
    assert_eq!(stated.project_source, ProjectSource::Stated);
    assert_eq!(
        stated.notice(),
        None,
        "a determined project is not a fallback"
    );

    // The caller's account of how it got the handle is recorded as told.
    let (_, from_marker) = commands
        .propose_decision_placed(fixture.proposal(
            "agent:claude:session-1",
            "Project from a folder marker",
            Some(DeterminedProject {
                handle: "billing",
                source: ProjectSource::FolderMarker,
            }),
        ))
        .expect("folder marker project succeeds");
    assert_eq!(from_marker.project_source, ProjectSource::FolderMarker);

    // No handle: the derived personal address, announced.
    let (_, fallback) = commands
        .propose_decision_placed(fixture.proposal(
            "agent:claude:session-1",
            "No project stated",
            None,
        ))
        .expect("fallback succeeds");
    assert_eq!(fallback.project, "personal:agent:claude");
    assert_eq!(fallback.project_source, ProjectSource::PersonalFallback);
    assert_eq!(fallback.notice(), Some(PERSONAL_FALLBACK_NOTICE));
    assert!(PERSONAL_FALLBACK_NOTICE.contains("saved to your personal project"));

    let recorded: Vec<(Option<String>, Option<String>)> = ledger
        .read(0, 100)
        .expect("read succeeds")
        .iter()
        .filter(|event| event.event_type == EventType::DecisionProposed)
        .map(|event| {
            (
                event
                    .payload
                    .get("project")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                event
                    .payload
                    .get("project_source")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
            )
        })
        .collect();
    assert_eq!(
        recorded,
        vec![
            (Some("billing".to_owned()), Some("stated".to_owned())),
            (Some("billing".to_owned()), Some("folder_marker".to_owned())),
            (None, Some("personal_fallback".to_owned())),
        ],
        "the reply and the ledger say the same thing"
    );
}

#[test]
fn a_project_source_only_the_write_layer_may_record_is_refused_with_a_handle() {
    // `personal_fallback` is what this layer records when no handle is given, and `moved` is
    // what a move records; a caller stating a handle may claim neither.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let events_before = ledger.read(0, 100).expect("read succeeds").len();

    for source in [ProjectSource::PersonalFallback, ProjectSource::Moved] {
        let error = commands
            .propose_decision_placed(fixture.proposal(
                "actor:alice",
                "Claims a source only HiveMind records",
                Some(DeterminedProject {
                    handle: "billing",
                    source,
                }),
            ))
            .expect_err("a source only the write layer records must be refused");
        let message = error.to_string();
        assert!(
            message.contains(source.as_str())
                && message.contains("cannot accompany a project handle"),
            "unexpected error: {message}"
        );
    }
    assert_eq!(
        ledger.read(0, 100).expect("read succeeds").len(),
        events_before,
        "a refused capture writes nothing"
    );
}

#[test]
fn supersede_reports_the_inherited_or_stated_placement() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing", "payments"]);
    let (old_id, _) = commands
        .propose_decision_placed(fixture.proposal(
            "actor:alice",
            "Decision A",
            Some(DeterminedProject {
                handle: "billing",
                source: ProjectSource::Rig,
            }),
        ))
        .expect("propose decision A");

    let supersede = |old: &str, project: Option<DeterminedProject<'_>>| {
        commands.supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: old,
            new_title: "Decision B",
            new_rationale: "Decision B replaces decision A because the billing plan changed",
            topic_keys: &["topic".to_owned()],
            option_labels: &["Option A".to_owned()],
            chosen_option_label: Some("Option A"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project,
            grounding: None,
            expressed_confidence: None,
        })
    };

    // Not stated: inherits the old decision's project and how it was determined.
    let inherited = supersede(&old_id, None).expect("supersede inherits");
    assert_eq!(inherited.placement.project, "billing");
    assert_eq!(inherited.placement.project_source, ProjectSource::Rig);
    assert_eq!(inherited.placement.notice(), None);

    // Stated: overrides it.
    let stated = supersede(
        &inherited.new_decision_id,
        Some(DeterminedProject::stated("payments")),
    )
    .expect("supersede states a project");
    assert_eq!(stated.placement.project, "payments");
    assert_eq!(stated.placement.project_source, ProjectSource::Stated);

    // A refused override supersedes nothing.
    let events_before = ledger.read(0, 100).expect("read succeeds").len();
    supersede(
        &stated.new_decision_id,
        Some(DeterminedProject {
            handle: "payments",
            source: ProjectSource::Moved,
        }),
    )
    .expect_err("moved cannot accompany a handle");
    assert_eq!(
        ledger.read(0, 100).expect("read succeeds").len(),
        events_before
    );
}

#[test]
fn supersede_of_a_personal_fallback_decision_stays_announced() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &[]);
    let (old_id, _) = commands
        .propose_decision_placed(fixture.proposal("agent:claude:session-1", "Decision A", None))
        .expect("propose decision A");

    let outcome = commands
        .supersede(SupersedeInput {
            actor_id: "agent:claude:session-1",
            old_decision_id: &old_id,
            new_title: "Decision B",
            new_rationale: "Decision B replaces decision A because the plan changed",
            topic_keys: &["topic".to_owned()],
            option_labels: &["Option A".to_owned()],
            chosen_option_label: Some("Option A"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project: None,
            grounding: None,
            expressed_confidence: None,
        })
        .expect("supersede succeeds");
    assert_eq!(outcome.placement.project, "personal:agent:claude");
    assert_eq!(
        outcome.placement.project_source,
        ProjectSource::PersonalFallback
    );
    assert_eq!(outcome.placement.notice(), Some(PERSONAL_FALLBACK_NOTICE));
}

#[test]
fn personal_project_handle_strips_session_from_agent_actor() {
    // Personal address rule (Alex, choice 3a): agent:<tool>:<session> becomes
    // personal:agent:<tool> -- the session is provenance, not part of the address.
    assert_eq!(
        personal_project_handle("agent:claude:scribe-42"),
        "personal:agent:claude"
    );
    assert_eq!(
        personal_project_handle("agent:codex:furiosa"),
        "personal:agent:codex"
    );
}

#[test]
fn personal_project_handle_keeps_human_actor_id_intact() {
    // human:<id> has no session component to remove.
    assert_eq!(
        personal_project_handle("human:alice"),
        "personal:human:alice"
    );
}

#[test]
fn agent_actor_session_is_the_part_the_personal_address_drops() {
    assert_eq!(
        agent_actor_session("agent:claude:scribe-42"),
        Some("scribe-42")
    );
    // Only the first colon after the tool splits; the rest is the session.
    assert_eq!(
        agent_actor_session("agent:codex:furiosa/session-1"),
        Some("furiosa/session-1")
    );
    // No session component: a human, a bare tool address, or an unconventional id.
    assert_eq!(agent_actor_session("human:alice"), None);
    assert_eq!(agent_actor_session("agent:claude"), None);
    assert_eq!(agent_actor_session("actor:bob"), None);
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
                project: None,
                actor_id,
                title: "Record direct agent provenance",
                rationale: "Agent-written decisions must be distinguishable from CLI writes",
                topic_keys: &["Integrations".to_owned()],
                option_ids: &[option_id],
                option_labels: &["Keep substrate small".to_owned()],
                chosen_option_id: None,
                decided_by: None,
                delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Pick one",
            rationale: "Need to make progress on this before the deadline",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "agent:claude:scribe",
            title: "Human decides, agent records",
            rationale: "The human chose; the agent is only writing it down",
            topic_keys: &["governance".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Ship it".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: Some("human:alex"),
            delegated_by: None,
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
        project: None,
        actor_id: "agent:claude:scribe",
        title: "No chosen option yet",
        rationale: "Still an open proposal",
        topic_keys: &["governance".to_owned()],
        option_ids: &[option_id],
        option_labels: &["A".to_owned()],
        chosen_option_id: None,
        decided_by: Some("human:alex"),
        delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Chosen option, no decided_by",
            rationale: "The proposer is the decider here",
            topic_keys: &["Core".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Proposed with a leaning but not decided",
            rationale: "Awaiting someone else's decision",
            topic_keys: &["Core".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: Some(option_id.as_str()),
            decided_by: None,
            delegated_by: None,
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
        project: None,
        actor_id: "actor:alice",
        title: "Contradictory flags",
        rationale: "still_proposed and decided_by disagree about whether this is decided",
        topic_keys: &["Core".to_owned()],
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &[],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: Some("human:alex"),
        delegated_by: None,
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

/// An agent capture with a chosen option, ready for the delegation tests to vary
/// (`DecisionProposalInput` is `Copy`, so tests use struct-update syntax on this base).
fn agent_capture_input<'a>(
    actor_id: &'a str,
    option_ids: &'a [String],
    labels: &'a [String],
    topic_keys: &'a [String],
) -> DecisionProposalInput<'a> {
    DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        project: None,
        actor_id,
        title: "Agent decides within a delegated scope",
        rationale: "The owner delegated this class of small choice to the agent up front",
        topic_keys,
        option_ids,
        option_labels: labels,
        chosen_option_id: option_ids.first().map(String::as_str),
        decided_by: None,
        delegated_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
    }
}

fn accepted_events(ledger: &InMemoryEventLedger) -> Vec<crate::events::Event> {
    ledger
        .read(0, 50)
        .expect("read events")
        .into_iter()
        .filter(|event| event.event_type == EventType::DecisionAccepted)
        .collect()
}

#[test]
fn propose_decision_with_delegated_by_records_the_marker_on_the_agents_self_acceptance() {
    // hivemind-zdsh.6, case 2 of Alex's attribution ruling: an agent decides for itself
    // within a scope a human delegated. The agent is still the decider (self-accept), and
    // the delegation rides on that acceptance rather than on a new node or edge kind.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let agent = "agent:claude:builder";
    let option_id = commands
        .record_option(agent, "Ship it", "Option A")
        .expect("option");
    let option_ids = [option_id];
    let labels = ["Ship it".to_owned()];
    let topics = ["governance".to_owned()];

    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            delegated_by: Some("human:alex"),
            ..agent_capture_input(agent, &option_ids, &labels, &topics)
        })
        .expect("delegated capture succeeds");

    let accepted = accepted_events(&ledger);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].actor_id, agent);
    assert_eq!(
        accepted[0].payload,
        json!({ "decision_id": decision_id, "delegated_by": "human:alex" })
    );
    let proposal = ledger
        .read(0, 50)
        .expect("read events")
        .into_iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal event present");
    assert!(
        proposal.payload.get("delegated_by").is_none(),
        "the marker belongs on the acceptance, not the proposal"
    );
}

#[test]
fn propose_decision_without_delegated_by_leaves_the_agents_self_acceptance_unmarked() {
    // Case 3 (agent decides alone) must stay distinguishable from case 2: no marker.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let agent = "agent:claude:builder";
    let option_id = commands
        .record_option(agent, "Ship it", "Option A")
        .expect("option");
    let option_ids = [option_id];
    let labels = ["Ship it".to_owned()];
    let topics = ["governance".to_owned()];

    let decision_id = commands
        .propose_decision(agent_capture_input(agent, &option_ids, &labels, &topics))
        .expect("capture succeeds");

    let accepted = accepted_events(&ledger);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].actor_id, agent);
    assert_eq!(accepted[0].payload, json!({ "decision_id": decision_id }));
}

#[test]
fn propose_decision_delegated_by_accepts_a_decided_by_naming_the_recording_agent() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let agent = "agent:claude:builder";
    let option_id = commands
        .record_option(agent, "Ship it", "Option A")
        .expect("option");
    let option_ids = [option_id];
    let labels = ["Ship it".to_owned()];
    let topics = ["governance".to_owned()];

    commands
        .propose_decision(DecisionProposalInput {
            decided_by: Some(agent),
            delegated_by: Some("human:alex"),
            ..agent_capture_input(agent, &option_ids, &labels, &topics)
        })
        .expect("decided_by naming the recorder is the same self-accept");

    let accepted = accepted_events(&ledger);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].actor_id, agent);
    assert_eq!(
        accepted[0].payload.get("delegated_by"),
        Some(&json!("human:alex"))
    );
}

#[test]
fn propose_decision_refuses_invalid_delegation_before_writing_anything() {
    let agent = "agent:claude:builder";
    let topics = ["governance".to_owned()];
    let labels = ["Ship it".to_owned()];

    struct Refused<'a> {
        actor: &'a str,
        chosen: bool,
        still_proposed: bool,
        decided_by: Option<&'a str>,
        delegated_by: &'a str,
        why: &'a str,
    }
    let cases = [
        Refused {
            actor: agent,
            chosen: false,
            still_proposed: false,
            decided_by: None,
            delegated_by: "human:alex",
            why: "no chosen option to qualify",
        },
        Refused {
            actor: agent,
            chosen: true,
            still_proposed: true,
            decided_by: None,
            delegated_by: "human:alex",
            why: "open recommendation is not decided",
        },
        Refused {
            actor: agent,
            chosen: true,
            still_proposed: false,
            decided_by: Some("human:sam"),
            delegated_by: "human:alex",
            why: "someone else decided; decided_by already names them",
        },
        Refused {
            actor: agent,
            chosen: true,
            still_proposed: false,
            decided_by: None,
            delegated_by: "agent:claude:other",
            why: "delegator is not a human",
        },
        Refused {
            actor: agent,
            chosen: true,
            still_proposed: false,
            decided_by: None,
            delegated_by: "alex",
            why: "delegator is not a typed human id",
        },
        Refused {
            actor: "human:sam",
            chosen: true,
            still_proposed: false,
            decided_by: None,
            delegated_by: "human:alex",
            why: "a human recorder is not an agent deciding under a delegation",
        },
    ];

    for Refused {
        actor,
        chosen,
        still_proposed,
        decided_by,
        delegated_by,
        why,
    } in cases
    {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);
        let option_id = commands
            .record_option(actor, "Ship it", "Option A")
            .expect("option");
        let option_ids = [option_id];
        let base = agent_capture_input(actor, &option_ids, &labels, &topics);

        let result = commands.propose_decision(DecisionProposalInput {
            chosen_option_id: if chosen { base.chosen_option_id } else { None },
            still_proposed,
            decided_by,
            delegated_by: Some(delegated_by),
            ..base
        });

        assert!(result.is_err(), "must be refused: {why}");
        assert!(
            ledger.read(0, 50).expect("read events").is_empty(),
            "a refused delegated capture must not leave a half-written proposal behind ({why})"
        );
    }
}

#[test]
fn accept_decision_delegated_marks_an_agents_acceptance_of_its_own_proposal() {
    // The still-proposed-then-accepted route: the agent first recorded an open
    // recommendation, then decides it under a delegation.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let agent = "agent:claude:builder";
    let option_id = commands
        .record_option(agent, "Ship it", "Option A")
        .expect("option");
    let option_ids = [option_id];
    let labels = ["Ship it".to_owned()];
    let topics = ["governance".to_owned()];
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            still_proposed: true,
            ..agent_capture_input(agent, &option_ids, &labels, &topics)
        })
        .expect("open recommendation");
    assert!(accepted_events(&ledger).is_empty());

    commands
        .accept_decision_delegated(&decision_id, agent, "human:alex")
        .expect("agent accepts its own proposal under a delegation");

    let accepted = accepted_events(&ledger);
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0].payload,
        json!({ "decision_id": decision_id, "delegated_by": "human:alex" })
    );
}

#[test]
fn accept_decision_delegated_refuses_a_decision_someone_else_proposed() {
    // A delegation qualifies an agent's OWN decision. Accepting another actor's proposal is
    // peer review, recorded plainly by `accept_decision`; it cannot borrow a delegation.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let proposer = "agent:claude:builder";
    let option_id = commands
        .record_option(proposer, "Ship it", "Option A")
        .expect("option");
    let option_ids = [option_id];
    let labels = ["Ship it".to_owned()];
    let topics = ["governance".to_owned()];
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            still_proposed: true,
            ..agent_capture_input(proposer, &option_ids, &labels, &topics)
        })
        .expect("open recommendation");

    let other_agent =
        commands.accept_decision_delegated(&decision_id, "agent:claude:other", "human:alex");
    assert!(
        other_agent.is_err(),
        "another agent must not accept under a delegation"
    );
    let human = commands.accept_decision_delegated(&decision_id, "human:sam", "human:alex");
    assert!(human.is_err(), "a human accepter carries no delegation");
    let missing = commands.accept_decision_delegated("decision-missing", proposer, "human:alex");
    assert!(missing.is_err(), "an unknown decision cannot be accepted");
    assert!(
        accepted_events(&ledger).is_empty(),
        "no refused delegated accept may reach the ledger"
    );

    commands
        .accept_decision(&decision_id, "agent:claude:other")
        .expect("plain peer acceptance stays available");
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
        project: None,
        actor_id: "actor:alice",
        title: "Mismatched labels",
        rationale: "option_labels shorter than option_ids and non-empty",
        topic_keys: &["Core".to_owned()],
        option_ids: &[option_a, option_b],
        option_labels: &["Only one label".to_owned()],
        chosen_option_id: None,
        decided_by: None,
        delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: run_on_title,
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
        project: None,
        actor_id: "actor:alice",
        title: &title,
        rationale: "rationale",
        topic_keys: &["Core".to_owned()],
        option_ids: &[option_id],
        option_labels: &[],
        chosen_option_id: None,
        decided_by: None,
        delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: &title,
            rationale: "This rationale is long enough to pass the minimum checks.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Use SQLite for slice 1. Migrate to Postgres later.",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "1) Use SQLite 2) Add WAL mode",
            rationale: "rationale",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &[],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Ship v1.2.3 to prod.",
            rationale: "This rationale is long enough to pass the minimum checks.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This rationale is long enough to pass the minimum checks.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("decision");

    let new_title: String = "y".repeat(MAX_TITLE_LEN + 1);
    let result = commands.supersede(SupersedeInput {
        project: None,
        actor_id: "actor:alice",
        old_decision_id: &old_decision_id,
        new_title: &new_title,
        new_rationale: "This new rationale is long enough to pass the minimum checks.",
        topic_keys: &[],
        option_labels: &["Replacement".to_owned()],
        chosen_option_label: None,
        hypothesis_ids: &[],
        evidence_ids: &[],
        grounding: None,
        expressed_confidence: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Pick a queue",
            rationale: "Need durable delivery with minimal operational overhead",
            topic_keys: &["infra".to_owned()],
            option_ids: &[option_a, option_b],
            option_labels: &["Amazon SQS".to_owned(), "Kafka".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_a],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Decision B",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_b],
            option_labels: &["B".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("decision");

    let first = commands
        .supersede(SupersedeInput {
            project: None,
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "New rationale that explains the replacement decision.",
            topic_keys: &[],
            option_labels: &["Replacement".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
            grounding: None,
            expressed_confidence: None,
        })
        .expect("supersede succeeds");
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let second = commands
        .supersede(SupersedeInput {
            project: None,
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Decision B",
            new_rationale: "New rationale that explains the replacement decision.",
            topic_keys: &[],
            option_labels: &["Replacement".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
            grounding: None,
            expressed_confidence: None,
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
            project: None,
            actor_id: "actor:alice",
            old_decision_id: "decision-missing",
            new_title: "Decision B",
            new_rationale: "New rationale that explains the replacement decision.",
            topic_keys: &["Core".to_owned()],
            option_labels: &["Replacement".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
            grounding: None,
            expressed_confidence: None,
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
            project: None,
            actor_id: "actor:alice",
            title: "Decision A",
            rationale: "This is a self-contained rationale for the test decision.",
            topic_keys: &["Core".to_owned()],
            option_ids: &[option_id],
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
fn move_decision_appends_event_and_is_reversible() {
    // Approved record shape, item 3: decision.moved {decision_id, from, to, reason?},
    // and a reversal is another recorded move -- nothing is deleted or rewritten.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing", "pricing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let move_event_id = commands
        .move_decision(
            "actor:alice",
            &decision_id,
            "billing",
            "pricing",
            Some("per-seat pricing decisions live under Pricing"),
        )
        .expect("move succeeds");

    let events = ledger.read(0, 10).expect("read succeeds");
    let moved = events
        .iter()
        .find(|event| event.event_id == Some(move_event_id))
        .expect("move event present");
    assert_eq!(moved.event_type, EventType::DecisionMoved);
    assert_eq!(moved.actor_id, "actor:alice");
    assert_eq!(
        moved.payload.get("decision_id").and_then(|v| v.as_str()),
        Some(decision_id.as_str())
    );
    assert_eq!(
        moved.payload.get("from").and_then(|v| v.as_str()),
        Some("billing")
    );
    assert_eq!(
        moved.payload.get("to").and_then(|v| v.as_str()),
        Some("pricing")
    );
    assert_eq!(
        moved.payload.get("reason").and_then(|v| v.as_str()),
        Some("per-seat pricing decisions live under Pricing")
    );

    // Reversal: moving it back is just another recorded move, not a rewrite.
    commands
        .move_decision("actor:alice", &decision_id, "pricing", "billing", None)
        .expect("reversal succeeds");
    let events = ledger.read(0, 10).expect("read succeeds");
    let moves: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == EventType::DecisionMoved)
        .collect();
    assert_eq!(moves.len(), 2, "both moves are recorded, none rewritten");
}

#[test]
fn move_decision_rejects_stale_from() {
    // `from` must equal the decision's *current* project -- a caller naming a project the
    // decision already left is refused rather than silently moving it again.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing", "pricing", "platform"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");
    commands
        .move_decision("actor:alice", &decision_id, "billing", "pricing", None)
        .expect("first move succeeds");

    let error = commands
        .move_decision("actor:alice", &decision_id, "billing", "platform", None)
        .expect_err("stale from is refused");
    let message = error.to_string();
    assert!(message.contains("pricing"), "message was: {message}");

    let moves = ledger
        .read(0, 100)
        .expect("read succeeds")
        .into_iter()
        .filter(|event| event.event_type == EventType::DecisionMoved)
        .count();
    assert_eq!(moves, 1, "the rejected move must not append");
}

#[test]
fn move_decision_rejects_matching_from_and_to() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    assert!(commands
        .move_decision("actor:alice", &decision_id, "billing", "billing", None)
        .is_err());
}

#[test]
fn move_decision_rejects_unregistered_to() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let error = commands
        .move_decision("actor:alice", &decision_id, "billing", "pricing", None)
        .expect_err("unregistered to is refused");
    assert!(error.to_string().contains("pricing"));
}

#[test]
fn move_decision_rejects_someone_elses_personal_project() {
    // `to` may be the acting actor's own personal address only (Alex, choice 3a) -- a
    // decision can never be moved into someone else's personal project.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let bob_personal = personal_project_handle("actor:bob");
    assert!(commands
        .move_decision("actor:alice", &decision_id, "billing", &bob_personal, None)
        .is_err());
}

#[test]
fn move_decision_accepts_actors_own_personal_project() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let alice_personal = personal_project_handle("actor:alice");
    commands
        .move_decision(
            "actor:alice",
            &decision_id,
            "billing",
            &alice_personal,
            None,
        )
        .expect("moving into the actor's own personal project succeeds");
}

#[test]
fn move_decision_from_personal_fallback_resolves_derived_handle() {
    // A decision proposed with no stated project lands in the proposer's personal
    // fallback (approved record shape, item 2). `from` must be that derived handle, not
    // a typed one, even though it was never stored on the proposal event itself.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing", "pricing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal("actor:alice", "Judgement call", None))
        .expect("propose succeeds");
    let alice_personal = personal_project_handle("actor:alice");

    // "billing" reads like a plausible `from`, but the decision actually landed in the
    // derived personal fallback -- naming the wrong (if registered) project is refused
    // the same way any other stale `from` is.
    let error = commands
        .move_decision("actor:alice", &decision_id, "billing", "pricing", None)
        .expect_err("billing is not this decision's current project");
    assert!(error.to_string().contains(&alice_personal));
    commands
        .move_decision(
            "actor:alice",
            &decision_id,
            &alice_personal,
            "billing",
            None,
        )
        .expect("moving out of the derived personal fallback succeeds");
}

#[test]
fn move_decision_rejects_missing_decision() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands
        .register_project("actor:alice", "billing", None, None)
        .expect("register billing");
    commands
        .register_project("actor:alice", "pricing", None, None)
        .expect("register pricing");

    assert!(commands
        .move_decision(
            "actor:alice",
            "decision-does-not-exist",
            "billing",
            "pricing",
            None
        )
        .is_err());
}

#[test]
fn move_decision_to_reads_the_current_project_and_reports_both_ends() {
    // The caller names only where the decision goes; `from` is whatever the ledger resolves
    // now, so a second move needs no bookkeeping and reversal is just another `to`.
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing", "pricing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let first = commands
        .move_decision_to(
            "actor:alice",
            &decision_id,
            "pricing",
            Some("belongs there"),
        )
        .expect("move succeeds");
    assert_eq!(first.decision_id, decision_id);
    assert_eq!(first.from, "billing");
    assert_eq!(first.to, "pricing");
    assert_eq!(first.reason.as_deref(), Some("belongs there"));

    let back = commands
        .move_decision_to("actor:alice", &decision_id, "billing", None)
        .expect("reversal succeeds");
    assert_eq!(back.from, "pricing", "from follows the earlier move");
    assert_eq!(back.to, "billing");
    assert_eq!(back.reason, None);
    assert!(back.event_id > first.event_id);

    let moves = ledger
        .read(0, 20)
        .expect("read succeeds")
        .into_iter()
        .filter(|event| event.event_type == EventType::DecisionMoved)
        .count();
    assert_eq!(moves, 2, "both moves are recorded, none rewritten");
}

#[test]
fn move_decision_to_leaves_the_personal_fallback_without_naming_it() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal("actor:alice", "Judgement call", None))
        .expect("propose succeeds");

    let moved = commands
        .move_decision_to("actor:alice", &decision_id, "billing", None)
        .expect("move succeeds");
    assert_eq!(moved.from, personal_project_handle("actor:alice"));
}

#[test]
fn move_decision_to_says_when_the_decision_is_already_there() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let error = commands
        .move_decision_to("actor:alice", &decision_id, "billing", None)
        .expect_err("a move to the same project is refused");
    let message = error.to_string();
    assert!(
        message.contains("already in project billing"),
        "message was: {message}"
    );
    let moves = ledger
        .read(0, 20)
        .expect("read succeeds")
        .into_iter()
        .filter(|event| event.event_type == EventType::DecisionMoved)
        .count();
    assert_eq!(moves, 0, "the refused move must not append");
}

#[test]
fn move_decision_to_keeps_every_rule_of_move_decision() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let fixture = PlacementFixture::new(&commands, &["billing"]);
    let decision_id = commands
        .propose_decision(fixture.proposal(
            "actor:alice",
            "Per-seat pricing",
            Some(DeterminedProject::stated("billing")),
        ))
        .expect("propose succeeds");

    let unregistered = commands
        .move_decision_to("actor:alice", &decision_id, "pricing", None)
        .expect_err("an unregistered project is refused");
    assert!(unregistered.to_string().contains("project not registered"));

    let bob_personal = personal_project_handle("actor:bob");
    assert!(
        commands
            .move_decision_to("actor:alice", &decision_id, &bob_personal, None)
            .is_err(),
        "someone else's personal project is refused"
    );

    let missing = commands
        .move_decision_to("actor:alice", "decision-does-not-exist", "billing", None)
        .expect_err("a missing decision is refused");
    assert!(missing.to_string().contains("decision does not exist"));

    let own = personal_project_handle("actor:alice");
    commands
        .move_decision_to("actor:alice", &decision_id, &own, None)
        .expect("the actor's own personal project is allowed");
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
            project: None,
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
            delegated_by: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title,
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Only option".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
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
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            grounding: Grounding::Declared {
                premise_decision_ids: &[],
                evidence_ids: &[],
                hypothesis_ids: &[],
            },
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision with declared-but-empty grounding",
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
                project: None,
                grounding: Grounding::Declared {
                    premise_decision_ids: &premise_ids,
                    evidence_ids: &[],
                    hypothesis_ids: &[],
                },
                expressed_confidence: None,
                actor_id: "actor:alice",
                title: "A decision naming itself as a premise",
                rationale: "Rationale text long enough for the readable floor",
                topic_keys: &["topic".to_owned()],
                option_ids: std::slice::from_ref(&option_id),
                option_labels: &["A".to_owned()],
                chosen_option_id: None,
                decided_by: None,
                delegated_by: None,
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
            project: None,
            grounding: Grounding::Declared {
                premise_decision_ids: &premise_ids,
                evidence_ids: &[],
                hypothesis_ids: &[],
            },
            expressed_confidence: None,
            actor_id: "actor:alice",
            title: "Decision naming a premise that doesn't exist",
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
                project: None,
                grounding: Grounding::Declared {
                    premise_decision_ids: &premise_ids,
                    evidence_ids: &[],
                    hypothesis_ids: &[],
                },
                expressed_confidence: None,
                actor_id: "actor:alice",
                title: "Rests on a since-superseded decision",
                rationale: "Rationale text long enough for the readable floor",
                topic_keys: &["topic".to_owned()],
                option_ids: std::slice::from_ref(&option_id),
                option_labels: &["A".to_owned()],
                chosen_option_id: None,
                decided_by: None,
                delegated_by: None,
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
fn propose_decision_reports_only_rejected_or_superseded_premises_as_stale() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let live_premise = propose_minimal_decision(&commands, "Premise that stays live");
    let rejected_premise = propose_minimal_decision(&commands, "Premise that gets rejected");
    commands
        .disagree(
            "actor:bob",
            &rejected_premise,
            "Rejected so the test has a stale premise",
        )
        .expect("reject premise");

    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let premise_ids = vec![live_premise.clone(), rejected_premise.clone()];
    let result = commands
        .propose_decision_with_id(
            DecisionProposalInput {
                project: None,
                grounding: Grounding::Declared {
                    premise_decision_ids: &premise_ids,
                    evidence_ids: &[],
                    hypothesis_ids: &[],
                },
                expressed_confidence: None,
                actor_id: "actor:alice",
                title: "Rests on one live and one rejected decision",
                rationale: "Rationale text long enough for the readable floor",
                topic_keys: &["topic".to_owned()],
                option_ids: std::slice::from_ref(&option_id),
                option_labels: &["A".to_owned()],
                chosen_option_id: None,
                decided_by: None,
                delegated_by: None,
                still_proposed: true,
                hypothesis_ids: &[],
                evidence_ids: &[],
                quote: None,
                question: None,
            },
            "decision-with-mixed-premises",
            super::DecisionProposalEventUuids {
                proposal: Uuid::new_v4(),
                has_option: vec![Uuid::new_v4()],
                chose: None,
                assumes: Vec::new(),
                based_on: Vec::new(),
                follows_from: vec![Uuid::new_v4(), Uuid::new_v4()],
            },
        )
        .expect("both premises are linked; only the rejected one is reported stale");

    assert_eq!(result.premise_stale, vec![rejected_premise]);
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: Some("very high"),
            actor_id: "actor:alice",
            title: "Decision with an invalid confidence word",
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: Some("medium"),
            actor_id: "actor:alice",
            title: "Decision with a stated confidence",
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["A".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
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

fn recorded_hypothesis_payload(
    ledger: &InMemoryEventLedger,
    hypothesis_id: &str,
) -> serde_json::Value {
    ledger
        .read(0, 50)
        .expect("read events")
        .into_iter()
        .find(|event| {
            event.event_type == EventType::HypothesisRecorded
                && event.payload.get("hypothesis_id").and_then(|v| v.as_str())
                    == Some(hypothesis_id)
        })
        .expect("hypothesis event")
        .payload
}

#[test]
fn record_bet_without_statement_defaults_to_judgement_call_title() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let hypothesis_id = commands
        .record_bet(
            "actor:alice",
            None,
            "  Keep the embedded database for slice one  ",
            None,
            None,
        )
        .expect("a bare bet records the default statement");

    let payload = recorded_hypothesis_payload(&ledger, &hypothesis_id);
    assert_eq!(
        payload.get("statement").and_then(|v| v.as_str()),
        Some("Judgement call: Keep the embedded database for slice one")
    );
    assert_eq!(payload.get("kind").and_then(|v| v.as_str()), Some("bet"));
}

#[test]
fn record_bet_with_blank_statement_defaults_like_a_bare_bet() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let hypothesis_id = commands
        .record_bet(
            "actor:alice",
            Some("   "),
            "Adopt the new queue",
            None,
            None,
        )
        .expect("a blank statement is a bare bet");

    let payload = recorded_hypothesis_payload(&ledger, &hypothesis_id);
    assert_eq!(
        payload.get("statement").and_then(|v| v.as_str()),
        Some("Judgement call: Adopt the new queue")
    );
}

#[test]
fn record_bet_with_statement_keeps_it_verbatim_and_carries_bet_fields() {
    use chrono::TimeZone;

    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let check_by = chrono::Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();

    let hypothesis_id = commands
        .record_bet(
            "actor:alice",
            Some("Customers will not notice the migration"),
            "",
            Some(check_by),
            Some("Support tickets about the migration exceed ten"),
        )
        .expect("an explicit statement needs no decision title");

    let payload = recorded_hypothesis_payload(&ledger, &hypothesis_id);
    assert_eq!(
        payload.get("statement").and_then(|v| v.as_str()),
        Some("Customers will not notice the migration")
    );
    assert_eq!(payload.get("kind").and_then(|v| v.as_str()), Some("bet"));
    assert!(payload.get("check_by").and_then(|v| v.as_str()).is_some());
    assert_eq!(
        payload.get("would_change_if").and_then(|v| v.as_str()),
        Some("Support tickets about the migration exceed ten")
    );
}

#[test]
fn record_bet_without_statement_or_title_is_refused_and_writes_nothing() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let error = commands
        .record_bet("actor:alice", None, "   ", None, None)
        .expect_err("nothing to default from");
    assert!(
        error.to_string().contains("decision_title"),
        "unexpected error: {error}"
    );
    assert!(ledger.read(0, 10).expect("read events").is_empty());
}

#[test]
fn link_follows_from_rejects_nonexistent_decision_or_premise() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose_minimal_decision(&commands, "A decision that exists");

    let missing_premise = commands
        .link_follows_from(&decision_id, "decision-missing-premise", "actor:bob")
        .expect_err("a missing premise must be refused");
    assert!(
        missing_premise
            .to_string()
            .contains("decision does not exist: decision-missing-premise"),
        "unexpected error: {missing_premise}"
    );

    let missing_decision = commands
        .link_follows_from("decision-missing-source", &decision_id, "actor:bob")
        .expect_err("a missing source decision must be refused");
    assert!(
        missing_decision
            .to_string()
            .contains("decision does not exist: decision-missing-source"),
        "unexpected error: {missing_decision}"
    );
}

#[test]
fn ground_decision_rejects_nonexistent_targets_and_writes_nothing() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose_minimal_decision(&commands, "Decision to ground");
    let events_before = ledger.read(0, 100).expect("read events").len();

    let missing_decision = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: "decision-missing-target",
            premise_decision_ids: std::slice::from_ref(&decision_id),
            evidence_ids: &[],
            hypothesis_ids: &[],
        })
        .expect_err("a missing target decision must be refused");
    assert!(
        missing_decision
            .to_string()
            .contains("decision does not exist: decision-missing-target"),
        "unexpected error: {missing_decision}"
    );

    let missing_premise = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: &decision_id,
            premise_decision_ids: &["decision-missing-premise".to_owned()],
            evidence_ids: &[],
            hypothesis_ids: &[],
        })
        .expect_err("a missing premise must be refused");
    assert!(
        missing_premise
            .to_string()
            .contains("decision does not exist: decision-missing-premise"),
        "unexpected error: {missing_premise}"
    );

    let missing_evidence = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: &decision_id,
            premise_decision_ids: &[],
            evidence_ids: &["evidence-missing".to_owned()],
            hypothesis_ids: &[],
        })
        .expect_err("missing evidence must be refused");
    assert!(
        missing_evidence
            .to_string()
            .contains("evidence does not exist: evidence-missing"),
        "unexpected error: {missing_evidence}"
    );

    let missing_hypothesis = commands
        .ground_decision(GroundInput {
            actor_id: "actor:bob",
            decision_id: &decision_id,
            premise_decision_ids: &[],
            evidence_ids: &[],
            hypothesis_ids: &["hypothesis-missing".to_owned()],
        })
        .expect_err("a missing hypothesis must be refused");
    assert!(
        missing_hypothesis
            .to_string()
            .contains("hypothesis does not exist: hypothesis-missing"),
        "unexpected error: {missing_hypothesis}"
    );

    assert_eq!(
        ledger.read(0, 100).expect("read events").len(),
        events_before,
        "every refusal happens before any grounding event is appended"
    );
}

// ---------------------------------------------------------------------------
// Grounded capture (hivemind-gwhr.2): propose_grounded_decision and grounded supersede
// ---------------------------------------------------------------------------

/// A proposal input with no grounding of its own — what a capture verb hands to
/// `propose_grounded_decision` alongside its plan.
fn grounded_input<'a>(
    option_id: &'a String,
    option_labels: &'a [String],
    topic_keys: &'a [String],
    title: &'a str,
) -> DecisionProposalInput<'a> {
    DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: "actor:alice",
        title,
        rationale: "Rationale text long enough for the readable floor",
        topic_keys,
        option_ids: std::slice::from_ref(option_id),
        option_labels,
        chosen_option_id: None,
        decided_by: None,
        still_proposed: true,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
        delegated_by: None,
        project: None,
    }
}

fn relation_events(ledger: &InMemoryEventLedger) -> Vec<crate::events::Event> {
    ledger
        .read(0, 200)
        .expect("read events")
        .into_iter()
        .filter(|event| event.event_type == EventType::RelationAdded)
        .collect()
}

fn relation_str<'a>(event: &'a crate::events::Event, key: &str) -> &'a str {
    event
        .payload
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

#[test]
fn grounded_capture_records_every_kind_and_links_them_at_capture() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let premise_id = propose_minimal_decision(&commands, "Prior goal decision");
    let existing_evidence = commands
        .record_evidence("actor:alice", "an evidence item recorded earlier")
        .expect("evidence");
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let labels = ["A".to_owned()];
    let topics = ["topic".to_owned()];
    let check_by = chrono::DateTime::parse_from_rfc3339("2026-12-01T00:00:00Z")
        .expect("date")
        .with_timezone(&chrono::Utc);

    let proposal = commands
        .propose_grounded_decision(
            DecisionProposalInput {
                expressed_confidence: Some("high"),
                ..grounded_input(&option_id, &labels, &topics, "Adopt the new queue")
            },
            &GroundingPlan {
                premise_decision_ids: vec![premise_id.clone()],
                evidence_ids: vec![existing_evidence.clone()],
                hypothesis_ids: Vec::new(),
                new_evidence: vec![NewEvidence {
                    content: "p95 latency was 180ms in run 42".to_owned(),
                    source: Some("ci run 42".to_owned()),
                }],
                new_assumptions: vec!["traffic stays under 1k rps".to_owned()],
                bet: Some(NewBet {
                    statement: None,
                    would_change_if: Some("p95 rises above 300ms".to_owned()),
                    check_by: Some(check_by),
                }),
            },
        )
        .expect("grounded capture succeeds");

    let events = ledger.read(0, 200).expect("read events");
    let proposal_event = events
        .iter()
        .find(|event| {
            event.event_type == EventType::DecisionProposed
                && relation_str(event, "decision_id") == proposal.decision_id
        })
        .expect("proposal event");
    let proposal_event_id = proposal_event.event_id.expect("proposal event id");
    assert_eq!(
        proposal_event
            .payload
            .get("expressed_confidence")
            .and_then(|value| value.as_str()),
        Some("high")
    );

    // New evidence carries its source; the assumption and the bet differ only in kind.
    let evidence = events
        .iter()
        .find(|event| {
            event.event_type == EventType::EvidenceRecorded
                && relation_str(event, "content").contains("p95 latency")
        })
        .expect("new evidence recorded");
    assert_eq!(relation_str(evidence, "source"), "ci run 42");
    let bet = events
        .iter()
        .find(|event| {
            event.event_type == EventType::HypothesisRecorded
                && relation_str(event, "kind") == "bet"
        })
        .expect("bet recorded");
    assert_eq!(
        relation_str(bet, "statement"),
        "Judgement call: Adopt the new queue"
    );
    assert_eq!(
        relation_str(bet, "would_change_if"),
        "p95 rises above 300ms"
    );
    assert!(relation_str(bet, "check_by").starts_with("2026-12-01"));
    let assumption = events
        .iter()
        .find(|event| {
            event.event_type == EventType::HypothesisRecorded
                && relation_str(event, "statement") == "traffic stays under 1k rps"
        })
        .expect("assumption recorded");
    assert_ne!(relation_str(assumption, "kind"), "bet");

    // Every grounding edge is fanned out at capture: causation = the proposal event.
    let at_capture: Vec<_> = relation_events(&ledger)
        .into_iter()
        .filter(|event| relation_str(event, "relation") != "HAS_OPTION")
        .collect();
    for kind in ["FOLLOWS_FROM", "BASED_ON", "ASSUMES"] {
        assert!(
            at_capture
                .iter()
                .any(|event| relation_str(event, "relation") == kind),
            "missing {kind} edge"
        );
    }
    assert_eq!(at_capture.len(), 5, "1 premise + 2 evidence + 2 hypotheses");
    for event in &at_capture {
        assert_eq!(event.causation_event_id, Some(proposal_event_id));
        assert_eq!(relation_str(event, "from_id"), proposal.decision_id);
    }

    // The reply lists what was recorded: decisions, evidence, assumptions, then the bet.
    let kinds: Vec<_> = proposal.rests_on.iter().map(|item| item.kind).collect();
    assert_eq!(
        kinds,
        [
            RestsOnKind::Decision,
            RestsOnKind::Evidence,
            RestsOnKind::Evidence,
            RestsOnKind::Assumption,
            RestsOnKind::Bet,
        ]
    );
    assert_eq!(proposal.rests_on[0].id, premise_id);
    assert_eq!(proposal.rests_on[1].id, existing_evidence);
    assert_eq!(
        proposal.rests_on[2].label.as_deref(),
        Some("p95 latency was 180ms in run 42")
    );
    assert!(proposal.premise_stale.is_empty());
}

#[test]
fn grounded_capture_with_an_empty_plan_is_refused_and_writes_nothing() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let labels = ["A".to_owned()];
    let topics = ["topic".to_owned()];

    let error = commands
        .propose_grounded_decision(
            grounded_input(&option_id, &labels, &topics, "Nothing behind this"),
            &GroundingPlan::default(),
        )
        .expect_err("an empty plan must be refused");
    assert!(
        error
            .to_string()
            .contains(super::GROUNDING_REQUIRED_MESSAGE),
        "unexpected error: {error}"
    );
    assert_eq!(ledger.read(0, 20).expect("read events").len(), 0);
}

#[test]
fn grounded_capture_refusals_leave_no_orphan_node_behind() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let labels = ["A".to_owned()];
    let topics = ["topic".to_owned()];
    let plan = GroundingPlan {
        new_evidence: vec![NewEvidence {
            content: "an observation that must not be stranded".to_owned(),
            source: None,
        }],
        new_assumptions: vec!["an assumption that must not be stranded".to_owned()],
        ..GroundingPlan::default()
    };

    // A title the proposal itself refuses: the evidence and assumption above must not be written.
    let too_long_title = "t".repeat(MAX_TITLE_LEN + 1);
    commands
        .propose_grounded_decision(
            grounded_input(&option_id, &labels, &topics, &too_long_title),
            &plan,
        )
        .expect_err("an over-long title must be refused");
    // A rationale below the readable floor, likewise.
    commands
        .propose_grounded_decision(
            DecisionProposalInput {
                rationale: "1a",
                ..grounded_input(&option_id, &labels, &topics, "Fine title")
            },
            &plan,
        )
        .expect_err("an unreadable rationale must be refused");
    // A premise that does not exist, likewise.
    commands
        .propose_grounded_decision(
            grounded_input(&option_id, &labels, &topics, "Fine title"),
            &GroundingPlan {
                premise_decision_ids: vec!["decision-missing".to_owned()],
                ..plan.clone()
            },
        )
        .expect_err("a missing premise must be refused");
    // A blank new evidence item, likewise.
    commands
        .propose_grounded_decision(
            grounded_input(&option_id, &labels, &topics, "Fine title"),
            &GroundingPlan {
                new_evidence: vec![NewEvidence {
                    content: "   ".to_owned(),
                    source: None,
                }],
                ..plan.clone()
            },
        )
        .expect_err("blank evidence must be refused");

    assert_eq!(
        ledger.read(0, 50).expect("read events").len(),
        0,
        "no refusal may leave an evidence or hypothesis node behind"
    );
}

#[test]
fn grounded_capture_takes_its_grounding_only_from_the_plan() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let labels = ["A".to_owned()];
    let topics = ["topic".to_owned()];
    let plan = GroundingPlan {
        new_assumptions: vec!["something we assume".to_owned()],
        ..GroundingPlan::default()
    };

    let already_declared = commands
        .propose_grounded_decision(
            DecisionProposalInput {
                grounding: Grounding::Declared {
                    premise_decision_ids: &[],
                    evidence_ids: &[],
                    hypothesis_ids: &[],
                },
                ..grounded_input(&option_id, &labels, &topics, "Fine title")
            },
            &plan,
        )
        .expect_err("input must not carry its own grounding");
    assert!(already_declared
        .to_string()
        .contains("takes its grounding from the plan"));
    let legacy_ids = commands
        .propose_grounded_decision(
            DecisionProposalInput {
                evidence_ids: &["evidence-x".to_owned()],
                ..grounded_input(&option_id, &labels, &topics, "Fine title")
            },
            &plan,
        )
        .expect_err("input must not carry its own evidence ids");
    assert!(legacy_ids
        .to_string()
        .contains("takes its grounding from the plan"));
    assert_eq!(ledger.read(0, 20).expect("read events").len(), 0);
}

#[test]
fn grounded_capture_records_a_stale_premise_and_reports_it() {
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
    let labels = ["A".to_owned()];
    let topics = ["topic".to_owned()];

    let proposal = commands
        .propose_grounded_decision(
            grounded_input(&option_id, &labels, &topics, "Rests on a stale premise"),
            &GroundingPlan {
                premise_decision_ids: vec![premise_id.clone(), successor_id.clone()],
                ..GroundingPlan::default()
            },
        )
        .expect("a stale premise is allowed");
    assert_eq!(proposal.premise_stale, vec![premise_id.clone()]);
    assert_eq!(
        relation_events(&ledger)
            .iter()
            .filter(|event| relation_str(event, "relation") == "FOLLOWS_FROM")
            .count(),
        2,
        "the stale premise is still linked: honesty about it is the reader's job"
    );
}

fn grounded_supersede_input<'a>(
    old_decision_id: &'a str,
    plan: &'a GroundingPlan,
) -> SupersedeInput<'a> {
    SupersedeInput {
        actor_id: "actor:alice",
        old_decision_id,
        new_title: "Decision B",
        new_rationale: "New rationale that explains the replacement decision.",
        topic_keys: &[],
        option_labels: &[],
        chosen_option_label: None,
        hypothesis_ids: &[],
        evidence_ids: &[],
        grounding: Some(plan),
        expressed_confidence: None,
        project: None,
    }
}

#[test]
fn grounded_supersede_records_its_grounding_and_stays_idempotent() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old_decision_id = propose_minimal_decision(&commands, "Decision A");
    let premise_id = propose_minimal_decision(&commands, "Goal decision");
    let plan = GroundingPlan {
        premise_decision_ids: vec![premise_id.clone()],
        new_evidence: vec![NewEvidence {
            content: "the old approach failed twice".to_owned(),
            source: Some("incident 7".to_owned()),
        }],
        bet: Some(NewBet::default()),
        ..GroundingPlan::default()
    };

    let first = commands
        .supersede(grounded_supersede_input(&old_decision_id, &plan))
        .expect("grounded supersede succeeds");
    assert_eq!(
        first
            .rests_on
            .iter()
            .map(|item| item.kind)
            .collect::<Vec<_>>(),
        [
            RestsOnKind::Decision,
            RestsOnKind::Evidence,
            RestsOnKind::Bet
        ]
    );
    let relations = relation_events(&ledger);
    for kind in ["FOLLOWS_FROM", "BASED_ON", "ASSUMES"] {
        assert!(
            relations
                .iter()
                .any(|event| relation_str(event, "relation") == kind
                    && !relation_str(event, "from_id").is_empty()),
            "missing {kind} edge on the replacement"
        );
    }

    // An identical retry finds the same nodes (deterministic ids) and the same supersession.
    let latest_after_first = ledger.latest_offset().expect("latest offset");
    let retry = commands
        .supersede(grounded_supersede_input(&old_decision_id, &plan))
        .expect("retry succeeds");
    assert_eq!(retry.new_decision_id, first.new_decision_id);
    assert_eq!(retry.superseded_event_id, first.superseded_event_id);
    assert_eq!(retry.rests_on, first.rests_on);
    assert_eq!(
        ledger.latest_offset().expect("latest offset unchanged"),
        latest_after_first,
        "an identical grounded retry appends nothing"
    );

    // A retry that names a different premise is a new supersession, not a silent match.
    let other_premise_id = propose_minimal_decision(&commands, "A different goal");
    let different = commands
        .supersede(grounded_supersede_input(
            &old_decision_id,
            &GroundingPlan {
                premise_decision_ids: vec![other_premise_id],
                ..plan.clone()
            },
        ))
        .expect("different grounding supersedes again");
    assert_ne!(different.new_decision_id, first.new_decision_id);
}

#[test]
fn grounded_supersede_refuses_an_empty_plan_and_legacy_ids_and_writes_nothing() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old_decision_id = propose_minimal_decision(&commands, "Decision A");
    let events_before = ledger.read(0, 50).expect("read events").len();

    let empty = GroundingPlan::default();
    let error = commands
        .supersede(grounded_supersede_input(&old_decision_id, &empty))
        .expect_err("an empty plan must be refused");
    assert!(error
        .to_string()
        .contains(super::GROUNDING_REQUIRED_MESSAGE));

    let plan = GroundingPlan {
        new_assumptions: vec!["we assume the replacement is cheaper".to_owned()],
        ..GroundingPlan::default()
    };
    let error = commands
        .supersede(SupersedeInput {
            evidence_ids: &["evidence-x".to_owned()],
            ..grounded_supersede_input(&old_decision_id, &plan)
        })
        .expect_err("legacy ids alongside a grounding must be refused");
    assert!(error
        .to_string()
        .contains("takes hypothesis and evidence ids"));

    // A rationale below the readable floor is refused after the plan is planned but before
    // any node is recorded.
    let error = commands
        .supersede(SupersedeInput {
            new_rationale: "1a",
            ..grounded_supersede_input(&old_decision_id, &plan)
        })
        .expect_err("an unreadable rationale must be refused");
    assert!(!error.to_string().is_empty());

    assert_eq!(
        ledger.read(0, 50).expect("read events").len(),
        events_before,
        "no refusal may leave a grounding node behind"
    );
}

#[test]
fn ungrounded_supersede_is_unchanged_and_reports_no_grounding() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old_decision_id = propose_minimal_decision(&commands, "Decision A");

    let outcome = commands
        .supersede(SupersedeInput {
            grounding: None,
            ..grounded_supersede_input(&old_decision_id, &GroundingPlan::default())
        })
        .expect("ungrounded supersede still works for the review path");
    assert!(outcome.rests_on.is_empty());
    assert!(outcome.premise_stale.is_empty());
    assert!(relation_events(&ledger)
        .iter()
        .all(|event| relation_str(event, "relation") != "FOLLOWS_FROM"));
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

// ---------------------------------------------------------------------------
// Project topic vocabulary (hivemind-zywz)
// ---------------------------------------------------------------------------

/// One capture by `actor:alice` filed under `project` (`None` is the personal fallback) with
/// `topic_keys`, declaring `declare` on the way, through a fresh `Commands` like one CLI or MCP
/// call.
fn capture_with_topics(
    ledger: &InMemoryEventLedger,
    project: Option<&str>,
    topic_keys: &[&str],
    declare: &[&str],
) -> crate::Result<(String, DecisionPlacement)> {
    let owned = |keys: &[&str]| keys.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>();
    let topic_keys = owned(topic_keys);
    let declare = owned(declare);
    let commands = Commands::new(ledger).declaring_topics(&declare);
    let option_id = commands.record_option("actor:alice", "A", "Option A")?;
    commands.propose_decision_placed(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: "actor:alice",
        title: "Filed with topics",
        rationale: "The vocabulary decides which topic keys this capture may carry",
        topic_keys: &topic_keys,
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &["Option A".to_owned()],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: None,
        delegated_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
        project: project.map(DeterminedProject::stated),
    })
}

fn events_of(ledger: &InMemoryEventLedger, event_type: EventType) -> Vec<crate::events::Event> {
    ledger
        .read(0, 500)
        .expect("read events")
        .into_iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

fn ledger_with_project(handle: &str) -> InMemoryEventLedger {
    let ledger = InMemoryEventLedger::new();
    Commands::new(&ledger)
        .register_project("actor:alice", handle, None, None)
        .expect("register succeeds");
    ledger
}

#[test]
fn a_capture_into_a_project_may_only_use_declared_topics() {
    let ledger = ledger_with_project("billing");

    let error = capture_with_topics(&ledger, Some("billing"), &["pricing"], &[])
        .expect_err("an undeclared topic must be refused");
    let message = error.to_string();
    assert!(message.contains("`pricing` is not declared"), "{message}");
    assert!(message.contains("project billing"), "{message}");
    assert!(message.contains("declared: none yet"), "{message}");
    assert!(
        message.contains("--declare-topic pricing"),
        "the refusal must say how to declare: {message}"
    );
    assert!(
        message.contains("hivemind project declare-topic billing pricing"),
        "the refusal must name the standalone verb: {message}"
    );
    assert!(
        events_of(&ledger, EventType::DecisionProposed).is_empty()
            && events_of(&ledger, EventType::ProjectTopicDeclared).is_empty(),
        "a refused capture writes nothing"
    );
}

#[test]
fn the_refusal_lists_what_the_project_has_declared() {
    let ledger = ledger_with_project("billing");
    let commands = Commands::new(&ledger);
    for topic_key in ["invoicing", "pricing"] {
        commands
            .declare_project_topic("actor:alice", "billing", topic_key)
            .expect("declare succeeds");
    }

    let message = capture_with_topics(&ledger, Some("billing"), &["pricing", "taxes", "fees"], &[])
        .expect_err("undeclared topics must be refused")
        .to_string();
    assert!(
        message.contains("topics `fees`, `taxes` are not declared"),
        "every undeclared key is named, once each, in key order: {message}"
    );
    assert!(
        message.contains("declared: invoicing, pricing"),
        "{message}"
    );
}

#[test]
fn a_capture_declares_the_new_topics_it_says_so_about_and_the_reply_lists_them() {
    let ledger = ledger_with_project("billing");
    Commands::new(&ledger)
        .declare_project_topic("actor:alice", "billing", "auth")
        .expect("declare succeeds");

    let (_, placement) = capture_with_topics(
        &ledger,
        Some("billing"),
        &["auth", "Pricing Model"],
        &["pricing model"],
    )
    .expect("a capture may declare the key it introduces");

    assert_eq!(placement.declared_topics, vec!["pricing-model".to_owned()]);
    let events = ledger.read(0, 100).expect("read events");
    let declared_at = events
        .iter()
        .rposition(|event| event.event_type == EventType::ProjectTopicDeclared)
        .expect("declaration recorded");
    let proposed_at = events
        .iter()
        .position(|event| event.event_type == EventType::DecisionProposed)
        .expect("proposal recorded");
    assert!(
        declared_at < proposed_at,
        "declared just before the proposal"
    );
    let declaration = &events[declared_at];
    assert_eq!(declaration.actor_id, "actor:alice");
    assert_eq!(
        declaration.payload.get("handle").and_then(|v| v.as_str()),
        Some("billing")
    );
    assert_eq!(
        declaration
            .payload
            .get("topic_key")
            .and_then(|v| v.as_str()),
        Some("pricing-model"),
        "the vocabulary holds the normalised key"
    );

    // Declared once, used freely: the next capture needs no declaration and records none.
    let (_, second) = capture_with_topics(&ledger, Some("billing"), &["pricing-model"], &[])
        .expect("a declared topic needs no declaration");
    assert!(second.declared_topics.is_empty());
    assert_eq!(events_of(&ledger, EventType::ProjectTopicDeclared).len(), 2);
}

#[test]
fn a_capture_that_still_names_an_undeclared_topic_declares_nothing() {
    let ledger = ledger_with_project("billing");

    capture_with_topics(
        &ledger,
        Some("billing"),
        &["pricing", "taxes"],
        &["pricing"],
    )
    .expect_err("`taxes` is still undeclared");

    assert!(
        events_of(&ledger, EventType::ProjectTopicDeclared).is_empty(),
        "a refused capture must not leave half its declarations behind"
    );
}

#[test]
fn a_capture_may_only_declare_topics_it_uses() {
    let ledger = ledger_with_project("billing");

    let message = capture_with_topics(
        &ledger,
        Some("billing"),
        &["pricing"],
        &["pricing", "taxes"],
    )
    .expect_err("`taxes` is not one of this capture's keys")
    .to_string();
    assert!(message.contains("declared topic taxes"), "{message}");
    assert!(events_of(&ledger, EventType::ProjectTopicDeclared).is_empty());
}

#[test]
fn a_personal_project_has_no_vocabulary() {
    let ledger = InMemoryEventLedger::new();

    let (_, placement) = capture_with_topics(&ledger, None, &["anything-goes"], &[])
        .expect("a capture with no project accepts any topic");
    assert_eq!(placement.project_source, ProjectSource::PersonalFallback);

    let message = capture_with_topics(&ledger, None, &["anything-goes"], &["anything-goes"])
        .expect_err("declaring needs a registered project")
        .to_string();
    assert!(
        message.contains("declared topics need a registered project"),
        "{message}"
    );
}

#[test]
fn declare_project_topic_normalises_repeats_quietly_and_refuses_bad_targets() {
    let ledger = ledger_with_project("billing");
    let commands = Commands::new(&ledger);

    let first = commands
        .declare_project_topic("actor:alice", "billing", "Pricing Model")
        .expect("declare succeeds");
    assert_eq!(first.topic_key, "pricing-model");
    assert!(first.newly_declared && first.event_id.is_some());

    let again = commands
        .declare_project_topic("actor:bob", "billing", "pricing_model")
        .expect("declaring a key the project has succeeds");
    assert!(!again.newly_declared && again.event_id.is_none());
    assert_eq!(
        events_of(&ledger, EventType::ProjectTopicDeclared).len(),
        1,
        "a key already declared is not recorded twice"
    );

    let personal = commands
        .declare_project_topic("actor:alice", "personal:human:alex", "pricing")
        .expect_err("a personal project has no vocabulary")
        .to_string();
    assert!(
        personal.contains("personal project has no topic vocabulary"),
        "{personal}"
    );

    let unregistered = commands
        .declare_project_topic("actor:alice", "nosuch", "pricing")
        .expect_err("an unregistered project has no vocabulary")
        .to_string();
    assert!(
        unregistered.contains("hivemind project register nosuch"),
        "{unregistered}"
    );

    let empty = commands
        .declare_project_topic("actor:alice", "billing", "!!!")
        .expect_err("a key of no letters or digits is refused")
        .to_string();
    assert!(empty.contains("at least one letter or digit"), "{empty}");
}

#[test]
fn a_supersede_answers_to_the_vocabulary_of_the_project_it_inherits() {
    let ledger = ledger_with_project("billing");
    let (old_decision_id, _) =
        capture_with_topics(&ledger, Some("billing"), &["pricing"], &["pricing"])
            .expect("the first capture declares its key");
    let supersede = |commands: &Commands<'_, InMemoryEventLedger>, topic_keys: &[String]| {
        commands.supersede(SupersedeInput {
            actor_id: "actor:alice",
            old_decision_id: &old_decision_id,
            new_title: "Price per seat",
            new_rationale: "Per-seat pricing replaces the flat fee because usage grew",
            topic_keys,
            option_labels: &["Per seat".to_owned()],
            chosen_option_label: Some("Per seat"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project: None,
            grounding: None,
            expressed_confidence: None,
        })
    };

    let refused = supersede(&Commands::new(&ledger), &["seats".to_owned()])
        .expect_err("`seats` is not declared for the inherited project")
        .to_string();
    assert!(
        refused.contains("`seats` is not declared for project billing"),
        "{refused}"
    );

    let declared = ["seats".to_owned()];
    let outcome = supersede(
        &Commands::new(&ledger).declaring_topics(&declared),
        &["seats".to_owned()],
    )
    .expect("a supersede may declare the key it introduces");
    assert_eq!(outcome.placement.declared_topics, vec!["seats".to_owned()]);
}

#[test]
fn a_grounded_capture_refused_for_its_topics_leaves_no_orphan_nodes() {
    let ledger = ledger_with_project("billing");
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option("actor:alice", "A", "Option A")
        .expect("option a");
    let labels = ["A".to_owned()];
    let topics = ["pricing".to_owned()];

    commands
        .propose_grounded_decision(
            DecisionProposalInput {
                project: Some(DeterminedProject::stated("billing")),
                ..grounded_input(&option_id, &labels, &topics, "Price per seat")
            },
            &GroundingPlan {
                premise_decision_ids: Vec::new(),
                evidence_ids: Vec::new(),
                hypothesis_ids: Vec::new(),
                new_evidence: vec![NewEvidence {
                    content: "Usage doubled in Q3".to_owned(),
                    source: None,
                }],
                new_assumptions: vec!["Usage keeps growing".to_owned()],
                bet: None,
            },
        )
        .expect_err("`pricing` is not declared for billing");

    assert!(
        events_of(&ledger, EventType::EvidenceRecorded).is_empty()
            && events_of(&ledger, EventType::HypothesisRecorded).is_empty(),
        "the vocabulary is checked before the grounding nodes are recorded"
    );
}

#[test]
fn declare_topics_in_use_adopts_what_the_decisions_now_in_the_project_carry() {
    let ledger = ledger_with_project("billing");
    Commands::new(&ledger)
        .register_project("actor:alice", "platform", None, None)
        .expect("register succeeds");
    let (staying, _) = capture_with_topics(&ledger, None, &["pricing", "invoicing"], &[])
        .expect("personal captures have no vocabulary");
    let (leaving, _) = capture_with_topics(&ledger, None, &["legacy"], &[])
        .expect("personal captures have no vocabulary");

    // A move never checks the destination's vocabulary: correcting where a decision lives
    // must not be refused for the keys it was captured with.
    let commands = Commands::new(&ledger);
    commands
        .move_decision_to("actor:alice", &staying, "billing", None)
        .expect("a move is not refused for undeclared topics");
    commands
        .move_decision_to("actor:alice", &leaving, "billing", None)
        .expect("moved in");
    commands
        .move_decision_to("actor:alice", &leaving, "platform", None)
        .expect("and moved out again");

    let declared = commands
        .declare_topics_in_use("actor:bob", "billing")
        .expect("adoption succeeds");
    let keys: Vec<&str> = declared.iter().map(|d| d.topic_key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["invoicing", "pricing"],
        "the keys of the decisions now in the project, in key order; `legacy` left with its decision"
    );
    assert!(declared
        .iter()
        .all(|declaration| declaration.newly_declared));
    let recorded = events_of(&ledger, EventType::ProjectTopicDeclared);
    assert!(
        recorded.iter().all(|event| event.actor_id == "actor:bob"),
        "adopted by the actor who ran it"
    );

    assert!(
        commands
            .declare_topics_in_use("actor:bob", "billing")
            .expect("second adoption succeeds")
            .is_empty(),
        "nothing left to adopt"
    );
    capture_with_topics(&ledger, Some("billing"), &["pricing"], &[])
        .expect("an adopted topic is usable");
}
