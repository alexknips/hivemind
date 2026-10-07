// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;

use std::cell::Cell;
use std::collections::HashSet;

use chrono::TimeZone;

use crate::events::Event;
use crate::ledger::InMemoryEventLedger;

fn batch_event(turns: serde_json::Value) -> crate::events::Event {
    crate::events::Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: uuid::Uuid::nil(),
        correlation_id: None,
        causation_event_id: None,
        event_type: crate::events::EventType::IngestBatchReceived,
        actor_id: "actor:test".to_owned(),
        source: Default::default(),
        source_ref: None,
        payload: serde_json::json!({ "batch_id": "b1", "turns": turns }),
        ts: None,
    }
}

// --- render_batch_text ---

#[test]
fn render_batch_text_no_turns_is_empty() {
    let event = batch_event(serde_json::json!([]));
    assert_eq!(render_batch_text(&event), "");
}

#[test]
fn render_batch_text_missing_turns_key_is_empty() {
    let event = crate::events::Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: uuid::Uuid::nil(),
        correlation_id: None,
        causation_event_id: None,
        event_type: crate::events::EventType::IngestBatchReceived,
        actor_id: "actor:test".to_owned(),
        source: Default::default(),
        source_ref: None,
        payload: serde_json::json!({ "batch_id": "b1" }),
        ts: None,
    };
    assert_eq!(render_batch_text(&event), "");
}

#[test]
fn render_batch_text_single_turn() {
    let event = batch_event(serde_json::json!([
        { "role": "user", "text": "hello", "truncated": false }
    ]));
    assert_eq!(render_batch_text(&event), "[user] hello\n");
}

#[test]
fn render_batch_text_truncated_appends_marker() {
    let event = batch_event(serde_json::json!([
        { "role": "assistant", "text": "partial response", "truncated": true }
    ]));
    assert_eq!(
        render_batch_text(&event),
        "[assistant] partial response [TRUNCATED]\n"
    );
}

#[test]
fn render_batch_text_multiple_turns_preserves_order() {
    let event = batch_event(serde_json::json!([
        { "role": "user", "text": "first", "truncated": false },
        { "role": "assistant", "text": "second", "truncated": false },
        { "role": "user", "text": "third", "truncated": false },
    ]));
    assert_eq!(
        render_batch_text(&event),
        "[user] first\n[assistant] second\n[user] third\n"
    );
}

#[test]
fn render_batch_text_names_each_turn_by_its_id() {
    let event = batch_event(serde_json::json!([
        { "turn_id": "t-1", "role": "assistant", "text": "which database?", "truncated": false },
        { "turn_id": "t-2", "role": "user", "text": "partial", "truncated": true },
    ]));
    assert_eq!(
        render_batch_text(&event),
        "[assistant turn t-1] which database?\n[user turn t-2] partial [TRUNCATED]\n"
    );
    assert_eq!(turn_ids(&event), vec!["t-1", "t-2"]);
}

#[test]
fn render_batch_text_unknown_role_defaults_to_unknown() {
    let event = batch_event(serde_json::json!([
        { "text": "no role field" }
    ]));
    assert_eq!(render_batch_text(&event), "[unknown] no role field\n");
}

// --- pending-batch filter ---

