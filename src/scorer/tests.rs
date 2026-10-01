// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::commands::{DecisionProposalInput, Grounding};
use crate::events::{CaptureItem, ModelDimension, QualityLevel};
use crate::ledger::InMemoryEventLedger;

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hivemind-scorer-test-{name}-{}",
        uuid::Uuid::new_v4()
    ))
}

// --- scorer model override ---

#[test]
fn resolve_scorer_model_defaults_when_no_override() {
    assert_eq!(resolve_scorer_model(None), SCORER_MODEL);
}

#[test]
fn resolve_scorer_model_uses_override_when_present() {
    assert_eq!(
        resolve_scorer_model(Some("claude-sonnet-5".to_owned())),
        "claude-sonnet-5"
    );
}

// --- render_decision_text ---

#[test]
fn render_decision_text_empty_capture_yields_empty_string() {
    assert_eq!(render_decision_text(&serde_json::json!({})), "");
}

#[test]
fn render_decision_text_all_fields() {
    let capture = serde_json::json!({
        "title": "Use SQLite",
        "rationale": "Simple and embeddable",
        "options": ["SQLite", "PostgreSQL"],
        "chosen_option": "SQLite",
        "expressed_confidence": "high",
        "topic_keys": ["storage", "database"]
    });
    let text = render_decision_text(&capture);
    assert!(text.contains("Title: Use SQLite\n"));
    assert!(text.contains("Rationale: Simple and embeddable\n"));
    assert!(text.contains("Options considered: SQLite, PostgreSQL\n"));
    assert!(text.contains("Chosen option: SQLite\n"));
    assert!(text.contains("Expressed confidence: high\n"));
    assert!(text.contains("Topic keys: storage, database\n"));
}

#[test]
fn render_decision_text_omits_absent_optional_fields() {
    let capture = serde_json::json!({
        "title": "Use caching",
        "rationale": "Reduces latency"
    });
    let text = render_decision_text(&capture);
    assert!(text.contains("Title: Use caching\n"));
    assert!(text.contains("Rationale: Reduces latency\n"));
    assert!(!text.contains("Options considered"));
    assert!(!text.contains("Chosen option"));
    assert!(!text.contains("Expressed confidence"));
    assert!(!text.contains("Topic keys"));
}

#[test]
fn render_decision_text_empty_options_array_omitted() {
    let capture = serde_json::json!({
        "title": "Deploy now",
        "options": []
    });
    let text = render_decision_text(&capture);
    assert!(!text.contains("Options considered"));
}

// --- render_proposed_decision_text ---

#[test]
fn render_proposed_decision_text_empty_payload_yields_empty_string() {
    assert_eq!(render_proposed_decision_text(&serde_json::json!({})), "");
}

#[test]
fn render_proposed_decision_text_all_fields() {
    let payload = serde_json::json!({
        "title": "Pick queue",
        "question": "Which queue should we use?",
        "quote": "Use the async queue",
        "rationale": "Durable ingestion under load",
        "option_labels": ["sync", "async"],
        "option_descriptions": ["Blocking", "Non-blocking"]
    });
    let text = render_proposed_decision_text(&payload);
    assert!(text.contains("Title: Pick queue\n"));
    assert!(text.contains("Question: Which queue should we use?\n"));
    assert!(text.contains("Quote: Use the async queue\n"));
    assert!(text.contains("Rationale: Durable ingestion under load\n"));
    assert!(text.contains("Option: sync\n"));
    assert!(text.contains("Option description: Blocking\n"));
    assert!(text.contains("Option: async\n"));
    assert!(text.contains("Option description: Non-blocking\n"));
}

#[test]
fn render_proposed_decision_text_omits_absent_optional_fields() {
    let payload = serde_json::json!({
        "title": "Pick queue",
        "rationale": "Durable ingestion under load"
    });
    let text = render_proposed_decision_text(&payload);
    assert!(text.contains("Title: Pick queue\n"));
    assert!(text.contains("Rationale: Durable ingestion under load\n"));
    assert!(!text.contains("Question"));
    assert!(!text.contains("Quote"));
    assert!(!text.contains("Option"));
}

