// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;

#[test]
fn extracts_mentioned_thread_without_writing_events() {
    let thread = parse_slack_thread_fixture(include_str!(
        "../../tests/fixtures/slack/thread_with_mention.json"
    ))
    .expect("fixture parses");

    let draft =
        extract_slack_decision_draft(&thread, DEFAULT_SLACK_MENTION).expect("thread extracts");

    assert_eq!(draft.actor_id, "slack:T123:U111");
    assert_eq!(draft.source_ref, "slack://T123/C456/1715970800.000100");
    assert_eq!(draft.title, "Use local fake Slack ingest first");
    assert_eq!(
        draft.rationale,
        "It verifies mention-gated capture without Slack credentials"
    );
    assert_eq!(draft.topic_keys, vec!["integrations", "slack"]);
    assert_eq!(
        draft.option_labels,
        vec!["Build local fixture ingest", "Wait for hosted Slack app"]
    );
    assert_eq!(
        draft.chosen_option_label.as_deref(),
        Some("Build local fixture ingest")
    );
    assert!(draft.thread_context.contains("Thread context is safe"));
    // The root message carries the mention and the Decision:/Chosen: markers, so the decided
    // message's own ts is the root's, and nobody asked before it: no ask.
    let root_ts = parse_slack_ts("1715970800.000100").expect("root ts parses");
    assert_eq!(draft.event_ts, root_ts);
    assert_eq!(draft.ask, None);
}

#[test]
fn rejects_thread_without_mention() {
    let thread = parse_slack_thread_fixture(include_str!(
        "../../tests/fixtures/slack/thread_without_mention.json"
    ))
    .expect("fixture parses");

    let error = extract_slack_decision_draft(&thread, DEFAULT_SLACK_MENTION)
        .expect_err("thread without mention rejected");

    assert!(error.to_string().contains("missing required mention"));
}

// ── Slack import: source time and explicit asks (hivemind-bbnw.6) ──────────

fn slack_fixture_draft(fixture: &str) -> SlackDecisionDraft {
    let thread = parse_slack_thread_fixture(fixture).expect("fixture parses");
    extract_slack_decision_draft(&thread, DEFAULT_SLACK_MENTION).expect("thread extracts")
}

