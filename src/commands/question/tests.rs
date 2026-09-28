// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use uuid::Uuid;

use crate::commands::{
    Commands, DecisionProposalEventUuids, DecisionProposalInput, Grounding, GroundingPlan,
    GROUNDING_REQUIRED_MESSAGE,
};
use crate::events::{Event, EventPayload, EventType, QuestionRecordedPayload};
use crate::ledger::{EventLedger, InMemoryEventLedger};

const PROPOSER: &str = "actor:alice";
const GROUNDER: &str = "actor:bob";
const QUESTION: &str = "Which storage engine should the prototype use?";

fn proposal<'a>(
    option_id: &'a String,
    title: &'a str,
    quote: Option<&'a str>,
    question: Option<&'a str>,
) -> DecisionProposalInput<'a> {
    DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: PROPOSER,
        title,
        rationale: "Rationale text long enough for the readable floor",
        topic_keys: &[],
        option_ids: std::slice::from_ref(option_id),
        option_labels: &[],
        chosen_option_id: None,
        decided_by: None,
        still_proposed: true,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote,
        question,
        delegated_by: None,
        project: None,
    }
}

/// A capture with the topic and option label every proposal here shares.
fn capture(
    commands: &Commands<'_, InMemoryEventLedger>,
    title: &str,
    question: Option<&str>,
) -> String {
    let option_id = commands
        .record_option(PROPOSER, "Only option", "The only option")
        .expect("record option");
    let topics = ["storage".to_owned()];
    let labels = ["Only option".to_owned()];
    commands
        .propose_decision(DecisionProposalInput {
            topic_keys: &topics,
            option_labels: &labels,
            ..proposal(&option_id, title, None, question)
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

fn of_type(events: &[Event], event_type: EventType) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

/// (from, to) of every `ANSWERS` relation, in ledger order.
fn answers_links(events: &[Event]) -> Vec<(&str, &str)> {
    of_type(events, EventType::RelationAdded)
        .into_iter()
        .filter(|event| payload_str(event, "relation") == "ANSWERS")
        .map(|event| (payload_str(event, "from_id"), payload_str(event, "to_id")))
        .collect()
}

#[test]
fn a_capture_that_names_a_question_records_the_node_and_links_it_at_capture() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let decision_id = capture(&commands, "Use SQLite for the prototype", Some(QUESTION));

    let all = events(&ledger);
    let recorded = of_type(&all, EventType::QuestionRecorded);
    assert_eq!(recorded.len(), 1);
    assert_eq!(payload_str(recorded[0], "text"), QUESTION);
    let question_id = payload_str(recorded[0], "question_id").to_owned();
    assert!(question_id.starts_with("question-"), "{question_id}");
    assert_eq!(
        answers_links(&all),
        vec![(decision_id.as_str(), question_id.as_str())]
    );

    let proposal = of_type(&all, EventType::DecisionProposed)[0];
    assert_eq!(
        payload_str(proposal, "question"),
        QUESTION,
        "the text stays on the decision exactly as before"
    );
    let link = of_type(&all, EventType::RelationAdded)
        .into_iter()
        .find(|event| payload_str(event, "relation") == "ANSWERS")
        .expect("ANSWERS link");
    assert_eq!(recorded[0].causation_event_id, proposal.event_id);
    assert_eq!(
        link.causation_event_id, proposal.event_id,
        "both events are caused by the proposal, which is what marks the link as at capture"
    );
    assert_eq!(link.actor_id, PROPOSER);
}

#[test]
fn two_captures_with_the_same_question_share_one_node() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let first = capture(&commands, "Use SQLite for the prototype", Some(QUESTION));
    let second = capture(
        &commands,
        "Use Postgres for the prototype",
        Some("  which storage engine   should the PROTOTYPE use "),
    );

    let all = events(&ledger);
    let recorded = of_type(&all, EventType::QuestionRecorded);
    assert_eq!(recorded.len(), 1, "the second capture reuses the node");
    let question_id = payload_str(recorded[0], "question_id");
    assert_eq!(
        answers_links(&all),
        vec![
            (first.as_str(), question_id),
            (second.as_str(), question_id)
        ]
    );
}

#[test]
fn a_reused_question_is_reported_as_reused() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option(PROPOSER, "Only option", "The only option")
        .expect("option");
    let topics = ["storage".to_owned()];
    let labels = ["Only option".to_owned()];
    let propose = |title: &str| {
        commands
            .propose_decision_detailed(DecisionProposalInput {
                topic_keys: &topics,
                option_labels: &labels,
                ..proposal(&option_id, title, None, Some(QUESTION))
            })
            .expect("propose decision")
            .2
    };

    let first = propose("Use SQLite for the prototype")
        .question
        .expect("question");
    let second = propose("Use Postgres for the prototype")
        .question
        .expect("question");

    assert!(!first.reused);
    assert!(second.reused);
    assert_eq!(first.question_id, second.question_id);
}

