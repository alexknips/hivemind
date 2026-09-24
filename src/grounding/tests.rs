// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use serde_json::json;

use crate::commands::{Commands, DecisionProposalInput, Grounding};
use crate::ledger::InMemoryEventLedger;

use super::*;

fn wire(items: Value) -> std::result::Result<GroundingSpec, String> {
    GroundingSpec::from_wire(items.as_array().expect("array"), &[], &[])
}

fn propose(commands: &Commands<'_, InMemoryEventLedger>, title: &str) -> String {
    let option_id = commands
        .record_option("actor:alice", "Only option", "The only option")
        .expect("record option");
    commands
        .propose_decision(DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: "actor:alice",
            title,
            rationale: "Rationale text long enough for the readable floor",
            topic_keys: &["topic".to_owned()],
            option_ids: std::slice::from_ref(&option_id),
            option_labels: &["Only option".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            delegated_by: None,
            project: None,
        })
        .expect("propose decision")
}

#[test]
fn from_wire_parses_every_item_kind() {
    let spec = wire(json!([
        { "kind": "decision", "description": "the auth decision" },
        { "kind": "decision", "decision_id": "decision-abc" },
        { "kind": "evidence", "content": "p95 was 180ms", "source": "ci run 42" },
        { "kind": "evidence", "content": "no source given" },
        { "kind": "evidence", "evidence_id": "evidence-1" },
        { "kind": "assumption", "statement": "traffic stays flat" },
        { "kind": "assumption", "hypothesis_id": "hypothesis-1" },
        { "kind": "bet", "statement": "the vendor survives", "would_change_if": "they raise prices", "check_by": "2026-12-01" },
    ]))
    .expect("every kind parses");

    assert_eq!(spec.decisions.len(), 2);
    assert_eq!(spec.decisions[0].field, "grounding[0]");
    assert_eq!(
        spec.decisions[0].target,
        PremiseTarget::Description("the auth decision".to_owned())
    );
    assert_eq!(
        spec.decisions[1].target,
        PremiseTarget::Id("decision-abc".to_owned())
    );
    assert_eq!(spec.evidence.len(), 2);
    assert_eq!(spec.evidence[0].source.as_deref(), Some("ci run 42"));
    assert_eq!(spec.evidence[1].source, None);
    assert_eq!(spec.evidence_ids, ["evidence-1"]);
    assert_eq!(spec.assumptions, ["traffic stays flat"]);
    assert_eq!(spec.hypothesis_ids, ["hypothesis-1"]);
    let bet = spec.bet.expect("bet parsed");
    assert_eq!(bet.statement.as_deref(), Some("the vendor survives"));
    assert_eq!(bet.would_change_if.as_deref(), Some("they raise prices"));
    assert_eq!(
        bet.check_by.map(|at| at.to_rfc3339()),
        Some("2026-12-01T00:00:00+00:00".to_owned())
    );
}

#[test]
fn from_wire_maps_the_deprecated_id_aliases_into_the_spec() {
    let spec = GroundingSpec::from_wire(
        &[],
        &["hypothesis-1".to_owned()],
        &["evidence-1".to_owned(), "evidence-2".to_owned()],
    )
    .expect("aliases alone parse");

    assert_eq!(spec.hypothesis_ids, ["hypothesis-1"]);
    assert_eq!(spec.evidence_ids, ["evidence-1", "evidence-2"]);
    assert!(!spec.is_empty(), "an alias id counts as grounding");
    assert!(GroundingSpec::from_wire(&[], &[], &[])
        .expect("empty parses")
        .is_empty());
}

#[test]
fn from_wire_rejects_malformed_items_naming_the_field() {
    let cases = [
        (json!(["not an object"]), "`grounding[0]` must be an object"),
        (json!([{ "description": "x" }]), "`grounding[0].kind`"),
        (
            json!([{ "kind": "opinion", "statement": "x" }]),
            "`opinion` is not one of",
        ),
        (
            json!([{ "kind": "decision", "description": "x", "decision_id": "decision-1" }]),
            "exactly one of `description` or `decision_id`",
        ),
        (
            json!([{ "kind": "decision" }]),
            "exactly one of `description` or `decision_id`",
        ),
        (
            json!([{ "kind": "decision", "description": "   " }]),
            "`grounding[0].description` must not be blank",
        ),
        (
            json!([{ "kind": "decision", "description": 7 }]),
            "`grounding[0].description` must be a string",
        ),
        (
            json!([{ "kind": "decision", "description": "x", "content": "y" }]),
            "unknown field `content` for kind `decision`",
        ),
        (
            json!([{ "kind": "evidence", "evidence_id": "evidence-1", "source": "somewhere" }]),
            "drop `source`",
        ),
        (json!([{ "kind": "evidence" }]), "exactly one of `content`"),
        (
            json!([{ "kind": "assumption", "statement": "x", "hypothesis_id": "hypothesis-1" }]),
            "exactly one of `statement` or `hypothesis_id`",
        ),
        (
            json!([{ "kind": "bet", "check_by": "next tuesday" }]),
            "`grounding[0].check_by` must be an RFC3339 timestamp or a YYYY-MM-DD date",
        ),
        (
            json!([{ "kind": "bet" }, { "kind": "bet" }]),
            "at most one bet",
        ),
    ];
    for (items, expected) in cases {
        let error = wire(items.clone()).expect_err("malformed item must be refused");
        assert!(
            error.contains(expected),
            "for {items}: expected {expected:?} in {error:?}"
        );
    }
}

