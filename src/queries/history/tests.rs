// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::TimeZone;
use serde_json::json;
use uuid::Uuid;

use crate::ledger::InMemoryEventLedger;

use super::*;

#[test]
fn recent_activity_is_bounded_newest_first_and_cited() -> Result<()> {
    let ledger = fixture_ledger()?;

    let response = get_recent_activity(
        &ledger,
        &RecentActivityRequest {
            limit: 2,
            ..RecentActivityRequest::default()
        },
    )?;

    assert_eq!(response.result_count, 2);
    assert!(response.truncated);
    assert_eq!(response.data.next_cursor.as_deref(), Some("2"));
    assert!(
        response.data.items[0].event_origin > response.data.items[1].event_origin,
        "recent activity must be newest first"
    );
    assert_eq!(response.data.items[0].source, EventSource::Slack);
    assert_eq!(
        response.data.items[0].source_ref.as_deref(),
        Some("thread-1")
    );
    assert_eq!(response.data.items[0].citation_id, "event:8");

    Ok(())
}

#[test]
fn changed_since_classifies_decision_history_and_paginates() -> Result<()> {
    let ledger = fixture_ledger()?;

    let response = get_decisions_changed_since(
        &ledger,
        &ChangedSinceRequest {
            since_offset: Some(1),
            limit: 4,
            ..ChangedSinceRequest::default()
        },
    )?;

    assert_eq!(response.result_count, 4);
    assert!(response.truncated);
    assert_eq!(response.data.next_cursor.as_deref(), Some("4"));
    let kinds = response
        .data
        .items
        .iter()
        .map(|row| row.change_kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            HistoryChangeKind::ContextChange,
            HistoryChangeKind::NewDecision,
            HistoryChangeKind::StatusChange,
            HistoryChangeKind::NewDecision,
        ]
    );

    let second_page = get_decisions_changed_since(
        &ledger,
        &ChangedSinceRequest {
            since_offset: Some(1),
            limit: 10,
            cursor: Some("4".to_owned()),
            ..ChangedSinceRequest::default()
        },
    )?;
    let kinds = second_page
        .data
        .items
        .iter()
        .map(|row| row.change_kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            HistoryChangeKind::NewEvidence,
            HistoryChangeKind::RefutedAssumption,
            HistoryChangeKind::Supersession,
        ]
    );
    let refuted = second_page
        .data
        .items
        .iter()
        .find(|row| row.change_kind == HistoryChangeKind::RefutedAssumption)
        .expect("refuted assumption row");
    assert_eq!(refuted.decision_ids, vec!["decision-a".to_owned()]);
    assert_eq!(refuted.source_ref.as_deref(), Some("thread-1"));

    Ok(())
}

#[test]
fn changed_since_resolves_timestamp_bounds_to_offsets() -> Result<()> {
    let ledger = fixture_ledger()?;

    let response = get_decisions_changed_since(
        &ledger,
        &ChangedSinceRequest {
            since_timestamp: Some(ts(3)),
            until_timestamp: Some(ts(6)),
            limit: 10,
            ..ChangedSinceRequest::default()
        },
    )?;

    assert_eq!(response.data.resolved_since.offset, 3);
    assert_eq!(response.data.resolved_until.offset, 6);
    assert_eq!(
        response.data.boundary_event_offsets.since_timestamp_offset,
        Some(3)
    );
    assert_eq!(
        response.data.boundary_event_offsets.until_timestamp_offset,
        Some(6)
    );
    assert_eq!(
        response
            .data
            .items
            .iter()
            .map(|row| row.event_origin)
            .collect::<Vec<_>>(),
        vec![4, 5, 6]
    );

    Ok(())
}