fn events_of(ledger: &crate::ledger::InMemoryEventLedger, event_type: EventType) -> Vec<Event> {
    ledger
        .read(0, 100)
        .expect("read succeeds")
        .into_iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

/// Case 1 of the mayor's Q2 rule: a root that asks (ends in "?") and a later message that
/// decides. One `question.asked` at the root's own time, by the root's author; the decision
/// carries the chosen message's time and is linked to the ask's question.
#[test]
fn a_question_root_answered_later_writes_an_ask_at_the_roots_time() {
    let draft = slack_fixture_draft(include_str!(
        "../../tests/fixtures/slack/thread_asked_then_decided.json"
    ));
    let asked_ts = parse_slack_ts("1700000000.000000").expect("asked ts parses");
    let decided_ts = parse_slack_ts("1700003600.000000").expect("decided ts parses");
    assert_ne!(asked_ts, decided_ts);
    assert_eq!(
        draft.ask,
        Some(SlackAsk {
            actor_id: "slack:T123:U111".to_owned(),
            text: "@hivemind can we settle on a caching approach for the API?".to_owned(),
            ts: asked_ts,
        })
    );
    assert_eq!(draft.event_ts, decided_ts);

    let ledger = crate::ledger::InMemoryEventLedger::new();
    import_slack_thread(&ledger, &draft).expect("import succeeds");

    let asks = events_of(&ledger, EventType::QuestionAsked);
    assert_eq!(asks.len(), 1, "one ask for one asking root");
    assert_eq!(asks[0].ts, Some(asked_ts));
    assert_eq!(asks[0].actor_id, "slack:T123:U111");
    assert_eq!(
        asks[0].payload.get("text").and_then(|text| text.as_str()),
        Some("@hivemind can we settle on a caching approach for the API?")
    );

    let proposed = events_of(&ledger, EventType::DecisionProposed);
    assert_eq!(proposed.len(), 1);
    // The decision's own `ts` is the source's decided time, never "now" (the import time), and
    // the payload holds no ask time of its own: an ask has one home, `question.asked`.
    assert_eq!(proposed[0].ts, Some(decided_ts));
    assert!(proposed[0].payload.get("asked_at").is_none());
    // The decision answers the ask's own question node.
    let ask_question_id = asks[0]
        .payload
        .get("question_id")
        .expect("ask names a question");
    let answers: Vec<Event> = events_of(&ledger, EventType::RelationAdded)
        .into_iter()
        .filter(|event| event.payload.get("relation").and_then(|v| v.as_str()) == Some("ANSWERS"))
        .collect();
    assert_eq!(answers.len(), 1, "the decision answers the ask's question");
    assert_eq!(answers[0].payload.get("to_id"), Some(ask_question_id));
    // Everything the import writes after the ask carries the decided time, never "now".
    for event in ledger.read(0, 100).expect("read succeeds") {
        if event.event_type != EventType::QuestionAsked
            && event.event_type != EventType::QuestionRecorded
        {
            assert_eq!(event.ts, Some(decided_ts));
        }
    }
}

/// An ask has one home: an imported ask reads back where a `hivemind ask` does, so `why` shows
/// the root's own time as the decision's `asked_at` and the decision's own time as when it
/// happened.
#[test]
fn an_imported_ask_reads_back_as_the_decisions_asked_at() {
    let draft = slack_fixture_draft(include_str!(
        "../../tests/fixtures/slack/thread_asked_then_decided.json"
    ));
    let ledger = crate::ledger::InMemoryEventLedger::new();
    let outcome = import_slack_thread(&ledger, &draft).expect("import succeeds");

    let graph = crate::projector::memory::MemoryGraph::default();
    crate::projector::rebuild_graph_for_tenant(&ledger, &crate::events::TenantId::local(), &graph)
        .expect("ledger projects");
    let brief = crate::queries::get_decision_brief(&graph, outcome.decision_id())
        .expect("brief reads")
        .data
        .expect("decision has a brief");

    assert_eq!(
        brief.asked_at,
        Some(parse_slack_ts("1700000000.000000").expect("asked ts parses"))
    );
    assert_eq!(
        brief.question.as_deref(),
        Some("@hivemind can we settle on a caching approach for the API?")
    );
}

/// Case 2: a decision posted top-level. Root and decision are the one message; nobody asked
/// first, so the decision gets its own time and no ask (not a 0-second one).
#[test]
fn a_decision_posted_top_level_writes_no_ask() {
    let draft = slack_fixture_draft(include_str!(
        "../../tests/fixtures/slack/thread_decided_top_level.json"
    ));
    assert_eq!(draft.ask, None);
    let posted_ts = parse_slack_ts("1700100000.000000").expect("ts parses");
    assert_eq!(draft.event_ts, posted_ts);

    let ledger = crate::ledger::InMemoryEventLedger::new();
    import_slack_thread(&ledger, &draft).expect("import succeeds");

    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
    assert!(events_of(&ledger, EventType::QuestionRecorded).is_empty());
    let proposed = events_of(&ledger, EventType::DecisionProposed);
    assert_eq!(proposed[0].ts, Some(posted_ts));
}

/// Case 3: a thread whose root states the decision. Even a root that ends in "?" is no ask when
/// it is the message that carries the decision: the "different message" half of the rule.
#[test]
fn a_thread_whose_root_states_a_decision_writes_no_ask() {
    let draft = slack_fixture_draft(include_str!(
        "../../tests/fixtures/slack/thread_root_states_decision.json"
    ));
    assert_eq!(draft.ask, None);

    let ledger = crate::ledger::InMemoryEventLedger::new();
    import_slack_thread(&ledger, &draft).expect("import succeeds");

    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
    let proposed = events_of(&ledger, EventType::DecisionProposed);
    // The decided moment is the reply that chose, not the root that proposed.
    assert_eq!(
        proposed[0].ts,
        Some(parse_slack_ts("1700203600.000000").expect("ts parses"))
    );
}

#[test]
fn the_root_is_recognised_as_a_question_only_by_a_question_mark_or_an_ask_marker() {
    // Ends in "?": the whole message, trimmed, is the question.
    assert_eq!(
        root_question_text("  Which cache TTL do we use?\n").as_deref(),
        Some("Which cache TTL do we use?")
    );
    // Explicit markers, like Decision:/Chosen: — the words after the marker are the question.
    assert_eq!(
        root_question_text("Question: which cache TTL do we use").as_deref(),
        Some("which cache TTL do we use")
    );
    assert_eq!(
        root_question_text("fyi\nAsk: pick a cache TTL").as_deref(),
        Some("pick a cache TTL")
    );
    assert_eq!(
        root_question_text("decision needed: pick a cache TTL").as_deref(),
        Some("pick a cache TTL")
    );
    // No classifier guess: a statement, a topic mention and a bare "?" are none of them asks.
    assert_eq!(root_question_text("We should pick a cache TTL."), None);
    assert_eq!(root_question_text("@hivemind cache TTL thread"), None);
    assert_eq!(root_question_text("?"), None);
    // A question mark mid-message, or a marker with no value, is not enough.
    assert_eq!(root_question_text("Why? Because we said so."), None);
    assert_eq!(root_question_text("Question:"), None);
}

#[test]
fn a_marker_root_writes_an_ask_with_the_words_after_the_marker() {
    let draft = slack_fixture_draft(
        r#"{
          "team_id": "T123", "channel_id": "C456", "thread_ts": "1700300000.000000",
          "messages": [
            {"user_id": "U111", "ts": "1700300000.000000",
             "text": "@hivemind\nDecision needed: which cache TTL do we use"},
            {"user_id": "U222", "ts": "1700300600.000000",
             "text": "Decision: Use a five minute cache TTL\nRationale: Balances freshness against load\nOptions: 5m, 1h\nChosen: 5m"}
          ]
        }"#,
    );
    let ask = draft.ask.expect("a marker root is an ask");
    assert_eq!(ask.text, "which cache TTL do we use");
    assert_eq!(
        ask.ts,
        parse_slack_ts("1700300000.000000").expect("ts parses")
    );
}

