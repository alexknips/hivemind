//! Closes the hole that let hivemind-zdsh.10 ship `option_descriptions` on
//! `decision.proposed` without a matching `schemas/v0/decision.proposed.json` update
//! (hivemind-zdsh.20): every event type the write layer (`commands::Commands` plus the
//! `connector` same-as functions) actually constructs, produced through its real public
//! API rather than a hand-authored fixture, must validate against its `schemas/v0` file.
//! A payload field a caller starts populating without a matching schema update fails this
//! test, not just a hand-maintained fixture that nobody remembers to update alongside it.
//!
//! Six event types declared in `schemas/v0` (`decision.requested`, `blocker.reported`,
//! `blocker.resolved`, `notification.sent`, `notification.acknowledged`,
//! `decision.metadata_derived`) have no production write path anywhere in this codebase
//! today (verified by grepping for their payload struct literals outside `#[cfg(test)]`
//! code) — their schemas and fixtures exist ahead of the feature that will emit them, so
//! this test cannot and does not exercise them.
//!
//! `tenant_id` is deliberately excluded from the JSON validated here: every hand-authored
//! fixture in `tests/fixtures/v0` already omits it (see `type_contracts.rs` and
//! `src/events/tests.rs`), so `schemas/v0` documents the payload/envelope contract without
//! it by established convention. A real captured `Event` always carries `tenant_id` — that
//! gap between the schemas and the real wire shape is a separate, pre-existing issue (not
//! introduced by this change) and is out of scope for this bead.

use std::fs;
use std::path::PathBuf;

use hivemind::commands::{
    Commands, DecisionProposalInput, DeterminedProject, GroundInput, Grounding, SupersedeInput,
};
use hivemind::connector;
use hivemind::events::{
    CaptureItem, DecisionScoredPayload, EventType, ImportanceFactors, IngestTurn,
    ProjectAnchorKind, ProjectLinkKind, QualityDim, QualityDims, TenantId,
};
use hivemind::ledger::{EventLedger, InMemoryEventLedger};
use serde_json::Value;