#[test]
fn pending_batch_filter_excludes_classified_ids() {
    let received = vec![
        PendingBatch {
            batch_id: "a".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 1,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
        PendingBatch {
            batch_id: "b".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 2,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
        PendingBatch {
            batch_id: "c".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 3,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
    ];
    let mut classified: HashSet<String> = HashSet::new();
    classified.insert("b".to_owned());

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].batch_id, "a");
    assert_eq!(pending[1].batch_id, "c");
}

#[test]
fn pending_batch_filter_all_classified_yields_empty() {
    let received = vec![PendingBatch {
        batch_id: "x".to_owned(),
        submitted_at: None,
        actor_id: "actor:test".to_owned(),
        turn_count: 0,
        session_id: String::new(),
        agent_tool: String::new(),
        batch_text: String::new(),
    }];
    let mut classified: HashSet<String> = HashSet::new();
    classified.insert("x".to_owned());

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert!(pending.is_empty());
}

// --- classifier model override ---

#[test]
fn resolve_classifier_model_defaults_when_no_override() {
    assert_eq!(resolve_classifier_model(None), CLASSIFIER_MODEL);
}

#[test]
fn resolve_classifier_model_uses_override_when_present() {
    assert_eq!(
        resolve_classifier_model(Some("claude-sonnet-5".to_owned())),
        "claude-sonnet-5"
    );
}

#[test]
fn pending_batch_filter_no_classified_returns_all() {
    let received = vec![
        PendingBatch {
            batch_id: "p".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 5,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
        PendingBatch {
            batch_id: "q".to_owned(),
            submitted_at: None,
            actor_id: "actor:test".to_owned(),
            turn_count: 5,
            session_id: String::new(),
            agent_tool: String::new(),
            batch_text: String::new(),
        },
    ];
    let classified: HashSet<String> = HashSet::new();

    let pending: Vec<_> = received
        .into_iter()
        .filter(|b| !classified.contains(&b.batch_id))
        .collect();

    assert_eq!(pending.len(), 2);
}

// --- session_agent_actor ---

#[test]
fn session_agent_actor_prefers_already_agent_shaped_token() {
    // A token minted through the agent-token path (hivemind-zdsh.19) is already
    // the stable agent identity; use it as-is instead of synthesizing another one.
    assert_eq!(
        session_agent_actor("agent:gastown:crew", "claude"),
        Some("agent:gastown:crew".to_owned())
    );
}

#[test]
fn session_agent_actor_falls_back_to_tool_for_human_token() {
    // A human's own personal token still runs an agent tool on their behalf;
    // that tool needs an identity distinct from the human (hivemind-zdsh.19).
    assert_eq!(
        session_agent_actor("human:alex@example.com", "claude"),
        Some("agent:claude:hook".to_owned())
    );
}

#[test]
fn session_agent_actor_is_stable_across_different_session_ids() {
    // Same physical agent, two different (synthetic) runs -- the identity must
    // not depend on session_id at all, or the same agent looks like a different
    // actor after every restart (hivemind-zdsh.9).
    let first = session_agent_actor("human:alex@example.com", "codex");
    let second = session_agent_actor("human:alex@example.com", "codex");
    assert_eq!(first, second);
}

#[test]
fn session_agent_actor_none_when_no_tool_and_not_agent_token() {
    assert_eq!(session_agent_actor("human:alex@example.com", ""), None);
}

// --- source turn and question (hivemind-bbnw.8) ---

fn parsed_capture(extra: serde_json::Value) -> CaptureItem {
    let mut capture = serde_json::json!({
        "kind": "decision",
        "title": "Use Postgres for the hosted MVP",
        "rationale": "Several friends write at once",
        "topic_keys": ["hosted-mvp"],
        "evidence_ids": [],
        "options": null,
        "chosen_option": null,
        "extraction_confidence": 0.9,
        "expressed_confidence": null,
        "supersedes_id": null,
        "actor_id": null,
        "blocked_actor_id": null,
        "decision_id": null,
    });
    capture
        .as_object_mut()
        .expect("capture is an object")
        .extend(extra.as_object().expect("extra is an object").clone());
    let response = serde_json::json!({ "captures": [capture] }).to_string();
    parse_capture_response(&response)
        .expect("response parses")
        .remove(0)
}

#[test]
fn the_schema_asks_for_a_source_turn_and_a_question_on_every_capture() {
    let schema = capture_schema();
    let items = &schema["properties"]["captures"]["items"];
    for field in ["source_turn_id", "question"] {
        assert!(
            items["properties"].get(field).is_some(),
            "{field} is a property"
        );
        assert!(
            items["required"]
                .as_array()
                .expect("required list")
                .iter()
                .any(|name| name == field),
            "{field} is required (nullable) under structured outputs"
        );
    }
}

#[test]
fn a_response_without_the_new_fields_still_parses() {
    let capture = parsed_capture(serde_json::json!({}));
    assert_eq!(capture.source_turn_id, None);
    assert_eq!(capture.question, None);
}

#[test]
fn the_evaluator_path_keeps_the_question_but_no_turn() {
    // No batch, so no turn the model could honestly name.
    let capture = parsed_capture(serde_json::json!({
        "source_turn_id": "t-1",
        "question": "Which database should the hosted MVP use?"
    }));
    assert_eq!(capture.source_turn_id, None);
    assert_eq!(
        capture.question.as_deref(),
        Some("Which database should the hosted MVP use?")
    );
}

#[test]
fn a_turn_the_batch_does_not_hold_is_dropped_before_submission() {
    let mut named = parsed_capture(serde_json::json!({}));
    named.source_turn_id = Some("t-2".to_owned());
    let mut invented = parsed_capture(serde_json::json!({}));
    invented.source_turn_id = Some("t-99".to_owned());
    let mut captures = vec![named, invented];

    conform_to_batch(&mut captures, &["t-1".to_owned(), "t-2".to_owned()]);

    assert_eq!(captures[0].source_turn_id.as_deref(), Some("t-2"));
    assert_eq!(captures[1].source_turn_id, None);
}

#[test]
fn a_question_the_write_path_would_refuse_is_dropped_before_submission() {
    let mut kept = parsed_capture(serde_json::json!({}));
    kept.question = Some("Which database should the hosted MVP use?".to_owned());
    let mut request = parsed_capture(serde_json::json!({}));
    request.kind = "decision-request".to_owned();
    request.question = Some("Which database?".to_owned());
    let mut wordless = parsed_capture(serde_json::json!({}));
    wordless.question = Some(" ? ".to_owned());
    let mut on_evidence = parsed_capture(serde_json::json!({}));
    on_evidence.kind = "evidence".to_owned();
    on_evidence.question = Some("Which database?".to_owned());
    let mut captures = vec![kept, request, wordless, on_evidence];

    conform_to_batch(&mut captures, &[]);

    assert!(captures[0].question.is_some());
    assert!(captures[1].question.is_some());
    assert_eq!(captures[2].question, None);
    assert_eq!(captures[3].question, None);
}

// --- who decided (hivemind-u70b) ---

/// The words the two classify workers share about who decided: the server-side prompt and the
/// agent-seat command (Worker A), which is read from the plugin so a drift in either shows here.
const DECIDER_RULE: &[&str] = &[
    "a person stating the choice themselves",
    "list priya, not whoever reports it",
    "a proposal nobody has accepted stays empty however firmly it is worded",
    "an agent narrating or carrying out its own choice is proposing it",
];

fn one_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[test]
fn both_classify_workers_carry_the_same_rule_for_who_decided() {
    let command = include_str!("../../plugins/hivemind-capture/commands/classify-queue.md");
    for (worker, text) in [
        ("the server-side prompt", one_line(CLASSIFIER_PROMPT)),
        ("the classify-queue command", one_line(command)),
    ] {
        for phrase in DECIDER_RULE {
            assert!(text.contains(phrase), "{worker} lost {phrase:?}");
        }
    }
}

// --- the pending queue's reads (hivemind-t15t) ---

fn queue_event(event_type: EventType, payload: serde_json::Value, second: u32) -> Event {
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: uuid::Uuid::new_v4(),
        correlation_id: None,
        causation_event_id: None,
        event_type,
        actor_id: "agent:test:queue".to_owned(),
        source: Default::default(),
        source_ref: None,
        payload,
        ts: chrono::Utc
            .with_ymd_and_hms(2026, 10, 1, 12, second / 60, second % 60)
            .single(),
    }
}

fn append_received(
    ledger: &impl EventLedger,
    batch_id: &str,
    session_id: &str,
    actor_id: &str,
    second: u32,
) {
    let mut event = queue_event(
        EventType::IngestBatchReceived,
        serde_json::json!({
            "batch_id": batch_id,
            "session_id": session_id,
            "agent_tool": "claude",
            "turns": [{ "turn_id": "t1", "role": "user", "text": format!("text of {batch_id}"),
                        "truncated": false }],
        }),
        second,
    );
    event.actor_id = actor_id.to_owned();
    ledger
        .append_for_tenant(&TenantId::local(), event)
        .expect("append received batch");
}

fn append_classified(ledger: &impl EventLedger, batch_ids: &[String], second: u32) {
    let event = queue_event(
        EventType::IngestBatchClassified,
        serde_json::json!({
            "batch_id": batch_ids.first(),
            "batch_ids": batch_ids,
            "captures": [{ "kind": "decision", "title": "a capture the queue never reads" }],
        }),
        second,
    );
    ledger
        .append_for_tenant(&TenantId::local(), event)
        .expect("append classification");
}

/// A ledger that counts what the queue's reads cost: how many reads, and how many events came
/// back still carrying their turns.
struct CountingLedger {
    inner: InMemoryEventLedger,
    reads: Cell<usize>,
    bodies_read: Cell<usize>,
}

impl CountingLedger {
    fn new() -> Self {
        Self {
            inner: InMemoryEventLedger::new(),
            reads: Cell::new(0),
            bodies_read: Cell::new(0),
        }
    }

    fn note(&self, events: &[Event]) {
        self.reads.set(self.reads.get() + 1);
        self.bodies_read.set(
            self.bodies_read.get()
                + events
                    .iter()
                    .filter(|event| event.payload.get("turns").is_some())
                    .count(),
        );
    }

    fn reset(&self) {
        self.reads.set(0);
        self.bodies_read.set(0);
    }
}

impl EventLedger for CountingLedger {
    fn append_for_tenant(&self, tenant_id: &TenantId, event: Event) -> crate::Result<EventId> {
        self.inner.append_for_tenant(tenant_id, event)
    }

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> crate::Result<Vec<Event>> {
        let events = self.inner.read_for_tenant(tenant_id, offset, limit)?;
        self.note(&events);
        Ok(events)
    }

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> crate::Result<()>,
    ) -> crate::Result<()> {
        self.inner
            .replay_from_for_tenant(tenant_id, offset, callback)
    }

    fn latest_offset_for_tenant(&self, tenant_id: &TenantId) -> crate::Result<EventId> {
        self.inner.latest_offset_for_tenant(tenant_id)
    }

    fn read_types_for_tenant(
        &self,
        tenant_id: &TenantId,
        types: &[EventType],
        omit_payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> crate::Result<Vec<Event>> {
        let events =
            self.inner
                .read_types_for_tenant(tenant_id, types, omit_payload_keys, offset, limit)?;
        self.note(&events);
        Ok(events)
    }

    fn read_fields_for_tenant(
        &self,
        tenant_id: &TenantId,
        event_type: EventType,
        payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> crate::Result<Vec<crate::ledger::EventFields>> {
        self.reads.set(self.reads.get() + 1);
        self.inner
            .read_fields_for_tenant(tenant_id, event_type, payload_keys, offset, limit)
    }

    fn read_ids_for_tenant(
        &self,
        tenant_id: &TenantId,
        event_ids: &[EventId],
    ) -> crate::Result<Vec<Event>> {
        let events = self.inner.read_ids_for_tenant(tenant_id, event_ids)?;
        self.note(&events);
        Ok(events)
    }
}

/// `received` batches over 20 sessions, the first `classified` of them covered by classifications
/// of 100 batches each: `received - classified` stay pending.
fn queue_with_history(received: usize, classified: usize) -> CountingLedger {
    let ledger = CountingLedger::new();
    let ids: Vec<String> = (0..received).map(|n| format!("batch-{n}")).collect();
    for (n, batch_id) in ids.iter().enumerate() {
        append_received(
            &ledger,
            batch_id,
            &format!("session-{}", n % 20),
            "agent:test:queue",
            u32::try_from(n % 3000).unwrap_or(0),
        );
    }
    for covered in ids
        .iter()
        .take(classified)
        .cloned()
        .collect::<Vec<_>>()
        .chunks(100)
    {
        append_classified(&ledger, covered, 0);
    }
    ledger
}

#[test]
fn listing_the_pending_queue_never_reads_the_text_of_classified_history() {
    let measure = |received: usize, classified: usize| {
        let ledger = queue_with_history(received, classified);
        ledger.reset();
        let page = list_pending_batches_for_ledger(&ledger, &TenantId::local(), None, 10)
            .expect("list pending");
        (
            page.pending_total,
            page.batches.len(),
            ledger.reads.get(),
            ledger.bodies_read.get(),
        )
    };

    // Ten times the history, the same cost: three reads (classifications, received heads, the
    // page's bodies), and the turns of only the ten batches handed back.
    assert_eq!(measure(300, 250), (50, 10, 3, 10));
    assert_eq!(measure(3000, 2950), (50, 10, 3, 10));
}

#[test]
fn a_queue_longer_than_one_read_page_is_read_whole() {
    let ledger = InMemoryEventLedger::new();
    let received = QUEUE_READ_PAGE + 50;
    for n in 0..received {
        append_received(
            &ledger,
            &format!("batch-{n}"),
            "session-x",
            "agent:test:queue",
            0,
        );
    }
    // One classification per batch: more classifications than one read page, too.
    for n in 0..(received - 30) {
        append_classified(&ledger, &[format!("batch-{n}")], 0);
    }

    let page = list_pending_batches_for_ledger(&ledger, &TenantId::local(), None, 100)
        .expect("list pending");

    assert_eq!(page.pending_total, 30);
    assert!(!page.truncated);
    let ids: Vec<&str> = page.batches.iter().map(|b| b.batch_id.as_str()).collect();
    let expected: Vec<String> = ((received - 30)..received)
        .map(|n| format!("batch-{n}"))
        .collect();
    assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn listing_pending_batches_filters_by_session_and_says_when_it_stopped_short() {
    let ledger = InMemoryEventLedger::new();
    append_received(&ledger, "a1", "session-a", "agent:test:queue", 1);
    append_received(&ledger, "b1", "session-b", "agent:test:queue", 2);
    append_received(&ledger, "a2", "session-a", "agent:test:queue", 3);
    append_received(&ledger, "a3", "session-a", "agent:test:queue", 4);
    append_classified(&ledger, &["a2".to_owned()], 5);

    let everything =
        list_pending_batches_for_ledger(&ledger, &TenantId::local(), None, 20).expect("list");
    let ids: Vec<&str> = everything
        .batches
        .iter()
        .map(|b| b.batch_id.as_str())
        .collect();
    assert_eq!(ids, ["a1", "b1", "a3"]);
    assert_eq!(everything.pending_total, 3);
    assert!(!everything.truncated);
    assert!(everything.batches[0].batch_text.contains("text of a1"));
    assert_eq!(everything.batches[0].turn_count, 1);
    assert_eq!(everything.batches[0].session_id, "session-a");
    assert_eq!(everything.batches[0].agent_tool, "claude");

    let session_a =
        list_pending_batches_for_ledger(&ledger, &TenantId::local(), Some("session-a"), 1)
            .expect("list session a");
    let ids: Vec<&str> = session_a
        .batches
        .iter()
        .map(|b| b.batch_id.as_str())
        .collect();
    assert_eq!(ids, ["a1"], "the oldest of the session comes first");
    assert_eq!(
        session_a.pending_total, 2,
        "the total counts the session's batches, not the limit"
    );
    assert!(session_a.truncated);

    let none = list_pending_batches_for_ledger(&ledger, &TenantId::local(), Some("session-z"), 20)
        .expect("list unknown session");
    assert!(none.batches.is_empty());
    assert_eq!(none.pending_total, 0);
    assert!(!none.truncated);
}

#[test]
fn pending_sessions_are_summarised_newest_first_without_turn_text() {
    let ledger = InMemoryEventLedger::new();
    append_received(&ledger, "a1", "session-a", "agent:test:one", 10);
    append_received(&ledger, "a2", "session-a", "agent:test:one", 30);
    append_received(&ledger, "a3", "session-a", "agent:test:one", 20);
    append_received(&ledger, "b1", "session-b", "agent:test:two", 40);
    append_received(&ledger, "c1", "session-c", "agent:test:three", 5);
    append_received(&ledger, "c2", "session-c", "agent:test:three", 6);
    append_received(&ledger, "d1", "session-d", "agent:test:four", 50);
    // session-c has one batch left; session-d has none.
    append_classified(&ledger, &["c1".to_owned(), "d1".to_owned()], 60);

    let page = list_pending_sessions_for_ledger(&ledger, &TenantId::local(), 10).expect("sessions");

    assert_eq!(page.batch_total, 5);
    assert_eq!(page.session_total, 3);
    assert!(!page.truncated);
    let summary: Vec<(&str, &str, usize)> = page
        .sessions
        .iter()
        .map(|s| (s.session_id.as_str(), s.actor_id.as_str(), s.batch_count))
        .collect();
    assert_eq!(
        summary,
        [
            ("session-b", "agent:test:two", 1),
            ("session-a", "agent:test:one", 3),
            ("session-c", "agent:test:three", 1),
        ]
    );
    let session_a = &page.sessions[1];
    assert_eq!(
        session_a.oldest_submitted_at,
        queue_event(EventType::IngestBatchReceived, serde_json::json!({}), 10).ts
    );
    assert_eq!(
        session_a.newest_submitted_at,
        queue_event(EventType::IngestBatchReceived, serde_json::json!({}), 30).ts
    );

    let json = serde_json::to_value(&page).expect("page serialises");
    assert!(
        !json.to_string().contains("text of"),
        "a session summary carries no turn text: {json}"
    );

    let first_only =
        list_pending_sessions_for_ledger(&ledger, &TenantId::local(), 1).expect("limit 1");
    assert_eq!(first_only.sessions.len(), 1);
    assert_eq!(first_only.session_total, 3);
    assert!(first_only.truncated);
}

#[test]
fn the_daily_budget_counts_only_todays_classifications() {
    let ledger = InMemoryEventLedger::new();
    let mut yesterday = queue_event(
        EventType::IngestBatchClassified,
        serde_json::json!({ "batch_ids": ["old"], "captures": [] }),
        0,
    );
    yesterday.ts = Some(chrono::Utc::now() - chrono::Duration::days(1));
    ledger
        .append_for_tenant(&TenantId::local(), yesterday)
        .expect("append yesterday's classification");
    let mut today = queue_event(
        EventType::IngestBatchClassified,
        serde_json::json!({ "batch_ids": ["new"], "captures": [] }),
        0,
    );
    today.ts = Some(chrono::Utc::now());
    ledger
        .append_for_tenant(&TenantId::local(), today)
        .expect("append today's classification");
    append_received(&ledger, "unrelated", "session-a", "agent:test:queue", 0);

    let status = daily_cap_status(&ledger, &TenantId::local()).expect("budget");

    assert_eq!(status.classified_today, 1);
}