#[test]
fn changed_since_handles_request_blocker_and_notification_events() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let notification_id = Uuid::from_u128(3).to_string();
    for event in [
        event(
            1,
            EventType::DecisionRequested,
            "actor:requester",
            json!({
                "topic_keys": ["ops"],
                "decision_id": "decision-a",
                "reason": "Need an owner decision",
                "priority": "P1",
                "required_owner_id": "actor:owner",
                "authority_class": "operational",
                "requested_by": "actor:requester",
                "client_request_id": "request-1"
            }),
        ),
        event(
            2,
            EventType::BlockerReported,
            "actor:blocked",
            json!({
                "blocker_id": "blocker-1",
                "blocked_actor_id": "actor:blocked",
                "decision_id": "decision-a",
                "topic_keys": ["ops"],
                "blocked_ref": "task-1",
                "blocked_ref_type": "bead",
                "reason": "Needs decision",
                "priority": "P1",
                "last_progress_at": "2026-05-19T12:00:00Z",
                "required_owner_id": "actor:owner"
            }),
        ),
        event(
            3,
            EventType::NotificationSent,
            "actor:notifier",
            json!({
                "blocker_id": "blocker-1",
                "recipient_actor_id": "actor:owner",
                "channel": "gc",
                "threshold_rule": "p1",
                "source_event_ids": [2],
                "dedupe_key": "blocker-1:p1",
                "sent_at": "2026-05-19T12:00:03Z"
            }),
        ),
        event(
            4,
            EventType::BlockerResolved,
            "actor:owner",
            json!({
                "blocker_id": "blocker-1",
                "resolution_event_id": 4,
                "resolution_reason": "Decision owner responded"
            }),
        ),
        event(
            5,
            EventType::NotificationAcknowledged,
            "actor:owner",
            json!({
                "notification_id": notification_id,
                "ack_at": "2026-05-19T12:00:05Z",
                "snooze_until": null
            }),
        ),
    ] {
        ledger.append(event)?;
    }

    let response = get_decisions_changed_since(
        &ledger,
        &ChangedSinceRequest {
            since_offset: Some(0),
            limit: 10,
            ..ChangedSinceRequest::default()
        },
    )?;

    assert_eq!(response.result_count, 5);
    assert!(response
        .data
        .items
        .iter()
        .all(|row| row.change_kind == HistoryChangeKind::ContextChange));
    assert_eq!(response.data.items[0].decision_ids, vec!["decision-a"]);
    assert_eq!(response.data.items[1].decision_ids, vec!["decision-a"]);
    assert!(response.data.items[2].decision_ids.is_empty());
    assert!(response.data.items[3].decision_ids.is_empty());
    assert!(response.data.items[4].decision_ids.is_empty());
    assert!(response.data.items[0]
        .affected_nodes
        .contains(&affected_node(
            &Uuid::from_u128(1).to_string(),
            NodeKind::DecisionRequest
        )));
    assert!(response.data.items[1]
        .affected_nodes
        .contains(&affected_node("blocker-1", NodeKind::Blocker)));
    assert!(response.data.items[2]
        .affected_nodes
        .contains(&affected_node(
            &Uuid::from_u128(3).to_string(),
            NodeKind::Notification
        )));
    assert!(response.data.items[3]
        .affected_nodes
        .contains(&affected_node("blocker-1", NodeKind::Blocker)));
    assert!(response.data.items[4]
        .affected_nodes
        .contains(&affected_node(
            &Uuid::from_u128(3).to_string(),
            NodeKind::Notification
        )));

    Ok(())
}

#[test]
fn export_summary_includes_query_params_ledger_range_and_citations() -> Result<()> {
    let ledger = fixture_ledger()?;
    let generated_at = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();

    let response = export_read_only_summary(
        &ledger,
        &ReadOnlyExportRequest {
            query: ReadOnlyExportQuery::RecentActivity(RecentActivityRequest {
                limit: 1,
                ..RecentActivityRequest::default()
            }),
            format: ReadOnlyExportFormat::Markdown,
            generated_at,
        },
    )?;

    assert_eq!(response.result_count, 1);
    assert!(response.truncated);
    assert_eq!(response.data.generated_at, generated_at);
    assert_eq!(response.data.ledger_range.to_offset_inclusive, 8);
    assert_eq!(response.data.continuation_cursor.as_deref(), Some("1"));
    assert_eq!(
        response.data.query_params.get("query"),
        Some(&json!("recent_activity"))
    );
    assert!(response.data.citation_map.contains_key("event:8"));
    assert!(response.data.json.is_none());
    assert!(response
        .data
        .markdown
        .as_deref()
        .expect("markdown")
        .contains("citation=event:8"));

    Ok(())
}

