// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::commands::{ClassifiedBatchRecorded, CommandContext, Commands};
use crate::events::{
    CaptureItem, Event, EventProvenance, EventType, IngestBatchReceivedPayload, TenantId,
};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::{get_decision_brief, get_waiting_requests, WaitingRequestsRequest};

const SUBMITTER: &str = "agent:claude:hook";
const RECORDER: &str = "agent:hivemind:classifier";

/// A received batch and the classifier's captures for it, as the transcript fixtures hold them.
#[derive(Deserialize)]
struct Fixture {
    batch: IngestBatchReceivedPayload,
    captures: Vec<CaptureItem>,
}

const DECISION_IN_ITS_OWN_TURN: &str =
    include_str!("../../../tests/fixtures/transcripts/decision_in_its_own_turn.json");
const REQUEST_THEN_ANSWER: &str =
    include_str!("../../../tests/fixtures/transcripts/request_then_answer.json");
const AGENT_DECIDES_ALONE: &str =
    include_str!("../../../tests/fixtures/transcripts/agent_decides_alone.json");

fn moment(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test time parses")
        .with_timezone(&Utc)
}

fn commands(ledger: &InMemoryEventLedger) -> Commands<'_, InMemoryEventLedger> {
    Commands::new_with_context(
        ledger,
        CommandContext::new(TenantId::local(), EventProvenance::agent(RECORDER)),
    )
}

/// Receives the fixture's batch, then records the classification of it under `batch_id`.
fn record_as(
    ledger: &InMemoryEventLedger,
    fixture: &str,
    batch_id: &str,
) -> crate::Result<ClassifiedBatchRecorded> {
    let fixture: Fixture = serde_json::from_str(fixture).expect("fixture parses");
    let commands = commands(ledger);
    commands.record_ingest_batch(
        SUBMITTER,
        batch_id,
        &fixture.batch.agent_tool,
        &fixture.batch.session_id,
        fixture.batch.turns,
    )?;
    commands.record_ingest_batch_classified(
        RECORDER,
        &[batch_id.to_owned()],
        "test-model",
        "2",
        fixture.captures,
        None,
    )
}

fn record(ledger: &InMemoryEventLedger, fixture: &str) -> ClassifiedBatchRecorded {
    let batch_id = serde_json::from_str::<Fixture>(fixture)
        .expect("fixture parses")
        .batch
        .batch_id;
    record_as(ledger, fixture, &batch_id).expect("classification is recorded")
}

/// A fixture re-keyed by hand: the same batch with its captures replaced by `captures`.
fn fixture_with(fixture: &str, captures: &Value) -> String {
    let mut fixture: Value = serde_json::from_str(fixture).expect("fixture parses");
    fixture["captures"] = captures.clone();
    fixture.to_string()
}

fn events_of(ledger: &InMemoryEventLedger, event_type: EventType) -> Vec<Event> {
    ledger
        .read(0, 1000)
        .expect("ledger reads")
        .into_iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

fn answers_relations(ledger: &InMemoryEventLedger) -> Vec<Event> {
    events_of(ledger, EventType::RelationAdded)
        .into_iter()
        .filter(|event| event.payload["relation"] == "ANSWERS")
        .collect()
}

fn graph_of(ledger: &InMemoryEventLedger) -> MemoryGraph {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, &TenantId::local(), &graph).expect("graph rebuilds");
    graph
}

fn stored_capture(
    ledger: &InMemoryEventLedger,
    recorded: &ClassifiedBatchRecorded,
    index: usize,
) -> Value {
    let event = ledger
        .read(recorded.event_id - 1, 1)
        .expect("ledger reads")
        .remove(0);
    event.payload["captures"][index].clone()
}

