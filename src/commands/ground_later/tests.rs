// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};

use crate::commands::{
    Commands, DecisionProposalInput, Grounding, GroundingPlan, NewBet, NewEvidence, RestsOnKind,
};
use crate::events::{Event, EventType};
use crate::ledger::{EventLedger, InMemoryEventLedger};

const PROPOSER: &str = "actor:alice";
const GROUNDER: &str = "actor:bob";

fn propose(commands: &Commands<'_, InMemoryEventLedger>, title: &str) -> String {
    let option_id = commands
        .record_option(PROPOSER, "Only option", "The only option")
        .expect("record option");
    commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: PROPOSER,
            title,
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Only option".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            delegated_by: None,
            project: None,
        })
        .expect("propose decision")
}

fn events(ledger: &InMemoryEventLedger) -> Vec<Event> {
    ledger.read(0, 500).expect("read events")
}

fn payload_str<'a>(event: &'a Event, key: &str) -> &'a str {
    event
        .payload
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

/// The events appended after the first `already` ones.
fn appended_after(ledger: &InMemoryEventLedger, already: usize) -> Vec<Event> {
    events(ledger).into_iter().skip(already).collect()
}

fn relations(events: &[Event]) -> Vec<(&str, &str, &str)> {
    events
        .iter()
        .filter(|event| event.event_type == EventType::RelationAdded)
        .map(|event| {
            (
                payload_str(event, "relation"),
                payload_str(event, "from_id"),
                payload_str(event, "to_id"),
            )
        })
        .collect()
}

#[test]
fn grounds_an_existing_decision_on_every_kind_attributed_to_the_grounder() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let premise_id = propose(&commands, "Prior goal decision");
    let decision_id = propose(&commands, "Decision that was captured without saying why");
    let existing_evidence = commands
        .record_evidence(PROPOSER, "an evidence item recorded earlier")
        .expect("evidence");
    let check_by: DateTime<Utc> = "2026-12-01T00:00:00Z".parse().expect("date");
    let before = events(&ledger).len();

    let added = commands
        .ground_decision_with_plan(
            GROUNDER,
            &decision_id,
            &GroundingPlan {
                premise_decision_ids: vec![premise_id.clone()],
                evidence_ids: vec![existing_evidence.clone()],
                hypothesis_ids: Vec::new(),
                new_evidence: vec![NewEvidence {
                    content: "the queue dropped 3 messages in the load test".to_owned(),
                    source: Some("ci run 41".to_owned()),
                }],
                new_assumptions: vec!["traffic stays under 1k rps".to_owned()],
                bet: Some(NewBet {
                    statement: Some("the queue is fast enough".to_owned()),
                    would_change_if: Some("p99 latency passes 200ms".to_owned()),
                    check_by: Some(check_by),
                }),
            },
        )
        .expect("ground decision");

    let kinds: Vec<RestsOnKind> = added.rests_on.iter().map(|item| item.kind).collect();
    assert_eq!(
        kinds,
        vec![
            RestsOnKind::Decision,
            RestsOnKind::Evidence,
            RestsOnKind::Evidence,
            RestsOnKind::Assumption,
            RestsOnKind::Bet,
        ]
    );
    assert_eq!(added.decision_id, decision_id);
    assert_eq!(added.relation_event_ids.len(), 5);
    assert!(added.premise_stale.is_empty());

    let appended = appended_after(&ledger, before);
    assert!(
        appended.iter().all(|event| event.actor_id == GROUNDER),
        "every node and edge names the grounder, not the proposer"
    );
    assert!(
        appended
            .iter()
            .all(|event| event.causation_event_id.is_none()),
        "later grounding never carries causation to the proposal"
    );

    let evidence = appended
        .iter()
        .find(|event| event.event_type == EventType::EvidenceRecorded)
        .expect("new evidence is recorded");
    assert_eq!(payload_str(evidence, "source"), "ci run 41");
    let hypotheses: Vec<&Event> = appended
        .iter()
        .filter(|event| event.event_type == EventType::HypothesisRecorded)
        .collect();
    assert_eq!(hypotheses.len(), 2);
    let bet = hypotheses
        .iter()
        .find(|event| payload_str(event, "kind") == "bet")
        .expect("the bet is a hypothesis of kind bet");
    assert_eq!(
        payload_str(bet, "would_change_if"),
        "p99 latency passes 200ms"
    );
    assert!(payload_str(bet, "check_by").starts_with("2026-12-01"));

    let edges = relations(&appended);
    assert_eq!(edges.len(), 5);
    assert!(edges.contains(&("FOLLOWS_FROM", decision_id.as_str(), premise_id.as_str())));
    assert!(edges.contains(&("BASED_ON", decision_id.as_str(), existing_evidence.as_str())));
    assert_eq!(
        edges
            .iter()
            .filter(|(relation, from, _)| *relation == "BASED_ON" && *from == decision_id)
            .count(),
        2
    );
    assert_eq!(
        edges
            .iter()
            .filter(|(relation, _, _)| *relation == "ASSUMES")
            .count(),
        2
    );
}