fn fixture_ledger() -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for (index, event) in [
        event(
            1,
            EventType::EvidenceRecorded,
            "actor:researcher",
            json!({
                "evidence_id": "evidence-1",
                "content": "Baseline evidence",
                "source": "seed"
            }),
        ),
        event(
            2,
            EventType::HypothesisRecorded,
            "actor:researcher",
            json!({
                "hypothesis_id": "hypothesis-1",
                "statement": "Queue pressure stays low"
            }),
        ),
        event(
            3,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision-a",
                "title": "Use batch queue",
                "rationale": "It keeps writes simple",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hypothesis-1"],
                "evidence_ids": ["evidence-1"]
            }),
        ),
        event(
            4,
            EventType::DecisionAccepted,
            "actor:reviewer",
            json!({ "decision_id": "decision-a" }),
        ),
        event(
            5,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision-b",
                "title": "Use streaming queue",
                "rationale": "It reduces latency",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        event(
            6,
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": "BASED_ON",
                "from_id": "decision-b",
                "to_id": "evidence-1"
            }),
        ),
        event(
            7,
            EventType::RelationAdded,
            "actor:auditor",
            json!({
                "relation": "REFUTES",
                "from_id": "evidence-1",
                "to_id": "hypothesis-1"
            }),
        ),
        event(
            8,
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({
                "old_decision_id": "decision-a",
                "new_decision_id": "decision-b"
            }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut event = event;
        if index >= 6 {
            event.source = EventSource::Slack;
            event.source_ref = Some("thread-1".to_owned());
        }
        ledger.append(event)?;
    }
    Ok(ledger)
}

fn event(sequence: u64, event_type: EventType, actor_id: &str, payload: Value) -> Event {
    Event {
        event_id: None,
        event_uuid: Uuid::from_u128(u128::from(sequence)),
        correlation_id: Some("history-test".to_owned()),
        causation_event_id: None,
        event_type,
        actor_id: actor_id.to_owned(),
        source: EventSource::Api,
        source_ref: Some("history-test".to_owned()),
        payload,
        ts: Some(ts(sequence)),
    }
}

fn ts(sequence: u64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
        + chrono::Duration::seconds(i64::try_from(sequence).unwrap())
}

#[test]
fn added_since_returns_empty_for_window_before_any_event() -> Result<()> {
    let ledger = fixture_ledger()?;
    let response = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_timestamp: Some(Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap()),
            until_timestamp: Some(Utc.with_ymd_and_hms(2026, 5, 19, 11, 0, 0).unwrap()),
            limit: 10,
            ..DecisionsAddedSinceRequest::default()
        },
    )?;

    assert_eq!(response.data.total_added, 0);
    assert_eq!(response.data.total_changed_existing, 0);
    assert!(response.data.added_decisions.is_empty());
    assert!(response.data.changed_existing_decisions.is_empty());
    Ok(())
}

#[test]
fn added_since_separates_added_and_changed_with_deterministic_order() -> Result<()> {
    let ledger = fixture_ledger()?;

    let response = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_offset: Some(0),
            limit: 50,
            ..DecisionsAddedSinceRequest::default()
        },
    )?;

    assert_eq!(response.data.total_added, 2);
    assert_eq!(response.data.total_changed_existing, 0);
    let added_ids: Vec<_> = response
        .data
        .added_decisions
        .iter()
        .map(|entry| entry.decision_id.clone())
        .collect();
    assert_eq!(added_ids, vec!["decision-a", "decision-b"]);

    let decision_a = &response.data.added_decisions[0];
    assert_eq!(decision_a.creation.event_origin, 3);
    assert_eq!(decision_a.status, DecisionStatus::Superseded);
    assert_eq!(decision_a.evidence_ids, vec!["evidence-1"]);
    assert_eq!(decision_a.hypothesis_ids, vec!["hypothesis-1"]);
    let kinds: Vec<_> = decision_a
        .changes_in_window
        .iter()
        .map(|change| change.change_kind)
        .collect();
    assert!(kinds.contains(&HistoryChangeKind::StatusChange));
    assert!(kinds.contains(&HistoryChangeKind::RefutedAssumption));
    assert!(kinds.contains(&HistoryChangeKind::Supersession));

    let decision_b = &response.data.added_decisions[1];
    assert_eq!(decision_b.creation.event_origin, 5);
    assert!(decision_b
        .changes_in_window
        .iter()
        .any(|change| change.change_kind == HistoryChangeKind::NewEvidence));
    assert_eq!(decision_b.evidence_ids, vec!["evidence-1"]);

    Ok(())
}

#[test]
fn added_since_classifies_changed_existing_when_creation_is_before_window() -> Result<()> {
    let ledger = fixture_ledger()?;

    let response = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_offset: Some(5),
            limit: 50,
            ..DecisionsAddedSinceRequest::default()
        },
    )?;

    assert_eq!(response.data.total_added, 0);
    assert_eq!(response.data.total_changed_existing, 2);
    let changed_ids: Vec<_> = response
        .data
        .changed_existing_decisions
        .iter()
        .map(|entry| entry.decision_id.clone())
        .collect();
    assert_eq!(changed_ids, vec!["decision-b", "decision-a"]);

    let decision_b_change = &response.data.changed_existing_decisions[0];
    assert!(decision_b_change.creation.is_some());
    assert_eq!(decision_b_change.creation.as_ref().unwrap().event_origin, 5);
    let decision_a_change = &response.data.changed_existing_decisions[1];
    assert!(decision_a_change
        .changes_in_window
        .iter()
        .any(|change| change.change_kind == HistoryChangeKind::Supersession));

    Ok(())
}