/// (a) A decision the classifier derived is recorded at the time of the turn it came from, not
/// when it was classified.
#[test]
fn a_derived_decision_carries_the_time_of_its_own_turn() {
    let ledger = InMemoryEventLedger::new();
    let recorded = record(&ledger, DECISION_IN_ITS_OWN_TURN);

    let turn_time = moment("2026-09-01T09:05:00Z");
    let capture = stored_capture(&ledger, &recorded, 0);
    assert_eq!(capture["source_turn_id"], "t-2");
    assert_eq!(capture["source_ts"], "2026-09-01T09:05:00Z");

    let decision_id = format!("capture:{}:0", recorded.event_id);
    let brief = get_decision_brief(&graph_of(&ledger), &decision_id)
        .expect("brief reads")
        .data
        .expect("decision is in the graph");
    assert_eq!(brief.occurred_at, Some(turn_time));
    // Not the import time: the classification event itself was written just now.
    let classified = ledger
        .read(recorded.event_id - 1, 1)
        .expect("ledger reads")
        .remove(0);
    assert!(classified.ts.expect("event carries a time") > turn_time);
}

/// With no turn time, nothing is guessed: the decision stays at the classification's time.
#[test]
fn a_decision_whose_turn_has_no_time_stays_at_the_classification_time() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(DECISION_IN_ITS_OWN_TURN).expect("parses");
    for turn in fixture["batch"]["turns"].as_array_mut().expect("turns") {
        turn.as_object_mut().expect("turn").remove("ts");
    }
    let recorded = record_as(&ledger, &fixture.to_string(), "batch-no-times")
        .expect("classification recorded");

    let capture = stored_capture(&ledger, &recorded, 0);
    assert_eq!(capture["source_turn_id"], "t-2");
    assert!(capture.get("source_ts").is_none());

    let classified = ledger
        .read(recorded.event_id - 1, 1)
        .expect("ledger reads")
        .remove(0);
    let decision_id = format!("capture:{}:0", recorded.event_id);
    let brief = get_decision_brief(&graph_of(&ledger), &decision_id)
        .expect("brief reads")
        .data
        .expect("decision is in the graph");
    assert_eq!(brief.occurred_at, classified.ts);
}

/// A capture that names no turn is recorded exactly as before.
#[test]
fn a_capture_that_names_no_turn_is_recorded_at_the_classification_time() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(DECISION_IN_ITS_OWN_TURN).expect("parses");
    fixture["captures"][0]
        .as_object_mut()
        .expect("capture")
        .remove("source_turn_id");
    let recorded =
        record_as(&ledger, &fixture.to_string(), "batch-no-turn").expect("classification recorded");

    let capture = stored_capture(&ledger, &recorded, 0);
    assert!(capture.get("source_turn_id").is_none());
    assert!(capture.get("source_ts").is_none());
}

/// Only a received turn's own time counts: a `source_ts` the submission carries is replaced.
#[test]
fn a_source_time_the_submission_carries_is_replaced_by_the_turns_own() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(DECISION_IN_ITS_OWN_TURN).expect("parses");
    fixture["captures"][0]["source_ts"] = json!("2000-01-01T00:00:00Z");
    let recorded =
        record_as(&ledger, &fixture.to_string(), "batch-forged").expect("classification recorded");
    let capture = stored_capture(&ledger, &recorded, 0);
    assert_eq!(capture["source_ts"], "2026-09-01T09:05:00Z");

    // And with no turn named there is no time to take, so the forged one is dropped.
    fixture["captures"][0]
        .as_object_mut()
        .expect("capture")
        .remove("source_turn_id");
    let recorded = record_as(&ledger, &fixture.to_string(), "batch-forged-no-turn")
        .expect("classification recorded");
    assert!(stored_capture(&ledger, &recorded, 0)
        .get("source_ts")
        .is_none());
}

