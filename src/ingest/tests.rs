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