#[test]
fn different_questions_get_different_nodes() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    capture(&commands, "Use SQLite for the prototype", Some(QUESTION));
    capture(
        &commands,
        "Ship the prototype in October",
        Some("When should the prototype ship?"),
    );

    let all = events(&ledger);
    let ids: Vec<&str> = of_type(&all, EventType::QuestionRecorded)
        .into_iter()
        .map(|event| payload_str(event, "question_id"))
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn a_capture_without_a_question_writes_no_question_events() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    capture(&commands, "Use SQLite for the prototype", None);

    let all = events(&ledger);
    assert!(of_type(&all, EventType::QuestionRecorded).is_empty());
    assert!(answers_links(&all).is_empty());
}

#[test]
fn a_question_of_only_punctuation_is_refused_and_nothing_is_written() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option(PROPOSER, "Only option", "The only option")
        .expect("option");
    let topics = ["storage".to_owned()];
    let labels = ["Only option".to_owned()];
    let before = events(&ledger).len();

    let refused = commands.propose_decision(DecisionProposalInput {
        topic_keys: &topics,
        option_labels: &labels,
        ..proposal(&option_id, "Use SQLite for the prototype", None, Some("?!"))
    });

    let error = refused
        .expect_err("a question with no words names nothing")
        .to_string();
    assert!(error.contains("must contain words"), "{error}");
    assert_eq!(events(&ledger).len(), before);
}

#[test]
fn a_quote_still_requires_its_question_but_a_question_stands_alone() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option(PROPOSER, "Only option", "The only option")
        .expect("option");
    let topics = ["storage".to_owned()];
    let labels = ["Only option".to_owned()];
    let propose = |quote: Option<&str>, question: Option<&str>| {
        commands.propose_decision(DecisionProposalInput {
            topic_keys: &topics,
            option_labels: &labels,
            ..proposal(&option_id, "Use SQLite for the prototype", quote, question)
        })
    };

    let error = propose(Some("SQLite, obviously, for now"), None)
        .expect_err("a quote answers no stated question")
        .to_string();
    assert!(error.contains("quote requires question"), "{error}");
    assert!(events(&ledger)
        .iter()
        .all(|event| event.event_type != EventType::DecisionProposed));

    propose(None, Some(QUESTION)).expect("a question needs no quote");
    propose(Some("SQLite, obviously, for now"), Some(QUESTION)).expect("a quote with its question");
}

#[test]
fn an_identical_retry_of_a_capture_writes_the_question_events_once() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let option_id = commands
        .record_option_with_id(PROPOSER, "option-only", "Only option", "The only option")
        .expect("option");
    let topics = ["storage".to_owned()];
    let labels = ["Only option".to_owned()];
    let propose = || {
        commands
            .propose_decision_with_id(
                DecisionProposalInput {
                    topic_keys: &topics,
                    option_labels: &labels,
                    ..proposal(
                        &option_id,
                        "Use SQLite for the prototype",
                        None,
                        Some(QUESTION),
                    )
                },
                "decision-retry",
                DecisionProposalEventUuids {
                    proposal: Uuid::from_u128(1),
                    has_option: vec![Uuid::from_u128(2)],
                    chose: None,
                    assumes: Vec::new(),
                    based_on: Vec::new(),
                    follows_from: Vec::new(),
                },
            )
            .expect("propose decision")
    };

    propose();
    let after_first = events(&ledger).len();
    propose();

    assert_eq!(
        events(&ledger).len(),
        after_first,
        "the ledger deduplicates every event of the retry, the question's included"
    );
    let all = events(&ledger);
    assert_eq!(of_type(&all, EventType::QuestionRecorded).len(), 1);
    assert_eq!(answers_links(&all).len(), 1);
}