#[test]
fn render_proposed_decision_text_option_without_description_omits_description_line() {
    let payload = serde_json::json!({
        "title": "Pick queue",
        "option_labels": ["sync"]
    });
    let text = render_proposed_decision_text(&payload);
    assert!(text.contains("Option: sync\n"));
    assert!(!text.contains("Option description"));
}

// --- build_assessed_payload ---

fn all_not_assessed_dimensions_json() -> serde_json::Value {
    serde_json::json!({
        "framing": {"status": "not_assessed", "reason": "n/a"},
        "alternatives": {"status": "not_assessed", "reason": "n/a"},
        "information": {"status": "not_assessed", "reason": "n/a"},
        "reasoning": {"status": "not_assessed", "reason": "n/a"},
        "values_tradeoffs": {"status": "not_assessed", "reason": "n/a"},
        "bias_exposure": {"status": "not_assessed", "reason": "n/a"},
        "calibration": {"status": "not_assessed", "reason": "n/a"}
    })
}

/// A stub model answer that needs no quote at all (every dimension is either `not_assessed` or
/// `assessed` at level `none`), so it can be written against any decision's recorded text
/// without the write path's substring check ever having anything to check.
fn stub_assessment_no_quotes_needed() -> AssessmentOutput {
    let mut dims = all_not_assessed_dimensions_json();
    dims["framing"] = serde_json::json!({"status": "assessed", "level": "none", "explanation": "no question recorded"});
    dims["values_tradeoffs"] = serde_json::json!({"status": "assessed", "level": "none", "explanation": "no tradeoff named"});
    serde_json::from_value(serde_json::json!({ "dimensions": dims }))
        .expect("valid assessment output")
}

#[test]
fn build_assessed_payload_sets_schema_version_and_ids() {
    let mut dims = all_not_assessed_dimensions_json();
    dims["framing"] = serde_json::json!({
        "status": "assessed", "level": "solid", "explanation": "e", "quote": "Use REST"
    });
    let output: AssessmentOutput =
        serde_json::from_value(serde_json::json!({ "dimensions": dims })).expect("valid output");

    let payload = build_assessed_payload(
        "decision-abc",
        "claude-haiku-4-5-20251001",
        "assessment-v1",
        output,
    );

    assert_eq!(payload.schema_version, DECISION_ASSESSED_SCHEMA_VERSION);
    assert_eq!(payload.decision_id, "decision-abc");
    assert_eq!(payload.model, "claude-haiku-4-5-20251001");
    assert_eq!(payload.prompt_version, "assessment-v1");
    assert!(payload.supersedes_score_id.is_none());
    assert!(payload.importance.is_none());
    assert!(matches!(
        payload.dimensions.framing,
        ModelDimension::Assessed {
            level: QualityLevel::Solid,
            ..
        }
    ));
}

#[test]
fn build_assessed_payload_carries_optional_importance_when_given() {
    let output: AssessmentOutput = serde_json::from_value(serde_json::json!({
        "dimensions": all_not_assessed_dimensions_json(),
        "importance": {
            "stakes": 5.0, "stakes_explanation": "e",
            "irreversibility": 0.5, "irreversibility_explanation": "e",
            "actionability": 0.5, "actionability_explanation": "e"
        }
    }))
    .expect("valid output");

    let payload = build_assessed_payload("decision-x", "model", "v1", output);

    assert_eq!(payload.importance.expect("importance present").stakes, 5.0);
}

// --- find_unscored_decisions (also covers hivemind-qo11.9's acceptance: a directly captured
// decision, not just a classifier capture, gets picked up for assessment) ---

fn minimal_decision_capture() -> CaptureItem {
    CaptureItem {
        kind: "decision".to_owned(),
        title: "Use Postgres".to_owned(),
        rationale: "Concurrent writes required".to_owned(),
        topic_keys: vec![],
        evidence_ids: vec![],
        options: None,
        chosen_option: None,
        extraction_confidence: 0.9,
        expressed_confidence: None,
        supersedes_id: None,
        premised_on_ids: vec![],
        supports_ids: vec![],
        refutes_ids: vec![],
        actor_id: None,
        accepted_by: vec![],
        rejected_by: vec![],
        blocked_actor_id: None,
        decision_id: None,
        participants: vec![],
        restates_id: None,
        session_initiator: None,
    }
}