/// (b) A request turn followed by an answering turn yields one `question.asked` at the request
/// turn's time, and a decision linked to the same question.
#[test]
fn a_request_turn_then_an_answering_turn_is_one_ask_and_a_linked_decision() {
    let ledger = InMemoryEventLedger::new();
    let recorded = record(&ledger, REQUEST_THEN_ANSWER);

    let asks = events_of(&ledger, EventType::QuestionAsked);
    assert_eq!(asks.len(), 1, "one request turn is one ask");
    let ask = &asks[0];
    assert_eq!(ask.ts, Some(moment("2026-09-02T10:00:00Z")));
    assert_eq!(ask.actor_id, "agent:claude:builder");
    assert_eq!(
        ask.payload["text"],
        "Which database should the hosted MVP use?"
    );

    // One question node, shared by the ask and the answer: the wording differs in case and
    // trailing punctuation only.
    assert_eq!(events_of(&ledger, EventType::QuestionRecorded).len(), 1);
    let answers = answers_relations(&ledger);
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].payload["to_id"], ask.payload["question_id"]);
    let decision_id = format!("capture:{}:1", recorded.event_id);
    assert_eq!(answers[0].payload["from_id"], decision_id.as_str());
    assert_eq!(answers[0].causation_event_id, Some(recorded.event_id));
    assert_eq!(answers[0].ts, Some(moment("2026-09-02T10:30:00Z")));
    assert_eq!(answers[0].actor_id, RECORDER);

    // The ledger reads ask, then the classification, then the answer.
    assert!(ask.event_id.expect("ask has an offset") < recorded.event_id);
    assert!(answers[0].event_id.expect("answer has an offset") > recorded.event_id);

    // A reader sees both times, and the question is no longer waiting.
    let graph = graph_of(&ledger);
    let brief = get_decision_brief(&graph, &decision_id)
        .expect("brief reads")
        .data
        .expect("decision is in the graph");
    assert_eq!(brief.asked_at, Some(moment("2026-09-02T10:00:00Z")));
    assert_eq!(brief.occurred_at, Some(moment("2026-09-02T10:30:00Z")));
    let waiting = get_waiting_requests(
        &graph,
        &WaitingRequestsRequest {
            limit: 10,
            cursor: None,
        },
    )
    .expect("waiting reads")
    .data;
    assert!(waiting.items.is_empty());
}

/// A request nobody has answered is waiting, asked at the request turn's own time.
#[test]
fn a_request_turn_nobody_answered_is_waiting() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"]
        .as_array_mut()
        .expect("captures")
        .truncate(1);
    record_as(&ledger, &fixture.to_string(), "batch-unanswered").expect("classification recorded");

    let waiting = get_waiting_requests(
        &graph_of(&ledger),
        &WaitingRequestsRequest {
            limit: 10,
            cursor: None,
        },
    )
    .expect("waiting reads")
    .data;
    assert_eq!(waiting.items.len(), 1);
    assert_eq!(waiting.items[0].asked_at, moment("2026-09-02T10:00:00Z"));
    assert_eq!(
        waiting.items[0].requested_by.as_deref(),
        Some("agent:claude:builder")
    );
}

/// The same transcript ingested again adds no second ask: it is the same moment seen again.
#[test]
fn a_re_ingested_request_turn_is_not_asked_twice() {
    let ledger = InMemoryEventLedger::new();
    record(&ledger, REQUEST_THEN_ANSWER);
    record_as(&ledger, REQUEST_THEN_ANSWER, "batch-request-answer-again")
        .expect("classification recorded");

    assert_eq!(events_of(&ledger, EventType::QuestionAsked).len(), 1);
    assert_eq!(events_of(&ledger, EventType::QuestionRecorded).len(), 1);
}

/// Without an actor named on the request, the batch's submitter is the one who asked.
#[test]
fn an_ask_with_no_named_asker_is_by_the_batch_submitter() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][0]
        .as_object_mut()
        .expect("capture")
        .remove("actor_id");
    record_as(&ledger, &fixture.to_string(), "batch-unnamed-asker").expect("recorded");

    let asks = events_of(&ledger, EventType::QuestionAsked);
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].actor_id, SUBMITTER);
}

/// (c) An agent deciding alone mid-task makes no ask.
#[test]
fn an_agent_deciding_alone_writes_no_ask() {
    let ledger = InMemoryEventLedger::new();
    let recorded = record(&ledger, AGENT_DECIDES_ALONE);

    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
    assert!(events_of(&ledger, EventType::QuestionRecorded).is_empty());
    assert!(answers_relations(&ledger).is_empty());
    // Its own turn's time is still the decision's.
    let decision_id = format!("capture:{}:0", recorded.event_id);
    let brief = get_decision_brief(&graph_of(&ledger), &decision_id)
        .expect("brief reads")
        .data
        .expect("decision is in the graph");
    assert_eq!(brief.occurred_at, Some(moment("2026-09-03T11:00:00Z")));
    assert_eq!(brief.asked_at, None);
}