#[test]
fn a_bet_without_a_statement_records_the_judgement_call_of_the_grounded_decision() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose(&commands, "Adopt the shared queue");
    let before = events(&ledger).len();

    let added = commands
        .ground_decision_with_plan(
            GROUNDER,
            &decision_id,
            &GroundingPlan {
                bet: Some(NewBet::default()),
                ..GroundingPlan::default()
            },
        )
        .expect("ground decision");

    assert_eq!(added.rests_on.len(), 1);
    assert_eq!(
        added
            .rests_on
            .first()
            .and_then(|item| item.label.as_deref()),
        Some("Judgement call: Adopt the shared queue")
    );
    let appended = appended_after(&ledger, before);
    let bet = appended
        .iter()
        .find(|event| event.event_type == EventType::HypothesisRecorded)
        .expect("the bet is recorded");
    assert_eq!(
        payload_str(bet, "statement"),
        "Judgement call: Adopt the shared queue"
    );
}

#[test]
fn a_superseded_or_rejected_premise_is_recorded_and_reported_stale() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let rejected_premise = propose(&commands, "A premise we rejected");
    let live_premise = propose(&commands, "A premise that still stands");
    let decision_id = propose(&commands, "Decision to ground");
    commands
        .reject_decision(&rejected_premise, PROPOSER)
        .expect("reject premise");

    let added = commands
        .ground_decision_with_plan(
            GROUNDER,
            &decision_id,
            &GroundingPlan {
                premise_decision_ids: vec![rejected_premise.clone(), live_premise.clone()],
                ..GroundingPlan::default()
            },
        )
        .expect("a stale premise is allowed: the ledger is append-only");

    assert_eq!(added.premise_stale, vec![rejected_premise]);
    assert_eq!(added.relation_event_ids.len(), 2);
}

/// Run `plan` against `decision_id` and assert it is refused with `expected` in the message and
/// that not one event was appended — no orphan evidence, assumption or bet either.
fn assert_refused_writing_nothing(
    ledger: &InMemoryEventLedger,
    commands: &Commands<'_, InMemoryEventLedger>,
    decision_id: &str,
    plan: &GroundingPlan,
    expected: &str,
) {
    let before = events(ledger).len();
    let error = commands
        .ground_decision_with_plan(GROUNDER, decision_id, plan)
        .expect_err("the call must be refused");
    assert!(
        error.to_string().contains(expected),
        "expected `{expected}` in: {error}"
    );
    assert_eq!(
        events(ledger).len(),
        before,
        "a refused ground call writes nothing"
    );
}

fn plan_with_new_nodes() -> GroundingPlan {
    GroundingPlan {
        new_evidence: vec![NewEvidence {
            content: "something observed".to_owned(),
            source: Some("a source".to_owned()),
        }],
        new_assumptions: vec!["something assumed".to_owned()],
        bet: Some(NewBet::default()),
        ..GroundingPlan::default()
    }
}

#[test]
fn an_empty_plan_is_refused() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose(&commands, "Decision to ground");

    assert_refused_writing_nothing(
        &ledger,
        &commands,
        &decision_id,
        &GroundingPlan::default(),
        "grounding must name at least one premise decision, evidence item or hypothesis",
    );
}

#[test]
fn an_unknown_decision_is_refused_even_when_the_plan_would_create_nodes() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    assert_refused_writing_nothing(
        &ledger,
        &commands,
        "decision-does-not-exist",
        &plan_with_new_nodes(),
        "decision does not exist: decision-does-not-exist",
    );
}

#[test]
fn a_self_premise_is_refused_before_any_node_is_recorded() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose(&commands, "Decision to ground");

    assert_refused_writing_nothing(
        &ledger,
        &commands,
        &decision_id,
        &GroundingPlan {
            premise_decision_ids: vec![decision_id.clone()],
            ..plan_with_new_nodes()
        },
        "a decision cannot be its own premise",
    );
}

#[test]
fn a_premise_or_existing_node_that_does_not_exist_is_refused_before_any_node_is_recorded() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose(&commands, "Decision to ground");

    assert_refused_writing_nothing(
        &ledger,
        &commands,
        &decision_id,
        &GroundingPlan {
            premise_decision_ids: vec!["decision-missing".to_owned()],
            ..plan_with_new_nodes()
        },
        "decision does not exist: decision-missing",
    );
    assert_refused_writing_nothing(
        &ledger,
        &commands,
        &decision_id,
        &GroundingPlan {
            evidence_ids: vec!["evidence-missing".to_owned()],
            ..plan_with_new_nodes()
        },
        "evidence does not exist: evidence-missing",
    );
    assert_refused_writing_nothing(
        &ledger,
        &commands,
        &decision_id,
        &GroundingPlan {
            hypothesis_ids: vec!["hypothesis-missing".to_owned()],
            ..plan_with_new_nodes()
        },
        "hypothesis does not exist: hypothesis-missing",
    );
}

#[test]
fn a_malformed_new_node_is_refused_before_any_other_node_is_recorded() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = propose(&commands, "Decision to ground");

    assert_refused_writing_nothing(
        &ledger,
        &commands,
        &decision_id,
        &GroundingPlan {
            new_evidence: vec![NewEvidence {
                content: "a fine observation".to_owned(),
                source: None,
            }],
            new_assumptions: vec!["   ".to_owned()],
            ..GroundingPlan::default()
        },
        "rests-on assumption statement",
    );
}