#[test]
fn a_question_recorded_under_another_id_is_reused() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let event = commands
        .event_with_uuid(
            GROUNDER,
            EventPayload::QuestionRecorded(QuestionRecordedPayload {
                question_id: "question-hand-made".to_owned(),
                text: "which storage engine should the prototype use".to_owned(),
            }),
            None,
            Uuid::new_v4(),
        )
        .expect("event");
    commands.append_event(event).expect("append");

    let decision_id = capture(&commands, "Use SQLite for the prototype", Some(QUESTION));

    let all = events(&ledger);
    assert_eq!(of_type(&all, EventType::QuestionRecorded).len(), 1);
    assert_eq!(
        answers_links(&all),
        vec![(decision_id.as_str(), "question-hand-made")]
    );
}

#[test]
fn a_decision_can_be_linked_to_its_question_later_attributed_to_the_grounder() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = capture(&commands, "Use SQLite for the prototype", None);
    let before = events(&ledger).len();

    let answered = commands
        .answer_question(GROUNDER, &decision_id, QUESTION)
        .expect("answer question");

    assert!(!answered.reused);
    let appended: Vec<Event> = events(&ledger).into_iter().skip(before).collect();
    assert_eq!(appended.len(), 2);
    assert!(appended.iter().all(|event| event.actor_id == GROUNDER));
    assert!(
        appended
            .iter()
            .all(|event| event.causation_event_id.is_none()),
        "a later link carries no causation to the proposal: that is how it reads as attributed later"
    );
    assert_eq!(
        answers_links(&appended),
        vec![(decision_id.as_str(), answered.question_id.as_str())]
    );
}

#[test]
fn answering_a_question_the_decision_already_answers_writes_nothing() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = capture(&commands, "Use SQLite for the prototype", Some(QUESTION));
    let before = events(&ledger).len();

    let answered = commands
        .answer_question(
            GROUNDER,
            &decision_id,
            "which storage engine should the prototype use?",
        )
        .expect("same question, another spelling");

    assert!(answered.reused);
    assert_eq!(events(&ledger).len(), before);
}

#[test]
fn a_decision_answers_one_question_a_second_different_one_is_refused() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = capture(&commands, "Use SQLite for the prototype", Some(QUESTION));
    let before = events(&ledger).len();

    let error = commands
        .answer_question(GROUNDER, &decision_id, "When should the prototype ship?")
        .expect_err("a decision answers one question")
        .to_string();

    assert!(error.contains("already answers question"), "{error}");
    assert_eq!(events(&ledger).len(), before, "nothing was written");
}

#[test]
fn answering_refuses_an_unknown_decision_and_a_blank_question() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = capture(&commands, "Use SQLite for the prototype", None);
    let before = events(&ledger).len();

    let unknown = commands
        .answer_question(GROUNDER, "decision-missing", QUESTION)
        .expect_err("no such decision")
        .to_string();
    assert!(unknown.contains("decision does not exist"), "{unknown}");
    for blank in ["", "   ", "?"] {
        assert!(
            commands
                .answer_question(GROUNDER, &decision_id, blank)
                .is_err(),
            "{blank:?} names no question"
        );
    }
    assert_eq!(events(&ledger).len(), before);
}

#[test]
fn ground_and_answer_may_carry_only_the_question_but_never_nothing() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = capture(&commands, "Use SQLite for the prototype", None);
    let before = events(&ledger).len();

    let nothing = commands
        .ground_and_answer(GROUNDER, &decision_id, &GroundingPlan::default(), None)
        .expect_err("nothing to record")
        .to_string();
    assert!(nothing.contains(GROUNDING_REQUIRED_MESSAGE), "{nothing}");
    assert_eq!(events(&ledger).len(), before);

    let added = commands
        .ground_and_answer(
            GROUNDER,
            &decision_id,
            &GroundingPlan::default(),
            Some(QUESTION),
        )
        .expect("the question alone is enough");
    assert!(added.rests_on.is_empty());
    assert!(added.relation_event_ids.is_empty());
    assert!(added.question.is_some());
}

#[test]
fn ground_and_answer_resolves_the_question_before_the_first_write() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision_id = capture(&commands, "Use SQLite for the prototype", Some(QUESTION));
    let before = events(&ledger).len();
    let plan = GroundingPlan {
        new_assumptions: vec!["writes stay under one per second".to_owned()],
        ..GroundingPlan::default()
    };

    let error = commands
        .ground_and_answer(
            GROUNDER,
            &decision_id,
            &plan,
            Some("When should the prototype ship?"),
        )
        .expect_err("the decision already answers another question")
        .to_string();

    assert!(error.contains("already answers question"), "{error}");
    assert_eq!(
        events(&ledger).len(),
        before,
        "the refused question leaves no orphan assumption behind"
    );
}