/// A decision that states the question it answers is linked to it, but with no request turn
/// before it nobody asked: no ask, and the timeline reads "asked at: not recorded".
#[test]
fn a_decision_stating_its_question_with_no_request_turn_is_linked_but_not_asked() {
    let ledger = InMemoryEventLedger::new();
    let mut captures =
        serde_json::from_str::<Value>(AGENT_DECIDES_ALONE).expect("parses")["captures"].clone();
    captures[0]["question"] = json!("How many times should the queue retry a failed job?");
    let recorded = record_as(
        &ledger,
        &fixture_with(AGENT_DECIDES_ALONE, &captures),
        "batch-states-question",
    )
    .expect("classification recorded");

    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
    assert_eq!(events_of(&ledger, EventType::QuestionRecorded).len(), 1);
    assert_eq!(answers_relations(&ledger).len(), 1);
    let decision_id = format!("capture:{}:0", recorded.event_id);
    let brief = get_decision_brief(&graph_of(&ledger), &decision_id)
        .expect("brief reads")
        .data
        .expect("decision is in the graph");
    assert_eq!(brief.asked_at, None);
}

/// A request turn with no time would be dated by when it was classified, so it writes no ask.
#[test]
fn a_request_turn_with_no_time_writes_no_ask() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["batch"]["turns"][0]
        .as_object_mut()
        .expect("turn")
        .remove("ts");
    record_as(&ledger, &fixture.to_string(), "batch-request-no-time").expect("recorded");

    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
    // The answer still links to its question, at its own turn's time.
    assert_eq!(answers_relations(&ledger).len(), 1);
}

/// A request that states no question has no words to ask: no ask is written.
#[test]
fn a_request_with_no_question_writes_no_ask() {
    let ledger = InMemoryEventLedger::new();
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][0]
        .as_object_mut()
        .expect("capture")
        .remove("question");
    record_as(&ledger, &fixture.to_string(), "batch-request-no-question").expect("recorded");

    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
}

/// Every refusal leaves nothing behind but the received batch.
fn assert_refused_with_nothing_written(fixture: &Value, expected: &str) {
    let ledger = InMemoryEventLedger::new();
    let error = record_as(&ledger, &fixture.to_string(), "batch-refused")
        .expect_err("classification is refused");
    assert!(
        error.to_string().contains(expected),
        "refusal should say {expected:?}, got: {error}"
    );
    let events = ledger.read(0, 100).expect("ledger reads");
    assert_eq!(events.len(), 1, "only the received batch is on the ledger");
    assert_eq!(events[0].event_type, EventType::IngestBatchReceived);
}

#[test]
fn a_turn_the_batch_does_not_hold_is_refused() {
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][1]["source_turn_id"] = json!("t-99");
    assert_refused_with_nothing_written(&fixture, "names turn t-99");
}

#[test]
fn a_blank_turn_id_is_refused() {
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][1]["source_turn_id"] = json!("  ");
    assert_refused_with_nothing_written(&fixture, "source_turn_id must not be empty");
}

#[test]
fn a_question_on_a_kind_that_takes_none_is_refused() {
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][1]["kind"] = json!("evidence");
    assert_refused_with_nothing_written(&fixture, "only a decision or a decision-request");
}

#[test]
fn a_question_with_no_words_is_refused() {
    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][0]["question"] = json!("?");
    assert_refused_with_nothing_written(&fixture, "no words in it");
}

/// A decision the same moment seen again is dropped as a restatement and writes no second link.
#[test]
fn a_decision_dropped_as_the_same_moment_seen_again_writes_no_link() {
    let ledger = InMemoryEventLedger::new();
    let first = record(&ledger, REQUEST_THEN_ANSWER);
    let restated_id = format!("capture:{}:1", first.event_id);

    let mut fixture: Value = serde_json::from_str(REQUEST_THEN_ANSWER).expect("parses");
    fixture["captures"][1]["restates_id"] = json!(restated_id);
    record_as(&ledger, &fixture.to_string(), "batch-request-answer-again").expect("recorded");

    assert_eq!(answers_relations(&ledger).len(), 1);
    assert_eq!(events_of(&ledger, EventType::QuestionAsked).len(), 1);
}