#[test]
fn resolve_grounding_resolves_ids_and_descriptions_and_dedupes() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let goal_id = propose(&commands, "Keep the ledger append-only");
    let vendor_id = propose(&commands, "Choose the queue vendor");

    let spec = wire(json!([
        { "kind": "decision", "description": "Keep the ledger append-only" },
        { "kind": "decision", "decision_id": goal_id },
        { "kind": "decision", "decision_id": vendor_id },
        { "kind": "evidence", "evidence_id": "evidence-1" },
        { "kind": "evidence", "evidence_id": "evidence-1" },
    ]))
    .expect("spec parses");
    let GroundingResolution::Ready(resolved) =
        resolve_grounding(&ledger, &TenantId::local(), spec).expect("resolves")
    else {
        panic!("every premise resolves");
    };

    assert_eq!(
        resolved.plan.premise_decision_ids,
        [goal_id.clone(), vendor_id.clone()],
        "the goal named twice (by description, then by id) is one premise, in first-named order"
    );
    assert_eq!(resolved.plan.evidence_ids, ["evidence-1"]);

    // Titles fill the decision labels of a reply and leave other items alone.
    let labelled = resolved.label(vec![
        RestsOn {
            kind: RestsOnKind::Decision,
            id: goal_id,
            label: None,
        },
        RestsOn {
            kind: RestsOnKind::Evidence,
            id: "evidence-1".to_owned(),
            label: None,
        },
    ]);
    assert_eq!(
        labelled[0].label.as_deref(),
        Some("Keep the ledger append-only")
    );
    assert_eq!(labelled[1].label, None);
}

#[test]
fn resolve_grounding_returns_an_ambiguous_premise_untouched() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    propose(&commands, "Adopt the queue");
    propose(&commands, "Adopt the queue");

    let spec = wire(json!([
        { "kind": "evidence", "content": "an observation" },
        { "kind": "decision", "description": "Adopt the queue" },
    ]))
    .expect("spec parses");
    let GroundingResolution::Unresolved(unresolved) =
        resolve_grounding(&ledger, &TenantId::local(), spec).expect("resolves")
    else {
        panic!("two identical titles are ambiguous");
    };

    assert_eq!(unresolved.field, "grounding[1]");
    assert_eq!(unresolved.text, "Adopt the queue");
    assert_eq!(unresolved.candidates().len(), 2);
    let envelope = unresolved.envelope();
    assert_eq!(envelope["data"]["outcome"], "ambiguous");
    assert_eq!(envelope["data"]["field"], "grounding[1]");
    assert_eq!(
        envelope["data"]["candidates"].as_array().map(Vec::len),
        Some(2)
    );
}

#[test]
fn resolve_grounding_returns_a_miss_as_data() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    propose(&commands, "Choose the queue vendor");

    for (item, key, text) in [
        (
            json!({ "kind": "decision", "description": "quantum flux capacitor" }),
            "description",
            "quantum flux capacitor",
        ),
        (
            json!({ "kind": "decision", "decision_id": "decision-does-not-exist" }),
            "decision_id",
            "decision-does-not-exist",
        ),
    ] {
        let spec = wire(json!([item])).expect("spec parses");
        let GroundingResolution::Unresolved(unresolved) =
            resolve_grounding(&ledger, &TenantId::local(), spec).expect("resolves")
        else {
            panic!("nothing matches {text}");
        };
        assert!(unresolved.candidates().is_empty());
        let envelope = unresolved.envelope();
        assert_eq!(envelope["data"]["outcome"], "not_found");
        assert_eq!(envelope["data"]["field"], "grounding[0]");
        assert_eq!(envelope["data"][key], text);
    }
}

#[test]
fn resolve_grounding_skips_the_graph_when_no_premise_decision_is_named() {
    // No ledger events at all: a graph rebuild would still succeed, so this pins the plan shape
    // rather than the skip — a spec of only new nodes resolves to a plan carrying them verbatim.
    let ledger = InMemoryEventLedger::new();
    let spec = wire(json!([
        { "kind": "evidence", "content": "an observation", "source": "somewhere" },
        { "kind": "assumption", "statement": "an assumption" },
        { "kind": "bet" },
    ]))
    .expect("spec parses");
    let GroundingResolution::Ready(resolved) =
        resolve_grounding(&ledger, &TenantId::local(), spec).expect("resolves")
    else {
        panic!("nothing to resolve");
    };
    assert!(resolved.plan.premise_decision_ids.is_empty());
    assert_eq!(resolved.plan.new_evidence.len(), 1);
    assert_eq!(resolved.plan.new_assumptions, ["an assumption"]);
    assert!(resolved.plan.bet.is_some());
}