#[test]
fn a_root_that_neither_asks_nor_decides_writes_no_ask() {
    let draft = slack_fixture_draft(
        r#"{
          "team_id": "T123", "channel_id": "C456", "thread_ts": "1700400000.000000",
          "messages": [
            {"user_id": "U111", "ts": "1700400000.000000",
             "text": "@hivemind let us settle the cache TTL today"},
            {"user_id": "U222", "ts": "1700400600.000000",
             "text": "Decision: Use a five minute cache TTL\nRationale: Balances freshness against load\nOptions: 5m, 1h\nChosen: 5m"}
          ]
        }"#,
    );
    // When in doubt, no ask: the root is not recognisably a question.
    assert_eq!(draft.ask, None);
}

#[test]
fn a_thread_whose_root_is_not_among_its_messages_writes_no_ask() {
    let draft = slack_fixture_draft(
        r#"{
          "team_id": "T123", "channel_id": "C456", "thread_ts": "1700500000.000000",
          "messages": [
            {"user_id": "U222", "ts": "1700500600.000000",
             "text": "@hivemind\nDecision: Use a five minute cache TTL\nRationale: Balances freshness against load\nOptions: 5m, 1h\nChosen: 5m"}
          ]
        }"#,
    );
    assert_eq!(draft.ask, None);
}