#[test]
fn added_since_resolves_timestamps_to_offsets_and_reports_boundaries() -> Result<()> {
    let ledger = fixture_ledger()?;

    let response = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_timestamp: Some(ts(2)),
            until_timestamp: Some(ts(5)),
            limit: 10,
            ..DecisionsAddedSinceRequest::default()
        },
    )?;

    assert_eq!(response.data.resolved_since.offset, 2);
    assert_eq!(response.data.resolved_until.offset, 5);
    assert_eq!(
        response.data.boundary_event_offsets.since_timestamp_offset,
        Some(2)
    );
    assert_eq!(
        response.data.boundary_event_offsets.until_timestamp_offset,
        Some(5)
    );
    assert_eq!(response.data.total_added, 2);
    assert_eq!(response.data.total_changed_existing, 0);
    Ok(())
}

#[test]
fn added_since_paginates_with_continuation_cursor() -> Result<()> {
    let ledger = fixture_ledger()?;

    let first = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_offset: Some(0),
            limit: 1,
            ..DecisionsAddedSinceRequest::default()
        },
    )?;
    assert_eq!(first.result_count, 1);
    assert!(first.truncated);
    assert_eq!(first.data.next_cursor.as_deref(), Some("1"));
    assert_eq!(first.data.added_decisions.len(), 1);
    assert_eq!(first.data.added_decisions[0].decision_id, "decision-a");

    let second = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_offset: Some(0),
            limit: 1,
            cursor: Some("1".to_owned()),
            ..DecisionsAddedSinceRequest::default()
        },
    )?;
    assert_eq!(second.result_count, 1);
    assert!(!second.truncated);
    assert!(second.data.next_cursor.is_none());
    assert_eq!(second.data.added_decisions[0].decision_id, "decision-b");

    Ok(())
}

#[test]
fn added_since_filters_by_import_run_id_extracted_from_source_ref() -> Result<()> {
    use crate::events::EventSource;
    use crate::ledger::InMemoryEventLedger;

    let ledger = InMemoryEventLedger::new();
    let import_source_ref = serde_json::json!({
        "source": "document",
        "path": "/docs/notes.md",
        "sha256": "abc",
        "import_run_id": "import:2026-05-19T12:00:00Z:r1",
        "block_id": "blk-1",
        "source_span": {"start_byte": 0, "end_byte": 10},
        "source_snippet": "hi",
        "importer_actor_id": "actor:importer",
        "original_actor_id": null,
        "provisional_actor": true
    })
    .to_string();

    let mut e1 = event(
        1,
        EventType::DecisionProposed,
        "actor:doc",
        json!({
            "decision_id": "decision-doc-1",
            "title": "Doc decision",
            "rationale": "Imported",
            "topic_keys": ["docs"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    );
    e1.source = EventSource::Document;
    e1.source_ref = Some(import_source_ref.clone());
    ledger.append(e1)?;

    let mut e2 = event(
        2,
        EventType::DecisionProposed,
        "actor:planner",
        json!({
            "decision_id": "decision-other",
            "title": "Other",
            "rationale": "Plain",
            "topic_keys": ["other"],
            "option_ids": [],
            "chosen_option_id": null,
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
    );
    e2.source = EventSource::Api;
    e2.source_ref = Some("plain-ref".to_owned());
    ledger.append(e2)?;

    let response = get_decisions_added_since(
        &ledger,
        &DecisionsAddedSinceRequest {
            since_offset: Some(0),
            limit: 10,
            filters: DecisionsAddedSinceFilterRequest {
                import_run_ids: vec!["import:2026-05-19T12:00:00Z:r1".to_owned()],
                ..DecisionsAddedSinceFilterRequest::default()
            },
            ..DecisionsAddedSinceRequest::default()
        },
    )?;

    assert_eq!(response.data.total_added, 1);
    assert_eq!(
        response.data.added_decisions[0].decision_id,
        "decision-doc-1"
    );
    assert_eq!(
        response.data.added_decisions[0]
            .creation
            .import_run_id
            .as_deref(),
        Some("import:2026-05-19T12:00:00Z:r1")
    );
    Ok(())
}