#[test]
fn find_unscored_decisions_includes_classified_and_direct_and_dedupes_v2() {
    let hivemind_dir = unique_test_dir("find-unscored");
    let tenant_id = TenantId::local();
    {
        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let commands = Commands::new(&ledger);

        let option_a = commands
            .record_option("actor:test", "REST", "REST is simpler to operate")
            .expect("option a");
        let option_b = commands
            .record_option("actor:test", "gRPC", "gRPC needs HTTP/2 and codegen")
            .expect("option b");
        commands
            .propose_decision(DecisionProposalInput {
                grounding: Grounding::NotAsked,
                expressed_confidence: None,
                project: None,
                actor_id: "actor:test",
                title: "Pick the API style",
                rationale: "REST is simpler to operate for this team",
                topic_keys: &["api".to_owned()],
                option_ids: &[option_a.clone(), option_b.clone()],
                option_labels: &["REST".to_owned(), "gRPC".to_owned()],
                chosen_option_id: Some(option_a.as_str()),
                decided_by: None,
                delegated_by: None,
                still_proposed: false,
                hypothesis_ids: &[],
                evidence_ids: &[],
                quote: None,
                question: None,
            })
            .expect("propose decision");

        commands
            .record_ingest_batch_classified(
                "actor:test",
                &["batch-unscored".to_owned()],
                "haiku",
                "2",
                vec![minimal_decision_capture()],
                None,
            )
            .expect("record batch classified");
    }

    let pending = find_unscored_decisions(&hivemind_dir, &tenant_id).expect("scan succeeds");
    assert_eq!(
        pending.len(),
        2,
        "both the directly proposed decision and the classified capture are pending"
    );

    let capture_id = pending
        .iter()
        .find(|p| p.decision_id.starts_with("capture:"))
        .expect("the classified capture is pending")
        .decision_id
        .clone();
    let direct_id = pending
        .iter()
        .find(|p| !p.decision_id.starts_with("capture:"))
        .expect("the directly proposed decision is pending")
        .decision_id
        .clone();
    assert!(pending
        .iter()
        .find(|p| p.decision_id == capture_id)
        .expect("capture entry")
        .decision_text
        .contains("Use Postgres"));
    assert!(pending
        .iter()
        .find(|p| p.decision_id == direct_id)
        .expect("direct entry")
        .decision_text
        .contains("Pick the API style"));

    // Assess the capture: it drops out of the next scan, the direct decision remains.
    {
        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let commands = Commands::new(&ledger);
        let payload = build_assessed_payload(
            &capture_id,
            "test-model",
            "test-prompt-v0",
            stub_assessment_no_quotes_needed(),
        );
        commands
            .record_decision_assessed(ACTOR_ID, payload, None)
            .expect("record assessed capture");
    }

    let pending = find_unscored_decisions(&hivemind_dir, &tenant_id).expect("scan succeeds");
    assert_eq!(
        pending.len(),
        1,
        "the assessed capture drops out; the direct decision remains unscored"
    );
    assert_eq!(pending[0].decision_id, direct_id);

    // Assess the direct decision too: nothing remains pending.
    {
        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let commands = Commands::new(&ledger);
        let payload = build_assessed_payload(
            &direct_id,
            "test-model",
            "test-prompt-v0",
            stub_assessment_no_quotes_needed(),
        );
        commands
            .record_decision_assessed(ACTOR_ID, payload, None)
            .expect("record assessed direct decision");
    }

    let pending = find_unscored_decisions(&hivemind_dir, &tenant_id).expect("scan succeeds");
    assert!(
        pending.is_empty(),
        "both decisions are assessed; nothing left to score"
    );

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

#[test]
fn a_malformed_v2_row_does_not_count_as_an_assessment_so_the_decision_is_still_pending() {
    let hivemind_dir = unique_test_dir("find-unscored-malformed");
    let tenant_id = TenantId::local();
    let decision_id = {
        let ledger = SqliteEventLedger::open(&hivemind_dir).expect("ledger opens");
        let commands = Commands::new(&ledger);
        let option_a = commands
            .record_option("actor:test", "REST", "REST is simpler to operate")
            .expect("option a");
        let option_b = commands
            .record_option("actor:test", "gRPC", "gRPC needs HTTP/2 and codegen")
            .expect("option b");
        let decision_id = commands
            .propose_decision(DecisionProposalInput {
                grounding: Grounding::NotAsked,
                expressed_confidence: None,
                project: None,
                actor_id: "actor:test",
                title: "Pick the API style",
                rationale: "REST is simpler to operate for this team",
                topic_keys: &["api".to_owned()],
                option_ids: &[option_a.clone(), option_b],
                option_labels: &["REST".to_owned(), "gRPC".to_owned()],
                chosen_option_id: Some(option_a.as_str()),
                decided_by: None,
                delegated_by: None,
                still_proposed: false,
                hypothesis_ids: &[],
                evidence_ids: &[],
                quote: None,
                question: None,
            })
            .expect("propose decision");
        // A row a buggy producer or a hand edit left behind: schema version 2, bad shape. The
        // write path refuses it, so it goes in through the ledger.
        ledger
            .append(crate::events::Event {
                tenant_id: tenant_id.clone(),
                event_id: None,
                event_uuid: uuid::Uuid::new_v4(),
                correlation_id: None,
                causation_event_id: None,
                event_type: EventType::DecisionScored,
                actor_id: ACTOR_ID.to_owned(),
                source: crate::events::EventSource::Agent,
                source_ref: None,
                payload: serde_json::json!({
                    "schema_version": 2,
                    "decision_id": decision_id,
                    "model": "model-x",
                    "prompt_version": "prompt-x",
                    "dimensions": {"framing": "not an answer"}
                }),
                ts: Some(chrono::Utc::now()),
            })
            .expect("a ledger appends what it is given");
        decision_id
    };

    let pending = find_unscored_decisions(&hivemind_dir, &tenant_id).expect("scan succeeds");
    assert_eq!(
        pending.len(),
        1,
        "the unreadable row is no assessment: the decision stays pending"
    );
    assert_eq!(pending[0].decision_id, decision_id);

    let _ = std::fs::remove_dir_all(&hivemind_dir);
}

// --- resolve_capture_node_id ---

#[test]
fn resolve_capture_node_id_finds_decision_capture() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let event_id = commands
        .record_ingest_batch_classified(
            "actor:test",
            &["batch-1".to_owned()],
            "haiku",
            "2",
            vec![minimal_decision_capture()],
            None,
        )
        .expect("record succeeds")
        .event_id;

    let (node_id, causation_event_id) =
        resolve_capture_node_id(&ledger, &TenantId::local(), "batch-1", 0)
            .expect("resolves successfully");

    assert_eq!(node_id, format!("capture:{event_id}:0"));
    assert_eq!(causation_event_id, event_id);
}

