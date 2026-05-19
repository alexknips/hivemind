use hivemind::events::EventType;
use hivemind::ledger::{EventLedger, InMemoryEventLedger};
use hivemind::projector::memory::MemoryGraph;
use hivemind::projector::rebuild_graph;
use hivemind::queries::{
    get_decision, get_relevant_decisions, get_supersession_chain, DecisionStatus, HypothesisStatus,
};

#[path = "support/organizational_scenarios.rs"]
mod organizational_scenarios;

use organizational_scenarios::{scenario_events, scenarios, OrganizationalScenario};

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn organizational_scenarios_are_named_and_mixed_actor() {
    let scenarios = scenarios();

    assert_eq!(
        scenario_names(&scenarios),
        vec![
            "production-incident-response",
            "remote-db-architecture-choice",
            "product-launch-readiness",
            "security-vulnerability-triage",
            "hiring-capacity-planning",
        ]
    );

    for scenario in scenarios {
        assert!(
            scenario
                .events
                .iter()
                .any(|event| event.actor_id.starts_with("human:")),
            "{} should include a human actor",
            scenario.name
        );
        assert!(
            scenario
                .events
                .iter()
                .any(|event| event.actor_id.starts_with("agent:")),
            "{} should include an agent actor",
            scenario.name
        );
    }
}

#[test]
fn organizational_scenarios_cover_required_event_shapes() {
    assert_eq!(scenario_events(), scenario_events());

    for scenario in scenarios() {
        assert!(
            has_event(&scenario, EventType::DecisionProposed),
            "{} should include decisions",
            scenario.name
        );
        assert!(
            has_event(&scenario, EventType::EvidenceRecorded),
            "{} should include evidence",
            scenario.name
        );
        assert!(
            has_event(&scenario, EventType::HypothesisRecorded),
            "{} should include hypotheses",
            scenario.name
        );
        assert!(
            has_event(&scenario, EventType::RelationAdded),
            "{} should include explicit relation events",
            scenario.name
        );
        assert!(
            scenario.events.iter().any(decision_has_options),
            "{} should include decision options",
            scenario.name
        );
        assert!(
            scenario
                .events
                .iter()
                .all(|event| event.event_uuid != uuid::Uuid::nil()),
            "{} should use deterministic non-empty event ids",
            scenario.name
        );
    }
}

#[test]
fn organizational_queries_preserve_final_decision_context() -> TestResult<()> {
    let graph = project_scenarios()?;

    for scenario in scenarios() {
        let response = get_decision(&graph, scenario.final_decision_id)?;
        let decision = response.data.ok_or_else(|| {
            missing_fixture_error(format!(
                "missing final decision {}",
                scenario.final_decision_id
            ))
        })?;

        assert_eq!(
            decision.status,
            DecisionStatus::Accepted,
            "{} final status should be accepted",
            scenario.name
        );
        assert!(
            decision.option_ids.len() >= 2,
            "{} final decision should preserve considered options",
            scenario.name
        );
        assert!(
            decision
                .evidence_ids
                .contains(&scenario.final_evidence_id.to_owned()),
            "{} final decision should preserve evidence {}",
            scenario.name,
            scenario.final_evidence_id
        );
        let hypothesis = decision
            .hypotheses
            .iter()
            .find(|hypothesis| hypothesis.id == scenario.final_hypothesis_id)
            .ok_or_else(|| {
                missing_fixture_error(format!(
                    "{} final decision missing hypothesis {}",
                    scenario.name, scenario.final_hypothesis_id
                ))
            })?;
        assert!(
            matches!(
                hypothesis.status,
                HypothesisStatus::Supported | HypothesisStatus::Refuted
            ),
            "{} final hypothesis should have evidence-derived status",
            scenario.name
        );

        let relevant =
            get_relevant_decisions(&graph, scenario.topic, Some(DecisionStatus::Accepted))?;
        assert!(
            relevant
                .data
                .iter()
                .any(|decision| decision.id == scenario.final_decision_id),
            "{} accepted topic filter should include final decision",
            scenario.name
        );
    }

    Ok(())
}

#[test]
fn organizational_queries_surface_contested_rejected_and_superseded_paths() -> TestResult<()> {
    let graph = project_scenarios()?;

    for scenario in scenarios() {
        if let Some(decision_id) = scenario.contested_decision_id {
            let decision = get_decision(&graph, decision_id)?.data.ok_or_else(|| {
                missing_fixture_error(format!(
                    "missing contested decision {decision_id} for {}",
                    scenario.name
                ))
            })?;
            assert_eq!(
                decision.status,
                DecisionStatus::Contested,
                "{} should preserve contested status",
                scenario.name
            );
            let filtered =
                get_relevant_decisions(&graph, scenario.topic, Some(DecisionStatus::Contested))?;
            assert!(
                filtered
                    .data
                    .iter()
                    .any(|decision| decision.id == decision_id),
                "{} contested topic filter should include {}",
                scenario.name,
                decision_id
            );
        }

        if let Some(decision_id) = scenario.rejected_decision_id {
            let decision = get_decision(&graph, decision_id)?.data.ok_or_else(|| {
                missing_fixture_error(format!(
                    "missing rejected decision {decision_id} for {}",
                    scenario.name
                ))
            })?;
            assert_eq!(
                decision.status,
                DecisionStatus::Rejected,
                "{} should preserve rejected status",
                scenario.name
            );
            let filtered =
                get_relevant_decisions(&graph, scenario.topic, Some(DecisionStatus::Rejected))?;
            assert!(
                filtered
                    .data
                    .iter()
                    .any(|decision| decision.id == decision_id),
                "{} rejected topic filter should include {}",
                scenario.name,
                decision_id
            );
        }

        if let Some(decision_id) = scenario.supersession_probe_id {
            let decision = get_decision(&graph, decision_id)?.data.ok_or_else(|| {
                missing_fixture_error(format!(
                    "missing superseded decision {decision_id} for {}",
                    scenario.name
                ))
            })?;
            assert_eq!(
                decision.status,
                DecisionStatus::Superseded,
                "{} should preserve superseded status",
                scenario.name
            );
            let chain = get_supersession_chain(&graph, decision_id)?;
            assert_eq!(
                chain.data.decision_ids,
                scenario
                    .supersession_chain
                    .iter()
                    .map(|id| (*id).to_owned())
                    .collect::<Vec<_>>(),
                "{} should preserve supersession chain",
                scenario.name
            );
        }
    }

    Ok(())
}

fn project_scenarios() -> TestResult<MemoryGraph> {
    let ledger = InMemoryEventLedger::new();
    for event in scenario_events() {
        ledger.append(event)?;
    }

    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok(graph)
}

fn scenario_names(scenarios: &[OrganizationalScenario]) -> Vec<&'static str> {
    scenarios.iter().map(|scenario| scenario.name).collect()
}

fn has_event(scenario: &OrganizationalScenario, event_type: EventType) -> bool {
    scenario
        .events
        .iter()
        .any(|event| event.event_type == event_type)
}

fn decision_has_options(event: &hivemind::events::Event) -> bool {
    event.event_type == EventType::DecisionProposed
        && event
            .payload
            .get("option_ids")
            .and_then(|value| value.as_array())
            .is_some_and(|options| options.len() >= 2)
}

fn missing_fixture_error(message: String) -> std::io::Error {
    std::io::Error::other(message)
}