/// A bad `Chosen:` marker is refused before the first write: no ask is left waiting for a
/// decision that was never recorded.
#[test]
fn a_chosen_marker_matching_no_option_leaves_no_ask_behind() {
    let mut draft = slack_fixture_draft(include_str!(
        "../../tests/fixtures/slack/thread_asked_then_decided.json"
    ));
    draft.chosen_option_label = Some("Not an option".to_owned());
    let ledger = crate::ledger::InMemoryEventLedger::new();

    let error = import_slack_thread(&ledger, &draft).expect_err("mismatched chosen is refused");

    assert!(error.to_string().contains("Chosen marker must match"));
    assert!(ledger.read(0, 100).expect("read succeeds").is_empty());
}

// ── document import: source time preservation ───────────────────────────────

fn import_document_fixture(ledger: &crate::ledger::InMemoryEventLedger, path: &Path) {
    import_documents(
        ledger,
        &DocumentImportRequest {
            paths: vec![path.to_path_buf()],
            importer_actor_id: "actor:importer".to_owned(),
            format: DocumentImportFormat::Auto,
            conflict_resolution: DocumentConflictResolutionAction::Report,
        },
    )
    .expect("import succeeds");
}

fn find_decision_proposed(ledger: &crate::ledger::InMemoryEventLedger) -> Event {
    ledger
        .read(0, 100)
        .expect("read succeeds")
        .into_iter()
        .find(|event| event.event_type == EventType::DecisionProposed)
        .expect("decision.proposed written")
}

/// Bead hivemind-bbnw.6, "git trailers": a git-trailer-derived decision block is not a
/// separate importer — it is text pasted into the same `Decision:` block grammar the local
/// document importer already parses, and the commit's own time is named by the same `ts:`
/// marker any other document source uses.
#[test]
fn document_import_honors_a_git_trailer_styled_blocks_own_ts_marker() {
    let ledger = crate::ledger::InMemoryEventLedger::new();
    let path = Path::new("tests/fixtures/documents_source_time/git_trailer_style.md");

    import_document_fixture(&ledger, path);

    let proposed = find_decision_proposed(&ledger);
    let expected_ts = DateTime::parse_from_rfc3339("2026-04-02T09:15:00Z")
        .expect("fixture ts parses")
        .with_timezone(&Utc);
    assert_eq!(proposed.ts, Some(expected_ts));
    // Documents carry a decided date at best, never an ask (crew answer, hc-0ow point 6).
    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
}

/// Bead hivemind-bbnw.6, "tracker/beads": same rule, a tracker ticket's body pasted into a
/// decision block, its own `ts:` marker naming the ticket's resolved time.
#[test]
fn document_import_honors_a_tracker_ticket_styled_blocks_own_ts_marker() {
    let ledger = crate::ledger::InMemoryEventLedger::new();
    let path = Path::new("tests/fixtures/documents_source_time/tracker_ticket_style.md");

    import_document_fixture(&ledger, path);

    let proposed = find_decision_proposed(&ledger);
    let expected_ts = DateTime::parse_from_rfc3339("2026-04-05T16:30:00Z")
        .expect("fixture ts parses")
        .with_timezone(&Utc);
    assert_eq!(proposed.ts, Some(expected_ts));
    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
}