#[test]
fn resolve_capture_node_id_rejects_non_decision_kind() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let mut evidence_capture = minimal_decision_capture();
    evidence_capture.kind = "evidence".to_owned();
    commands
        .record_ingest_batch_classified(
            "actor:test",
            &["batch-2".to_owned()],
            "haiku",
            "2",
            vec![evidence_capture],
            None,
        )
        .expect("record succeeds");

    let err = resolve_capture_node_id(&ledger, &TenantId::local(), "batch-2", 0).unwrap_err();
    assert!(err.to_string().contains("not \"decision\""));
}

#[test]
fn resolve_capture_node_id_rejects_out_of_range_index() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands
        .record_ingest_batch_classified(
            "actor:test",
            &["batch-3".to_owned()],
            "haiku",
            "2",
            vec![minimal_decision_capture()],
            None,
        )
        .expect("record succeeds");

    let err = resolve_capture_node_id(&ledger, &TenantId::local(), "batch-3", 5).unwrap_err();
    assert!(err.to_string().contains("out of range"));
}

#[test]
fn resolve_capture_node_id_rejects_unknown_batch() {
    let ledger = InMemoryEventLedger::new();
    let err = resolve_capture_node_id(&ledger, &TenantId::local(), "no-such-batch", 0).unwrap_err();
    assert!(err.to_string().contains("no-such-batch"));
}
