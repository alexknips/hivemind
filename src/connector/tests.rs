use super::*;

#[test]
fn test_segment_empty() {
    assert!(segment_into_statements("").is_empty());
}

#[test]
fn test_segment_single_sentence() {
    let stmts = segment_into_statements("We decided to use Rust.");
    assert_eq!(stmts.len(), 1);
    assert_eq!(stmts[0].text, "We decided to use Rust.");
}

#[test]
fn test_segment_two_sentences() {
    let text = "We decided to use Rust. This was approved by the team.";
    let stmts = segment_into_statements(text);
    assert_eq!(stmts.len(), 2, "stmts: {stmts:?}");
}

#[test]
fn test_narrowness_guardrail_positive() {
    assert!(has_decision_keywords(
        "We decided to use Rust for the backend."
    ));
    assert!(has_decision_keywords(
        "The team approved the new architecture."
    ));
}

#[test]
fn test_narrowness_guardrail_negative() {
    assert!(!has_decision_keywords("The sky is blue today."));
    assert!(!has_decision_keywords(
        "Functions can be reused across modules."
    ));
}

#[test]
fn test_diff_unchanged() {
    let s = |t: &str| Statement {
        text: t.to_owned(),
        byte_span: (0, t.len()),
    };
    let prev = vec![
        s("Alpha beta gamma"),
        s("Beta gamma delta"),
        s("Gamma delta epsilon"),
    ];
    let next = vec![
        s("Alpha beta gamma"),
        s("Beta gamma delta"),
        s("Gamma delta epsilon"),
    ];
    let diff = diff_adjacent(&prev, &next);
    assert!(
        diff.iter().all(|d| matches!(d, DiffItem::Unchanged { .. })),
        "expected all Unchanged, got: {diff:?}"
    );
}

#[test]
fn test_diff_added() {
    let s = |t: &str| Statement {
        text: t.to_owned(),
        byte_span: (0, t.len()),
    };
    let prev = vec![s("Alpha beta gamma"), s("Beta gamma delta")];
    let next = vec![
        s("Alpha beta gamma"),
        s("Beta gamma delta"),
        s("New statement here"),
    ];
    let diff = diff_adjacent(&prev, &next);
    assert!(
        diff.iter().any(|d| matches!(d, DiffItem::Added { .. })),
        "expected an Added item, got: {diff:?}"
    );
}

#[test]
fn test_connector_import_uuid_deterministic() {
    let u1 = connector_import_uuid("git_file", "repo:file", "abc123", 1, "proposal");
    let u2 = connector_import_uuid("git_file", "repo:file", "abc123", 1, "proposal");
    assert_eq!(u1, u2);
}

#[test]
fn test_slugify_author() {
    assert_eq!(slugify_author("Jane Doe"), "jane-doe");
    assert_eq!(slugify_author("María José"), "maria-jose");
}

#[test]
fn test_has_decision_keywords_will() {
    assert!(has_decision_keywords("We will use PostgreSQL."));
}

#[test]
fn test_topic_keys_from_doc_id() {
    let keys = topic_keys_from_doc_id("/repo:/docs/adr-001.md");
    assert!(keys.iter().any(|k| k.contains("docs")), "keys: {keys:?}");
}
