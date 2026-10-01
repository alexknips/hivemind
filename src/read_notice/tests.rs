// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use serde_json::json;

use crate::events::{Event, EventId};
use crate::ledger::InMemoryEventLedger;

fn fixture(json_text: &str) -> Event {
    let mut event: Event = serde_json::from_str(json_text).expect("fixture parses");
    event.event_id = None;
    event.event_uuid = uuid::Uuid::new_v4();
    event
}

fn proposal() -> Event {
    fixture(include_str!(
        "../../tests/fixtures/v0/decision.proposed.json"
    ))
}

fn assessment() -> Event {
    fixture(include_str!(
        "../../tests/fixtures/v0/scoring/decision.scored.assessed.json"
    ))
}

fn malformed_assessment() -> Event {
    let mut event = assessment();
    event.payload["dimensions"]["framing"] = json!("not an answer");
    event
}

fn row(event_id: EventId, event_type: EventType) -> UnreadableAnnotation {
    UnreadableAnnotation {
        event_id: Some(event_id),
        event_type,
        reason: format!("reason {event_id}"),
    }
}

#[test]
fn the_scan_finds_only_the_annotation_rows_nobody_can_read() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    ledger.append(proposal())?;
    ledger.append(assessment())?;
    assert!(unreadable_annotations(&ledger, &TenantId::local())?.is_empty());

    ledger.append(malformed_assessment())?;
    ledger.append(assessment())?;
    ledger.append(malformed_assessment())?;

    let rows = unreadable_annotations(&ledger, &TenantId::local())?;
    assert_eq!(
        rows.iter().map(|row| row.event_id).collect::<Vec<_>>(),
        vec![Some(3), Some(5)],
        "ledger order, and the readable assessments are not listed"
    );
    assert!(rows
        .iter()
        .all(|row| row.event_type == EventType::DecisionScored));
    Ok(())
}

#[test]
fn the_notice_names_how_many_which_and_why_in_one_line() {
    let one = unreadable_annotations_notice(&[row(57, EventType::DecisionScored)]);
    assert_eq!(
        one,
        "1 assessment row could not be read and was skipped (ledger event 57: reason 57); \
         answers leave it out"
    );
    assert!(!one.contains('\n'));

    let three = unreadable_annotations_notice(&[
        row(57, EventType::DecisionScored),
        row(60, EventType::DecisionScored),
        row(71, EventType::DecisionScored),
    ]);
    assert_eq!(
        three,
        "3 assessment rows could not be read and were skipped (ledger events 57, 60, 71; \
         first: reason 57); answers leave them out"
    );

    // Long lists say how many more, never a silent cut.
    let many: Vec<_> = (1..=8)
        .map(|id| row(id, EventType::DecisionScored))
        .collect();
    assert!(
        unreadable_annotations_notice(&many).contains("(ledger events 1, 2, 3, 4, 5 and 3 more;")
    );

    // Anything but an assessment is called what it is: an annotation.
    let mixed = unreadable_annotations_notice(&[
        row(1, EventType::DecisionScored),
        row(2, EventType::DecisionMetadataDerived),
    ]);
    assert!(mixed.starts_with("2 annotation rows could not be read"));
}

#[test]
fn no_unreadable_rows_means_no_notice() {
    assert_eq!(notice_for(&[]), None);
    assert!(notice_for(&[row(1, EventType::DecisionScored)]).is_some());
}

#[test]
fn a_json_answer_gets_a_notice_key_and_keeps_its_shape() {
    let compact = annotate_output(r#"{"data":{"x":1}}"#.to_owned(), "n");
    assert!(!compact.contains('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(&compact).expect("still json"),
        json!({"data": {"x": 1}, "notice": "n"})
    );

    let pretty = annotate_output("{\n  \"data\": 1\n}".to_owned(), "n");
    assert!(pretty.contains('\n'), "a pretty answer stays pretty");
    assert_eq!(
        serde_json::from_str::<Value>(&pretty).expect("still json"),
        json!({"data": 1, "notice": "n"})
    );

    let mut answer = json!([1, 2]);
    annotate_json(&mut answer, "n");
    assert_eq!(answer, json!([1, 2]), "only an object can carry a key");
}

#[test]
fn a_text_answer_gets_a_final_notice_line() {
    assert_eq!(
        annotate_output("decision: A\nwhy: B".to_owned(), "n"),
        "decision: A\nwhy: B\nnotice: n"
    );
    assert_eq!(annotate_output(String::new(), "n"), "notice: n");
    // JSON that is not an object is not an answer with keys; it is text to this function.
    assert_eq!(annotate_output("[1]".to_owned(), "n"), "[1]\nnotice: n");
}