#[test]
fn document_import_falls_back_to_the_files_own_modified_time_without_a_ts_marker() {
    let dir = tempfile::tempdir().expect("tempdir creates");
    let path = dir.path().join("undated_decision.md");
    fs::write(
        &path,
        "Decision:\n  id: undated-decision\n  title: Use the file's own revision time\n  \
         status: accepted\n  actor: actor:alice\n  rationale: No ts marker names this one; \
         the file's own revision time is the decided date at best.\n  options:\n    - a\n    \
         - b\n  chose: a\n",
    )
    .expect("fixture writes");

    // Backdate the file so its mtime is unambiguously distinguishable from "now".
    let modified_at = DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
        .expect("literal parses")
        .with_timezone(&Utc);
    let file = fs::File::open(&path).expect("file opens");
    file.set_modified(std::time::SystemTime::from(modified_at))
        .expect("mtime sets");
    drop(file);

    let ledger = crate::ledger::InMemoryEventLedger::new();
    import_document_fixture(&ledger, &path);

    let proposed = find_decision_proposed(&ledger);
    // Whole-second comparison: common filesystems do not keep sub-second mtime precision,
    // and this is a best-effort fallback, not an exact marker.
    let recorded_ts = proposed.ts.expect("event carries a ts");
    assert_eq!(recorded_ts.timestamp(), modified_at.timestamp());
    assert!(events_of(&ledger, EventType::QuestionAsked).is_empty());
}

// ── conflict_decision_status ────────────────────────────────────────────────

fn minimal_draft(block_id: &str) -> DocumentDecisionDraft {
    DocumentDecisionDraft {
        block_id: block_id.to_owned(),
        title: "test title".to_owned(),
        status: ImportedDecisionStatus::Proposed,
        original_actor_id: None,
        topic_keys: vec![],
        rationale: "test rationale".to_owned(),
        option_labels: vec![],
        chosen_option_label: None,
        evidence: vec![],
        hypotheses: vec![],
        supersedes: vec![],
        span: DocumentSourceSpan {
            byte_start: 0,
            byte_end: 42,
            line_start: 1,
            line_end: 5,
        },
        snippet: "snippet".to_owned(),
        prepared_source_ref: None,
        extractor_explanation: None,
        decided_at: None,
    }
}

#[test]
fn conflict_decision_status_proposed_when_no_votes() {
    assert_eq!(
        conflict_decision_status(false, false, false),
        DocumentConflictDecisionStatus::Proposed,
    );
}

#[test]
fn conflict_decision_status_accepted() {
    assert_eq!(
        conflict_decision_status(true, false, false),
        DocumentConflictDecisionStatus::Accepted,
    );
}

#[test]
fn conflict_decision_status_rejected() {
    assert_eq!(
        conflict_decision_status(false, true, false),
        DocumentConflictDecisionStatus::Rejected,
    );
}

#[test]
fn conflict_decision_status_contested_when_both_voted() {
    assert_eq!(
        conflict_decision_status(true, true, false),
        DocumentConflictDecisionStatus::Contested,
    );
}

#[test]
fn conflict_decision_status_superseded_dominates_all_vote_combinations() {
    for accepted in [false, true] {
        for rejected in [false, true] {
            assert_eq!(
                conflict_decision_status(accepted, rejected, true),
                DocumentConflictDecisionStatus::Superseded,
                "superseded must dominate (accepted={accepted}, rejected={rejected})"
            );
        }
    }
}

// ── conflict_resolution_uuid ────────────────────────────────────────────────

#[test]
fn conflict_resolution_uuid_is_deterministic() {
    let draft = minimal_draft("determinism-block");
    let a = conflict_resolution_uuid(
        DocumentConflictResolutionAction::Contest,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "decision.rejected",
        0,
    );
    let b = conflict_resolution_uuid(
        DocumentConflictResolutionAction::Contest,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "decision.rejected",
        0,
    );
    assert_eq!(a, b, "same inputs must produce the same UUID");
}

#[test]
fn conflict_resolution_uuid_differs_by_action() {
    let draft = minimal_draft("action-block");
    let contest = conflict_resolution_uuid(
        DocumentConflictResolutionAction::Contest,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "decision.rejected",
        0,
    );
    let add_ctx = conflict_resolution_uuid(
        DocumentConflictResolutionAction::AddContext,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "decision.rejected",
        0,
    );
    assert_ne!(contest, add_ctx);
}

