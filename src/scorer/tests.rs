// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::events::CaptureItem;
use crate::ledger::InMemoryEventLedger;

// --- clamp01 ---

#[test]
fn clamp01_in_range_passthrough() {
    assert_eq!(clamp01(0.0, "f").unwrap(), 0.0);
    assert_eq!(clamp01(0.5, "f").unwrap(), 0.5);
    assert_eq!(clamp01(1.0, "f").unwrap(), 1.0);
}

#[test]
fn clamp01_above_one_clamped_to_one() {
    assert_eq!(clamp01(1.5, "f").unwrap(), 1.0);
    assert_eq!(clamp01(f64::INFINITY, "f").unwrap(), 1.0);
}

#[test]
fn clamp01_negative_clamped_to_zero() {
    assert_eq!(clamp01(-0.1, "f").unwrap(), 0.0);
    assert_eq!(clamp01(f64::NEG_INFINITY, "f").unwrap(), 0.0);
}

#[test]
fn clamp01_nan_rejected() {
    let err = clamp01(f64::NAN, "framing").unwrap_err();
    assert!(err.to_string().contains("framing"));
    assert!(err.to_string().contains("NaN"));
}

// --- validate_stakes ---

#[test]
fn validate_stakes_positive_passthrough() {
    assert_eq!(validate_stakes(1.0).unwrap(), 1.0);
    assert_eq!(validate_stakes(100.0).unwrap(), 100.0);
    assert_eq!(validate_stakes(0.0).unwrap(), 0.0);
}

#[test]
fn validate_stakes_negative_rejected() {
    assert!(validate_stakes(-0.001).is_err());
}

#[test]
fn validate_stakes_nan_rejected() {
    let err = validate_stakes(f64::NAN).unwrap_err();
    assert!(err.to_string().contains("stakes"));
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

// --- build_scored_payload ---

fn valid_scorer_output_json(stakes: f64, information_score: f64) -> serde_json::Value {
    serde_json::json!({
        "quality_dims": {
            "framing": {"score": 0.5, "explanation": "e"},
            "alternatives": {"score": 0.5, "explanation": "e"},
            "information": {"score": information_score, "explanation": "e"},
            "reasoning": {"score": 0.5, "explanation": "e"},
            "values_tradeoffs": {"score": 0.5, "explanation": "e"},
            "bias_exposure": {"score": 0.5, "explanation": "e"},
            "calibration": {"score": 0.5, "explanation": "e"}
        },
        "importance": {
            "stakes": stakes,
            "stakes_explanation": "e",
            "irreversibility": 0.5,
            "irreversibility_explanation": "e",
            "actionability": 0.5,
            "actionability_explanation": "e"
        }
    })
}

#[test]
fn build_scored_payload_clamps_out_of_range_scores() {
    let output: ScorerOutput =
        serde_json::from_value(valid_scorer_output_json(8.0, 1.5)).expect("valid scorer output");

    let payload =
        build_scored_payload("capture:1:0", "haiku", "v1", output).expect("payload builds");

    assert_eq!(payload.capture_node_id, "capture:1:0");
    assert_eq!(payload.scorer_model, "haiku");
    assert_eq!(payload.weight_version, "v1");
    assert_eq!(payload.quality_dims.information.score, 1.0);
    assert_eq!(payload.importance.stakes, 8.0);
}

#[test]
fn build_scored_payload_rejects_negative_stakes() {
    let output: ScorerOutput =
        serde_json::from_value(valid_scorer_output_json(-1.0, 0.5)).expect("valid scorer output");

    let err = build_scored_payload("capture:1:0", "haiku", "v1", output).unwrap_err();
    assert!(err.to_string().contains("stakes"));
}

// --- resolve_capture_node_id ---

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
        accepted_by: None,
        rejected_by: None,
        blocked_actor_id: None,
        decision_id: None,
        participants: vec![],
        session_initiator: None,
    }
}

#[test]
fn resolve_capture_node_id_finds_decision_capture() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let event_id = commands
        .record_ingest_batch_classified(
            "actor:test",
            "batch-1",
            "haiku",
            "2",
            vec![minimal_decision_capture()],
            None,
        )
        .expect("record succeeds");

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
            "batch-2",
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
            "batch-3",
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
