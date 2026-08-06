// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;

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
