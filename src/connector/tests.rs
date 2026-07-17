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

// ---------------------------------------------------------------------------
// Same-as / dedup layer tests
// ---------------------------------------------------------------------------

fn make_source_ref_json(import_run_id: &str) -> String {
    let src = ConnectorSourceRef {
        source: "connector".to_owned(),
        connector_kind: ConnectorKind::GitFile,
        doc_id: "repo:/test.md".to_owned(),
        version_id: "abc".to_owned(),
        content_hash: "hash".to_owned(),
        source_url: None,
        statement_ordinal: 0,
        statement_span: (0, 10),
        import_run_id: import_run_id.to_owned(),
        importer_actor_id: "agent:test".to_owned(),
        original_actor_id: None,
    };
    serde_json::to_string(&src).unwrap()
}

fn emit_test_decision(
    ledger: &crate::ledger::InMemoryEventLedger,
    tenant_id: &TenantId,
    decision_id: &str,
    title: &str,
    rationale: &str,
    topic_keys: &[&str],
    import_run_id: &str,
) {
    use crate::events::{DecisionProposedPayload, EventBuilder, EventPayload, EventProvenance};
    use uuid::Uuid;

    let source_ref = make_source_ref_json(import_run_id);
    let event = EventBuilder::new(
        Uuid::new_v4(),
        "agent:test",
        EventPayload::DecisionProposed(DecisionProposedPayload {
            decision_id: decision_id.to_owned(),
            title: title.to_owned(),
            rationale: rationale.to_owned(),
            topic_keys: topic_keys.iter().map(|s| s.to_string()).collect(),
            option_ids: vec![],
            chosen_option_id: None,
            hypothesis_ids: vec![],
            evidence_ids: vec![],
            expressed_confidence: None,
        }),
    )
    .tenant_id(tenant_id.clone())
    .provenance(EventProvenance::document(source_ref))
    .build()
    .unwrap();
    ledger.append_for_tenant(tenant_id, event).unwrap();
}

#[test]
fn test_token_overlap_identical() {
    let score = token_overlap("we decided to use Rust", "we decided to use Rust");
    assert_eq!(score, 100);
}

#[test]
fn test_token_overlap_disjoint() {
    let score = token_overlap("alpha beta gamma", "delta epsilon zeta");
    assert_eq!(score, 0);
}

#[test]
fn test_token_overlap_partial() {
    let score = token_overlap("alpha beta gamma delta", "alpha beta epsilon zeta");
    assert!(score > 0 && score < 100, "score: {score}");
}

#[test]
fn test_find_same_as_candidates_match() {
    use crate::ledger::InMemoryEventLedger;
    let ledger = InMemoryEventLedger::new();
    let tenant = TenantId::local();
    let run = "run-001";

    emit_test_decision(
        &ledger,
        &tenant,
        "d-aaa",
        "We decided to adopt Rust for performance",
        "Rust gives us zero-cost abstractions and safety",
        &["rust", "performance"],
        run,
    );
    emit_test_decision(
        &ledger,
        &tenant,
        "d-bbb",
        "We decided to adopt Rust for performance reasons",
        "Rust gives us zero-cost abstractions and memory safety",
        &["rust", "performance"],
        run,
    );

    let config = SameAsConfig {
        min_score: 70,
        min_field_matches: 2,
    };
    let report = find_connector_same_as_candidates(&ledger, &tenant, run, &config).unwrap();
    assert_eq!(report.candidates_found, 1, "expected one candidate pair");
    let c = &report.candidates[0];
    assert!(c.score >= 70, "score too low: {}", c.score);
}

#[test]
fn test_find_same_as_candidates_no_match() {
    use crate::ledger::InMemoryEventLedger;
    let ledger = InMemoryEventLedger::new();
    let tenant = TenantId::local();
    let run = "run-002";

    emit_test_decision(
        &ledger,
        &tenant,
        "d-ccc",
        "We decided to use Postgres for the database",
        "Postgres has excellent JSONB support",
        &["postgres", "database"],
        run,
    );
    emit_test_decision(
        &ledger,
        &tenant,
        "d-ddd",
        "We will migrate the frontend to React",
        "The team has React expertise from prior projects",
        &["react", "frontend"],
        run,
    );

    let config = SameAsConfig::default();
    let report = find_connector_same_as_candidates(&ledger, &tenant, run, &config).unwrap();
    assert_eq!(report.candidates_found, 0);
}

#[test]
fn test_confirm_same_as_idempotent() {
    use crate::ledger::InMemoryEventLedger;
    let ledger = InMemoryEventLedger::new();
    let tenant = TenantId::local();

    let r1 = confirm_same_as(&ledger, &tenant, "d-aaa", "d-bbb", "agent:test").unwrap();
    assert!(!r1.idempotent);
    assert_eq!(r1.action, "confirmed");

    let r2 = confirm_same_as(&ledger, &tenant, "d-aaa", "d-bbb", "agent:test").unwrap();
    assert!(r2.idempotent, "second confirm should be a no-op");
}

#[test]
fn test_retract_same_as_idempotent() {
    use crate::ledger::InMemoryEventLedger;
    let ledger = InMemoryEventLedger::new();
    let tenant = TenantId::local();

    let r1 = retract_same_as(&ledger, &tenant, "d-eee", "d-fff", "agent:test").unwrap();
    assert!(!r1.idempotent);
    assert_eq!(r1.action, "retracted");

    let r2 = retract_same_as(&ledger, &tenant, "d-eee", "d-fff", "agent:test").unwrap();
    assert!(r2.idempotent);
}

#[test]
fn test_retracted_pair_skipped_in_candidates() {
    use crate::ledger::InMemoryEventLedger;
    let ledger = InMemoryEventLedger::new();
    let tenant = TenantId::local();
    let run = "run-003";

    emit_test_decision(
        &ledger,
        &tenant,
        "d-ggg",
        "We decided to adopt Rust for performance",
        "Rust gives us zero-cost abstractions and safety",
        &["rust", "performance"],
        run,
    );
    emit_test_decision(
        &ledger,
        &tenant,
        "d-hhh",
        "We decided to adopt Rust for performance reasons",
        "Rust gives us zero-cost abstractions and memory safety",
        &["rust", "performance"],
        run,
    );

    retract_same_as(&ledger, &tenant, "d-ggg", "d-hhh", "agent:test").unwrap();

    let config = SameAsConfig {
        min_score: 70,
        min_field_matches: 2,
    };
    let report = find_connector_same_as_candidates(&ledger, &tenant, run, &config).unwrap();
    assert_eq!(
        report.candidates_found, 0,
        "retracted pair should be permanently skipped"
    );
}