#[test]
fn record_ask_writes_question_recorded_then_question_asked() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let plan = commands.plan_ask(QUESTION).expect("plan ask");
    let recorded = commands.record_ask(PROPOSER, &plan).expect("record ask");

    let all = events(&ledger);
    let question_recorded = of_type(&all, EventType::QuestionRecorded);
    assert_eq!(question_recorded.len(), 1);
    assert_eq!(
        payload_str(question_recorded[0], "question_id"),
        recorded.question_id
    );
    let asked = of_type(&all, EventType::QuestionAsked);
    assert_eq!(asked.len(), 1);
    assert_eq!(payload_str(asked[0], "question_id"), recorded.question_id);
    assert_eq!(payload_str(asked[0], "text"), QUESTION);
    assert_eq!(asked[0].event_uuid.to_string(), recorded.request_id);
    assert!(!recorded.reused, "a new question is not reused");
}

#[test]
fn asking_the_same_question_twice_reuses_the_node_but_writes_two_asks() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    let first_plan = commands.plan_ask(QUESTION).expect("plan first ask");
    let first = commands
        .record_ask(PROPOSER, &first_plan)
        .expect("record first ask");
    let second_plan = commands
        .plan_ask("  which storage engine   should the PROTOTYPE use ")
        .expect("plan second ask");
    let second = commands
        .record_ask(GROUNDER, &second_plan)
        .expect("record second ask");

    assert_eq!(
        first.question_id, second.question_id,
        "same normalized text shares one node"
    );
    assert_ne!(
        first.request_id, second.request_id,
        "each ask is its own request"
    );
    assert!(
        second.reused,
        "the second ask reuses the existing question node"
    );

    let all = events(&ledger);
    assert_eq!(
        of_type(&all, EventType::QuestionRecorded).len(),
        1,
        "question.recorded is not repeated"
    );
    assert_eq!(
        of_type(&all, EventType::QuestionAsked).len(),
        2,
        "each ask writes its own question.asked"
    );
}

#[test]
fn find_ask_resolves_a_recorded_request_and_none_for_an_unknown_one() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let plan = commands.plan_ask(QUESTION).expect("plan ask");
    let recorded = commands.record_ask(PROPOSER, &plan).expect("record ask");

    let found = commands
        .find_ask(&recorded.request_id)
        .expect("find ask")
        .expect("the request exists");
    assert_eq!(found.question_id, recorded.question_id);
    assert_eq!(found.text, QUESTION);

    let missing = commands
        .find_ask("018f5d8a-03fb-7df0-8e36-64d7410cfe07")
        .expect("find ask does not error on an unknown id");
    assert!(missing.is_none());

    let not_a_uuid = commands
        .find_ask("not-a-uuid")
        .expect("find ask does not error on a malformed id");
    assert!(not_a_uuid.is_none());
}

#[test]
fn resolve_answers_request_resolves_a_request_to_its_question_text() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let plan = commands.plan_ask(QUESTION).expect("plan ask");
    let recorded = commands.record_ask(PROPOSER, &plan).expect("record ask");

    let resolved = commands
        .resolve_answers_request(Some(&recorded.request_id), None)
        .expect("resolve answers request");
    assert_eq!(resolved.as_deref(), Some(QUESTION));

    // No request id: question passes through unchanged.
    let passthrough = commands
        .resolve_answers_request(None, Some("some other question"))
        .expect("resolve with no request id");
    assert_eq!(passthrough.as_deref(), Some("some other question"));

    // Both given: refused.
    let conflict = commands
        .resolve_answers_request(Some(&recorded.request_id), Some("some other question"))
        .expect_err("answers and question together are refused")
        .to_string();
    assert!(conflict.contains("mutually exclusive"), "{conflict}");

    // Unknown request id: refused.
    let unknown = commands
        .resolve_answers_request(Some("018f5d8a-03fb-7df0-8e36-64d7410cfe07"), None)
        .expect_err("an unknown request id is refused")
        .to_string();
    assert!(unknown.contains("no request with id"), "{unknown}");
}