#[test]
fn every_write_path_event_validates_against_its_schema() {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let tenant_id = TenantId::local();
    let actor = "agent:contract-test";

    // -- evidence, hypothesis, options --
    let evidence_id = commands
        .record_evidence(actor, "Contract test evidence")
        .expect("record evidence");
    let hypothesis_id = commands
        .record_hypothesis(actor, "Contract test hypothesis")
        .expect("record hypothesis");
    let option_a = commands
        .record_option(actor, "Option A", "Description A")
        .expect("record option a");
    let option_b = commands
        .record_option(actor, "Option B", "Description B")
        .expect("record option b");

    // -- decision.proposed (chosen + quote/question) -> auto decision.accepted,
    // plus relation.added for has_option/chose/assumes/based_on --
    let decision_id = commands
        .propose_decision(DecisionProposalInput {
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: actor,
            title: "Use option A",
            rationale: "Option A is the simplest fit for this contract test scenario.",
            topic_keys: &["conformance".to_owned()],
            option_ids: &[option_a.clone(), option_b.clone()],
            option_labels: &["Option A".to_owned(), "Option B".to_owned()],
            chosen_option_id: Some(&option_a),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: std::slice::from_ref(&hypothesis_id),
            evidence_ids: std::slice::from_ref(&evidence_id),
            quote: Some("We'll go with option A"),
            question: Some("Which option should we use?"),
        })
        .expect("propose decision");

    // -- decision.accepted carrying a delegation marker (hivemind-zdsh.6): an agent deciding
    // within a scope a human delegated, so the optional `delegated_by` field is validated
    // against the schema's `additionalProperties: false` rather than a hand-authored fixture --
    let delegated_option = commands
        .record_option(actor, "Delegated option", "n/a")
        .expect("record delegated option");
    commands
        .propose_decision(DecisionProposalInput {
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: actor,
            title: "A decision made within a delegated scope",
            rationale: "This decision exists so the test can exercise the delegation marker.",
            topic_keys: &["conformance".to_owned()],
            option_ids: std::slice::from_ref(&delegated_option),
            option_labels: &["Delegated option".to_owned()],
            chosen_option_id: Some(&delegated_option),
            decided_by: None,
            delegated_by: Some("human:contract-owner"),
            still_proposed: false,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose delegated decision");

    // -- decision.rejected (with reason) via disagree --
    let rejected_option = commands
        .record_option(actor, "Rejected option", "n/a")
        .expect("record rejected option");
    let rejected_decision_id = commands
        .propose_decision(DecisionProposalInput {
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: actor,
            title: "A decision someone disagrees with",
            rationale: "This decision exists so the test can exercise disagreement.",
            topic_keys: &["conformance".to_owned()],
            option_ids: &[rejected_option],
            option_labels: &["Rejected option".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose decision to reject");
    commands
        .disagree(actor, &rejected_decision_id, "Contract test disagreement")
        .expect("disagree");

    // -- decision.superseded via supersede() (also exercises a second decision.proposed
    // without an auto-accept, since supersede never auto-accepts) --
    let old_option = commands
        .record_option(actor, "Old option", "n/a")
        .expect("record old option");
    let old_decision_id = commands
        .propose_decision(DecisionProposalInput {
            project: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: actor,
            title: "An old decision",
            rationale: "This decision exists so the test can exercise supersession.",
            topic_keys: &["conformance".to_owned()],
            option_ids: &[old_option],
            option_labels: &["Old option".to_owned()],
            chosen_option_id: None,
            decided_by: None,
            delegated_by: None,
            still_proposed: true,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose old decision");
    commands
        .supersede(SupersedeInput {
            project: None,
            actor_id: actor,
            old_decision_id: &old_decision_id,
            new_title: "The replacement decision",
            new_rationale: "The replacement decision better fits the contract test scenario.",
            topic_keys: &[],
            option_labels: &["New option".to_owned()],
            chosen_option_label: None,
            hypothesis_ids: &[],
            evidence_ids: &[],
        })
        .expect("supersede");

    // -- relation.added (based_on, direct call) / relation.removed (same_as) --
    commands
        .attach_evidence(&decision_id, &evidence_id, actor)
        .expect("attach evidence");
    connector::retract_same_as(
        &ledger,
        &tenant_id,
        &decision_id,
        &rejected_decision_id,
        actor,
    )
    .expect("retract same_as");

    // -- ingest.batch_received / ingest.batch_classified --
    commands
        .record_ingest_batch(
            actor,
            "batch:contract-test",
            "claude",
            "session:contract-test",
            vec![IngestTurn {
                turn_id: "turn-1".to_owned(),
                role: "user".to_owned(),
                text: "Should we use REST?".to_owned(),
                truncated: false,
            }],
        )
        .expect("record ingest batch");
    commands
        .record_ingest_batch_classified(
            actor,
            &["batch:contract-test".to_owned()],
            "claude-haiku-4-5-20251001",
            "1",
            vec![CaptureItem {
                kind: "decision".to_owned(),
                title: "Use REST".to_owned(),
                rationale: "REST is curl-friendly".to_owned(),
                topic_keys: vec!["api-design".to_owned()],
                evidence_ids: vec![],
                options: Some(vec!["REST".to_owned(), "gRPC".to_owned()]),
                chosen_option: Some("REST".to_owned()),
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
                session_initiator: None,
            }],
            None,
        )
        .expect("record ingest batch classified");

    // -- decision.scored --
    commands
        .record_decision_scored(
            actor,
            DecisionScoredPayload {
                capture_node_id: "capture:contract-test:0".to_owned(),
                scorer_model: "claude-haiku-4-5-20251001".to_owned(),
                weight_version: "v1".to_owned(),
                supersedes_score_id: None,
                quality_dims: QualityDims {
                    framing: QualityDim {
                        score: 0.8,
                        explanation: "Problem framed clearly".to_owned(),
                    },
                    alternatives: QualityDim {
                        score: 0.7,
                        explanation: "Two options considered".to_owned(),
                    },
                    information: QualityDim {
                        score: 0.6,
                        explanation: "Limited evidence referenced".to_owned(),
                    },
                    reasoning: QualityDim {
                        score: 0.9,
                        explanation: "Logic is sound".to_owned(),
                    },
                    values_tradeoffs: QualityDim {
                        score: 0.5,
                        explanation: "Tradeoffs implicit".to_owned(),
                    },
                    bias_exposure: QualityDim {
                        score: 0.8,
                        explanation: "No obvious bias".to_owned(),
                    },
                    calibration: QualityDim {
                        score: 0.7,
                        explanation: "Confidence matches evidence".to_owned(),
                    },
                },
                importance: ImportanceFactors {
                    stakes: 10.0,
                    stakes_explanation: "Department-level API choice".to_owned(),
                    irreversibility: 0.6,
                    irreversibility_explanation: "Migration possible but costly".to_owned(),
                    actionability: 1.0,
                    actionability_explanation: "Decision has a clear owner".to_owned(),
                },
            },
            None,
        )
        .expect("record decision scored");

    // -- project.registered / linked / unlinked / anchored / unanchored --
    commands
        .register_project(actor, "contract-a", Some("Contract A"), Some("Purpose A"))
        .expect("register project a");
    commands
        .register_project(actor, "contract-b", Some("Contract B"), Some("Purpose B"))
        .expect("register project b");
    commands
        .link_project(
            actor,
            "contract-a",
            "contract-b",
            ProjectLinkKind::DependsOn,
        )
        .expect("link project");
    commands
        .unlink_project(
            actor,
            "contract-a",
            "contract-b",
            ProjectLinkKind::DependsOn,
        )
        .expect("unlink project");
    commands
        .anchor_project(
            actor,
            "contract-a",
            ProjectAnchorKind::Rig,
            "contract-test-rig",
        )
        .expect("anchor project");
    commands
        .unanchor_project(
            actor,
            "contract-a",
            ProjectAnchorKind::Rig,
            "contract-test-rig",
        )
        .expect("unanchor project");

    // -- grounding (hivemind-gwhr.1): a bet hypothesis with kind/check_by/would_change_if, a
    // decision that follows from a premise at capture (relation.added FOLLOWS_FROM with
    // causation), later grounding without causation, and a standalone FOLLOWS_FROM link --
    let check_by = chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
        .expect("valid rfc3339")
        .with_timezone(&chrono::Utc);
    let bet_id = commands
        .record_bet(
            actor,
            None,
            "A decision resting on a bet",
            Some(check_by),
            Some("A load test shows the assumption does not hold"),
        )
        .expect("record bet");
    let grounded_option = commands
        .record_option(actor, "Grounded option", "n/a")
        .expect("record grounded option");
    let premise_ids = [decision_id.clone()];
    let grounded_decision_id = commands
        .propose_decision(DecisionProposalInput {
            // A stated, registered project (hivemind-s15q.3): writes project + project_source =
            // stated, so the conformance check covers both fields; every other decision above
            // states none and writes project_source = personal_fallback.
            project: Some(DeterminedProject::stated("contract-a")),
            grounding: Grounding::Declared {
                premise_decision_ids: &premise_ids,
                evidence_ids: &[],
                hypothesis_ids: std::slice::from_ref(&bet_id),
            },
            expressed_confidence: Some("medium"),
            actor_id: actor,
            title: "A decision that follows from another",
            rationale: "This decision names a premise so the test can exercise grounding.",
            topic_keys: &["conformance".to_owned()],
            option_ids: std::slice::from_ref(&grounded_option),
            option_labels: &["Grounded option".to_owned()],
            chosen_option_id: Some(&grounded_option),
            decided_by: None,
            delegated_by: None,
            still_proposed: false,
            hypothesis_ids: std::slice::from_ref(&bet_id),
            evidence_ids: &[],
            quote: None,
            question: None,
        })
        .expect("propose grounded decision");
    commands
        .ground_decision(GroundInput {
            actor_id: actor,
            decision_id: &grounded_decision_id,
            premise_decision_ids: std::slice::from_ref(&old_decision_id),
            evidence_ids: std::slice::from_ref(&evidence_id),
            hypothesis_ids: &[],
        })
        .expect("ground decision later");
    commands
        .link_follows_from(&old_decision_id, &decision_id, actor)
        .expect("link follows_from");

    // -- validate every emitted event against its schemas/v0 file --
    let events = ledger.read(0, 1000).expect("read events");
    assert!(
        !events.is_empty(),
        "write path produced no events to validate"
    );

    let seen_types: Vec<EventType> = events.iter().map(|event| event.event_type).collect();
    for expected in [
        EventType::DecisionProposed,
        EventType::DecisionAccepted,
        EventType::DecisionRejected,
        EventType::DecisionSuperseded,
        EventType::EvidenceRecorded,
        EventType::HypothesisRecorded,
        EventType::RelationAdded,
        EventType::RelationRemoved,
        EventType::IngestBatchReceived,
        EventType::IngestBatchClassified,
        EventType::DecisionScored,
        EventType::ProjectRegistered,
        EventType::ProjectLinked,
        EventType::ProjectUnlinked,
        EventType::ProjectAnchored,
        EventType::ProjectUnanchored,
    ] {
        assert!(
            seen_types.contains(&expected),
            "expected the write path exercise above to produce a {expected:?} event"
        );
    }

    assert!(
        events.iter().any(|event| {
            event.event_type == EventType::HypothesisRecorded
                && event.payload["kind"] == "bet"
                && event.payload["check_by"].is_string()
                && event.payload["would_change_if"].is_string()
        }),
        "expected a bet hypothesis carrying check_by and would_change_if"
    );
    assert!(
        events.iter().any(|event| {
            event.event_type == EventType::RelationAdded
                && event.payload["relation"] == "FOLLOWS_FROM"
        }),
        "expected a FOLLOWS_FROM relation.added event"
    );
    assert!(
        events.iter().any(|event| {
            event.event_type == EventType::DecisionAccepted
                && event.payload["delegated_by"] == "human:contract-owner"
        }),
        "expected a decision.accepted event carrying delegated_by"
    );

    for event in &events {
        let event_name = schema_file_stem(event.event_type);
        let schema = read_schema(event_name);
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");

        let mut envelope = serde_json::to_value(event).expect("event serializes");
        // See module doc: every hand-authored fixture omits tenant_id by established
        // convention, and schemas/v0 documents the contract without it.
        envelope
            .as_object_mut()
            .expect("event serializes to an object")
            .remove("tenant_id");

        assert!(
            validator.is_valid(&envelope),
            "a real {event_name} event failed schemas/v0/{event_name}.json: {:#?}\nevent: {envelope:#}",
            validator.iter_errors(&envelope).collect::<Vec<_>>(),
        );
    }
}

fn schema_file_stem(event_type: EventType) -> &'static str {
    match event_type {
        EventType::DecisionProposed => "decision.proposed",
        EventType::DecisionRequested => "decision.requested",
        EventType::DecisionAccepted => "decision.accepted",
        EventType::DecisionRejected => "decision.rejected",
        EventType::DecisionSuperseded => "decision.superseded",
        EventType::EvidenceRecorded => "evidence.recorded",
        EventType::HypothesisRecorded => "hypothesis.recorded",
        EventType::RelationAdded => "relation.added",
        EventType::RelationRemoved => "relation.removed",
        EventType::BlockerReported => "blocker.reported",
        EventType::BlockerResolved => "blocker.resolved",
        EventType::NotificationSent => "notification.sent",
        EventType::NotificationAcknowledged => "notification.acknowledged",
        EventType::IngestBatchReceived => "ingest.batch_received",
        EventType::IngestBatchClassified => "ingest.batch_classified",
        EventType::DecisionScored => "decision.scored",
        EventType::DecisionMetadataDerived => "decision.metadata_derived",
        EventType::ProjectRegistered => "project.registered",
        EventType::ProjectLinked => "project.linked",
        EventType::ProjectUnlinked => "project.unlinked",
        EventType::ProjectAnchored => "project.anchored",
        EventType::ProjectUnanchored => "project.unanchored",
    }
}

fn read_schema(event_name: &str) -> Value {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.extend(["schemas", "v0", &format!("{event_name}.json")]);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}