#[test]
fn conflict_resolution_uuid_differs_by_role() {
    let draft = minimal_draft("role-block");
    let rejected = conflict_resolution_uuid(
        DocumentConflictResolutionAction::AddContext,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "decision.rejected",
        1,
    );
    let recorded = conflict_resolution_uuid(
        DocumentConflictResolutionAction::AddContext,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "evidence.recorded",
        1,
    );
    assert_ne!(rejected, recorded);
}

#[test]
fn conflict_resolution_uuid_differs_by_index() {
    let draft = minimal_draft("index-block");
    let idx1 = conflict_resolution_uuid(
        DocumentConflictResolutionAction::AddContext,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "evidence.recorded",
        1,
    );
    let idx2 = conflict_resolution_uuid(
        DocumentConflictResolutionAction::AddContext,
        "docs/adr.md",
        "cafebabe1234",
        &draft,
        "evidence.recorded",
        2,
    );
    assert_ne!(idx1, idx2);
}

// ── stable_component ────────────────────────────────────────────────────────

#[test]
fn stable_component_lowercases_and_hyphenates() {
    assert_eq!(stable_component("Hello World"), "hello-world");
}

#[test]
fn stable_component_strips_trailing_hyphen() {
    assert_eq!(stable_component("trailing---"), "trailing");
}

#[test]
fn stable_component_collapses_separator_runs() {
    assert_eq!(stable_component("a  b  c"), "a-b-c");
}

#[test]
fn stable_component_fallback_for_no_alphanumeric() {
    let result = stable_component("---");
    assert_eq!(result.len(), 12, "fallback must be 12-char hex");
    assert!(result.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn stable_component_truncates_at_80_chars() {
    let long = "a".repeat(200);
    let result = stable_component(&long);
    assert!(result.len() <= 80);
}

// ── stable_decision_id ──────────────────────────────────────────────────────

#[test]
fn stable_decision_id_produces_namespaced_id() {
    assert_eq!(
        stable_decision_id("my-ns", "use postgres"),
        "decision:document:my-ns:use-postgres"
    );
}

// ── DocumentImportIdentities::new_conflict_supersession ─────────────────────

#[test]
fn new_conflict_supersession_includes_existing_id_in_supersedes() {
    let draft = minimal_draft("adr-block");
    let existing = "decision:document:proj:existing-decision";
    let identities = DocumentImportIdentities::new_conflict_supersession(
        &draft,
        "docs/adr.md",
        "abcdef012345",
        "proj",
        existing,
    )
    .expect("identities must be created");
    assert!(
        identities
            .supersedes_decision_ids
            .contains(&existing.to_owned()),
        "supersedes_decision_ids must include the existing decision id"
    );
}

#[test]
fn new_conflict_supersession_produces_different_decision_id_than_base() {
    let draft = minimal_draft("adr-block");
    let base = DocumentImportIdentities::new(&draft, "docs/adr.md", "abcdef012345", "proj")
        .expect("base identities");
    let supersession = DocumentImportIdentities::new_conflict_supersession(
        &draft,
        "docs/adr.md",
        "abcdef012345",
        "proj",
        "decision:document:proj:existing-decision",
    )
    .expect("supersession identities");
    assert_ne!(
        base.decision_id, supersession.decision_id,
        "conflict supersession must use a distinct identity block"
    );
}

#[test]
fn new_conflict_supersession_decision_id_is_deterministic() {
    let draft = minimal_draft("adr-block");
    let id1 = DocumentImportIdentities::new_conflict_supersession(
        &draft,
        "docs/adr.md",
        "abcdef012345",
        "proj",
        "decision:document:proj:existing-decision",
    )
    .expect("identities")
    .decision_id;
    let id2 = DocumentImportIdentities::new_conflict_supersession(
        &draft,
        "docs/adr.md",
        "abcdef012345",
        "proj",
        "decision:document:proj:existing-decision",
    )
    .expect("identities")
    .decision_id;
    assert_eq!(id1, id2, "same inputs must produce the same decision id");
}
