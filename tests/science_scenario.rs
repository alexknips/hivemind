use hivemind::events::{Event, EventSource, EventType, RelationKind as EventRelationKind};
use hivemind::projector::memory::MemoryGraph;
use hivemind::projector::{project_event, GraphParams, GraphValue, GraphView, RelationKind};
use hivemind::queries::{
    derive_hypothesis_status, get_decision, get_decision_neighborhood, get_supersession_chain,
    DecisionStatus, DecisionView, HypothesisStatus, NeighborhoodRequest, NeighborhoodView,
};
use serde_json::json;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const TOPIC: &str = "science:gw150914";

const ACTOR_PIPELINE: &str = "agent:detector-pipeline";
const ACTOR_REVIEWER: &str = "person:ligo-physicist-reviewer";
const ACTOR_CALIBRATION: &str = "agent:calibration-instrument";
const ACTOR_COLLABORATION: &str = "group:ligo-virgo-collaboration-review";
const ACTOR_PUBLICATION: &str = "coordinator:publication-communications";

const H_GW: &str = "gw150914:hypothesis:astrophysical-gravitational-wave";
const H_NOISE: &str = "gw150914:hypothesis:detector-noise-glitch";
const H_INJECTION: &str = "gw150914:hypothesis:test-injection-artifact";
const H_BBH: &str = "gw150914:hypothesis:binary-black-hole-merger";

const EV_COINCIDENT: &str = "gw150914:evidence:coincident-hanford-livingston";
const EV_MORPHOLOGY: &str = "gw150914:evidence:template-and-transient-morphology";
const EV_CALIBRATION: &str = "gw150914:evidence:calibration-detector-state-checks";
const EV_INJECTION_REVIEW: &str = "gw150914:evidence:injection-artifact-review";
const EV_PARAMETERS: &str = "gw150914:evidence:parameter-estimation-source-analysis";
const EV_FOLLOWUP: &str = "gw150914:evidence:multi-messenger-followup-context";

const D_ANNOUNCE_NOW: &str = "gw150914:decision:announce-on-initial-candidate";
const D_RUN_CHECKS: &str = "gw150914:decision:run-calibration-and-injection-checks";
const D_REFUTE_NOISE: &str = "gw150914:decision:treat-detector-noise-as-refuted";
const D_PROVISIONAL_BBH: &str = "gw150914:decision:provisional-binary-black-hole";
const D_PUBLISH: &str = "gw150914:decision:publish-and-announce";

// Short source anchors, paraphrased in event text:
// LIGO science summary: https://ligo.org/science-summaries/GW150914Astro/
// Discovery paper: https://journals.aps.org/prl/abstract/10.1103/PhysRevLett.116.061102
// Search paper: https://journals.aps.org/prd/abstract/10.1103/PhysRevD.93.122003
// Detector checks: https://dcc.ligo.org/P1500238/public
// Calibration summary: https://ligo.org/science-summaries/GW150914Calibration/
#[test]
fn gw150914_scenario_tracks_uncertainty_to_publication_decision() -> TestResult<()> {
    let graph = MemoryGraph::default();
    let mut scenario = ScienceScenario::default();

    scenario.record_initial_candidate(&graph)?;
    scenario.record_uncertainty_decisions(&graph)?;

    let check_decision = get_decision_data(&graph, D_RUN_CHECKS)?;
    assert_eq!(check_decision.status, DecisionStatus::Accepted);
    assert_eq!(check_decision.evidence_ids, vec![EV_COINCIDENT.to_owned()]);
    assert_hypothesis_status(&check_decision, H_GW, HypothesisStatus::Supported)?;
    assert_hypothesis_status(&check_decision, H_NOISE, HypothesisStatus::Open)?;
    assert_hypothesis_status(&check_decision, H_INJECTION, HypothesisStatus::Open)?;
    assert_eq!(
        derive_hypothesis_status(&graph, H_BBH)?,
        HypothesisStatus::Open
    );

    let immediate_claim = get_decision_data(&graph, D_ANNOUNCE_NOW)?;
    assert_eq!(immediate_claim.status, DecisionStatus::Rejected);

    scenario.record_detector_and_review_checks(&graph)?;

    assert_eq!(
        derive_hypothesis_status(&graph, H_NOISE)?,
        HypothesisStatus::Refuted
    );
    assert_eq!(
        derive_hypothesis_status(&graph, H_INJECTION)?,
        HypothesisStatus::Refuted
    );

    let check_decision_after_review = get_decision_data(&graph, D_RUN_CHECKS)?;
    assert_eq!(check_decision_after_review.status, DecisionStatus::Accepted);
    assert_eq!(
        check_decision_after_review.evidence_ids,
        vec![EV_COINCIDENT.to_owned()],
        "later evidence must not rewrite the original decision context"
    );
    assert_hypothesis_status(
        &check_decision_after_review,
        H_NOISE,
        HypothesisStatus::Refuted,
    )?;
    assert_hypothesis_status(
        &check_decision_after_review,
        H_INJECTION,
        HypothesisStatus::Refuted,
    )?;

    scenario.record_interpretation_and_publication(&graph)?;

    let final_decision = get_decision_data(&graph, D_PUBLISH)?;
    assert_eq!(final_decision.status, DecisionStatus::Accepted);
    assert_eq!(
        final_decision.chosen_option_id.as_deref(),
        Some("gw150914:option:announce-discovery")
    );
    assert_eq!(
        final_decision.evidence_ids,
        vec![
            EV_CALIBRATION.to_owned(),
            EV_COINCIDENT.to_owned(),
            EV_INJECTION_REVIEW.to_owned(),
            EV_FOLLOWUP.to_owned(),
            EV_PARAMETERS.to_owned(),
            EV_MORPHOLOGY.to_owned(),
        ]
    );
    assert_hypothesis_status(&final_decision, H_GW, HypothesisStatus::Supported)?;
    assert_hypothesis_status(&final_decision, H_BBH, HypothesisStatus::Supported)?;
    assert_hypothesis_status(&final_decision, H_NOISE, HypothesisStatus::Refuted)?;
    assert_hypothesis_status(&final_decision, H_INJECTION, HypothesisStatus::Refuted)?;

    let provisional = get_decision_data(&graph, D_PROVISIONAL_BBH)?;
    assert_eq!(provisional.status, DecisionStatus::Superseded);
    let chain = get_supersession_chain(&graph, D_PROVISIONAL_BBH)?;
    assert_eq!(
        chain.data.decision_ids,
        vec![D_PROVISIONAL_BBH.to_owned(), D_PUBLISH.to_owned()]
    );
    assert_eq!(chain.data.input_index, 0);

    let neighborhood = get_decision_neighborhood(&graph, D_PUBLISH, &NeighborhoodRequest::all())?;
    assert_edge(
        &neighborhood.data,
        RelationKind::BasedOn,
        D_PUBLISH,
        EV_CALIBRATION,
    );
    assert_edge(
        &neighborhood.data,
        RelationKind::Supports,
        EV_CALIBRATION,
        H_GW,
    );
    assert_edge(
        &neighborhood.data,
        RelationKind::Refutes,
        EV_CALIBRATION,
        H_NOISE,
    );
    assert_edge(
        &neighborhood.data,
        RelationKind::Refutes,
        EV_INJECTION_REVIEW,
        H_INJECTION,
    );

    let actors = actor_ids(&graph)?;
    for expected in [
        ACTOR_PIPELINE,
        ACTOR_REVIEWER,
        ACTOR_CALIBRATION,
        ACTOR_COLLABORATION,
        ACTOR_PUBLICATION,
    ] {
        assert!(
            actors.iter().any(|actor| actor == expected),
            "missing actor {expected}"
        );
    }

    Ok(())
}

#[derive(Default)]
struct ScienceScenario {
    next_event_id: u64,
}

impl ScienceScenario {
    fn record_initial_candidate(&mut self, graph: &MemoryGraph) -> TestResult<()> {
        for (id, statement) in [
            (
                H_GW,
                "The coincident signal is an astrophysical gravitational-wave event.",
            ),
            (
                H_NOISE,
                "The signal is detector or instrument noise, or a transient glitch.",
            ),
            (
                H_INJECTION,
                "The signal is a test injection or analysis artifact.",
            ),
            (
                H_BBH,
                "The source is consistent with a binary black-hole merger.",
            ),
        ] {
            self.hypothesis(graph, ACTOR_REVIEWER, id, statement)?;
        }

        self.evidence(
            graph,
            ACTOR_PIPELINE,
            EV_COINCIDENT,
            "Coincident candidate observed by the Hanford and Livingston detectors on 2015-09-14.",
            "https://journals.aps.org/prl/abstract/10.1103/PhysRevLett.116.061102",
        )?;
        self.relation(
            graph,
            ACTOR_PIPELINE,
            EventRelationKind::Supports,
            EV_COINCIDENT,
            H_GW,
        )
    }

    fn record_uncertainty_decisions(&mut self, graph: &MemoryGraph) -> TestResult<()> {
        self.decision(
            graph,
            ACTOR_PIPELINE,
            D_ANNOUNCE_NOW,
            "Announce GW150914 from the initial candidate alert",
            "The first-pass candidate is strong, but review has not ruled out detector or injection alternatives.",
            &[
                "gw150914:option:announce-now",
                "gw150914:option:wait-for-review",
            ],
            "gw150914:option:announce-now",
            &[H_GW, H_NOISE, H_INJECTION, H_BBH],
            &[EV_COINCIDENT],
        )?;
        self.reject(graph, ACTOR_REVIEWER, D_ANNOUNCE_NOW)?;

        self.decision(
            graph,
            ACTOR_REVIEWER,
            D_RUN_CHECKS,
            "Run calibration and injection checks before any discovery claim",
            "The candidate remains uncertain until instrument state, calibration, and test-injection paths are checked.",
            &[
                "gw150914:option:review-before-claim",
                "gw150914:option:treat-as-noise",
            ],
            "gw150914:option:review-before-claim",
            &[H_GW, H_NOISE, H_INJECTION],
            &[EV_COINCIDENT],
        )?;
        self.accept(graph, ACTOR_COLLABORATION, D_RUN_CHECKS)
    }

    fn record_detector_and_review_checks(&mut self, graph: &MemoryGraph) -> TestResult<()> {
        self.evidence(
            graph,
            ACTOR_PIPELINE,
            EV_MORPHOLOGY,
            "Matched-filter and minimally modeled transient analyses found a compact-binary-like morphology.",
            "https://journals.aps.org/prd/abstract/10.1103/PhysRevD.93.122003",
        )?;
        self.evidence(
            graph,
            ACTOR_CALIBRATION,
            EV_CALIBRATION,
            "Calibration and detector-state checks did not identify an instrumental cause for the candidate.",
            "https://ligo.org/science-summaries/GW150914Calibration/",
        )?;
        self.evidence(
            graph,
            ACTOR_COLLABORATION,
            EV_INJECTION_REVIEW,
            "Internal review ruled out a test injection or analysis artifact as the explanation.",
            "https://dcc.ligo.org/P1500238/public",
        )?;

        self.relation(
            graph,
            ACTOR_PIPELINE,
            EventRelationKind::Supports,
            EV_MORPHOLOGY,
            H_GW,
        )?;
        self.relation(
            graph,
            ACTOR_PIPELINE,
            EventRelationKind::Supports,
            EV_MORPHOLOGY,
            H_BBH,
        )?;
        self.relation(
            graph,
            ACTOR_CALIBRATION,
            EventRelationKind::Supports,
            EV_CALIBRATION,
            H_GW,
        )?;
        self.relation(
            graph,
            ACTOR_CALIBRATION,
            EventRelationKind::Refutes,
            EV_CALIBRATION,
            H_NOISE,
        )?;
        self.relation(
            graph,
            ACTOR_COLLABORATION,
            EventRelationKind::Refutes,
            EV_INJECTION_REVIEW,
            H_INJECTION,
        )?;

        self.decision(
            graph,
            ACTOR_REVIEWER,
            D_REFUTE_NOISE,
            "Treat detector noise and injection explanations as refuted",
            "Instrument, calibration, and review checks no longer support the leading non-astrophysical explanations.",
            &[
                "gw150914:option:refute-non-astrophysical",
                "gw150914:option:continue-noise-review",
            ],
            "gw150914:option:refute-non-astrophysical",
            &[H_NOISE, H_INJECTION, H_GW],
            &[EV_CALIBRATION, EV_INJECTION_REVIEW],
        )?;
        self.accept(graph, ACTOR_COLLABORATION, D_REFUTE_NOISE)
    }

    fn record_interpretation_and_publication(&mut self, graph: &MemoryGraph) -> TestResult<()> {
        self.evidence(
            graph,
            ACTOR_REVIEWER,
            EV_PARAMETERS,
            "Parameter-estimation and source-analysis review supported a binary black-hole merger interpretation.",
            "https://journals.aps.org/prl/abstract/10.1103/PhysRevLett.116.241102",
        )?;
        self.evidence(
            graph,
            ACTOR_COLLABORATION,
            EV_FOLLOWUP,
            "Follow-up context was reviewed before coordinating the public discovery announcement.",
            "https://ligo.org/science-summaries/GW150914Astro/",
        )?;
        self.relation(
            graph,
            ACTOR_REVIEWER,
            EventRelationKind::Supports,
            EV_PARAMETERS,
            H_BBH,
        )?;

        self.decision(
            graph,
            ACTOR_REVIEWER,
            D_PROVISIONAL_BBH,
            "Accept the binary-black-hole interpretation provisionally",
            "Current source analysis supports the binary-black-hole explanation while collaboration review continues.",
            &[
                "gw150914:option:provisional-bbh",
                "gw150914:option:withhold-interpretation",
            ],
            "gw150914:option:provisional-bbh",
            &[H_GW, H_BBH],
            &[EV_MORPHOLOGY, EV_PARAMETERS],
        )?;
        self.accept(graph, ACTOR_COLLABORATION, D_PROVISIONAL_BBH)?;

        self.decision(
            graph,
            ACTOR_PUBLICATION,
            D_PUBLISH,
            "Publish and announce the GW150914 discovery",
            "The collaboration can explain why the graph moved from uncertainty to an astrophysical binary-black-hole interpretation.",
            &[
                "gw150914:option:announce-discovery",
                "gw150914:option:defer-announcement",
            ],
            "gw150914:option:announce-discovery",
            &[H_GW, H_NOISE, H_INJECTION, H_BBH],
            &[
                EV_COINCIDENT,
                EV_MORPHOLOGY,
                EV_CALIBRATION,
                EV_INJECTION_REVIEW,
                EV_PARAMETERS,
                EV_FOLLOWUP,
            ],
        )?;
        self.accept(graph, ACTOR_COLLABORATION, D_PUBLISH)?;
        self.supersede(graph, ACTOR_COLLABORATION, D_PROVISIONAL_BBH, D_PUBLISH)
    }

    fn evidence(
        &mut self,
        graph: &MemoryGraph,
        actor_id: &str,
        evidence_id: &str,
        content: &str,
        source: &str,
    ) -> TestResult<()> {
        self.push(
            graph,
            EventType::EvidenceRecorded,
            actor_id,
            json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": source,
            }),
        )
    }

    fn hypothesis(
        &mut self,
        graph: &MemoryGraph,
        actor_id: &str,
        hypothesis_id: &str,
        statement: &str,
    ) -> TestResult<()> {
        self.push(
            graph,
            EventType::HypothesisRecorded,
            actor_id,
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement,
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn decision(
        &mut self,
        graph: &MemoryGraph,
        actor_id: &str,
        decision_id: &str,
        title: &str,
        rationale: &str,
        option_ids: &[&str],
        chosen_option_id: &str,
        hypothesis_ids: &[&str],
        evidence_ids: &[&str],
    ) -> TestResult<()> {
        self.push(
            graph,
            EventType::DecisionProposed,
            actor_id,
            json!({
                "decision_id": decision_id,
                "title": title,
                "rationale": rationale,
                "topic_keys": [TOPIC],
                "option_ids": option_ids,
                "chosen_option_id": chosen_option_id,
                "hypothesis_ids": hypothesis_ids,
                "evidence_ids": evidence_ids,
            }),
        )
    }

    fn accept(&mut self, graph: &MemoryGraph, actor_id: &str, decision_id: &str) -> TestResult<()> {
        self.push(
            graph,
            EventType::DecisionAccepted,
            actor_id,
            json!({ "decision_id": decision_id }),
        )
    }

    fn reject(&mut self, graph: &MemoryGraph, actor_id: &str, decision_id: &str) -> TestResult<()> {
        self.push(
            graph,
            EventType::DecisionRejected,
            actor_id,
            json!({ "decision_id": decision_id }),
        )
    }

    fn supersede(
        &mut self,
        graph: &MemoryGraph,
        actor_id: &str,
        old_decision_id: &str,
        new_decision_id: &str,
    ) -> TestResult<()> {
        self.push(
            graph,
            EventType::DecisionSuperseded,
            actor_id,
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id,
            }),
        )
    }

    fn relation(
        &mut self,
        graph: &MemoryGraph,
        actor_id: &str,
        relation: EventRelationKind,
        from_id: &str,
        to_id: &str,
    ) -> TestResult<()> {
        self.push(
            graph,
            EventType::RelationAdded,
            actor_id,
            json!({
                "relation": relation,
                "from_id": from_id,
                "to_id": to_id,
            }),
        )
    }

    fn push(
        &mut self,
        graph: &MemoryGraph,
        event_type: EventType,
        actor_id: &str,
        payload: serde_json::Value,
    ) -> TestResult<()> {
        self.next_event_id += 1;
        project_event(
            graph,
            &Event {
                event_id: Some(self.next_event_id),
                event_uuid: Uuid::from_u128(u128::from(self.next_event_id)),
                correlation_id: Some("science-scenario:gw150914".to_owned()),
                causation_event_id: None,
                event_type,
                actor_id: actor_id.to_owned(),
                source: EventSource::Api,
                source_ref: Some("science-scenario:gw150914".to_owned()),
                payload,
                ts: None,
            },
        )?;
        Ok(())
    }
}

fn get_decision_data(graph: &MemoryGraph, decision_id: &str) -> TestResult<DecisionView> {
    get_decision(graph, decision_id)?
        .data
        .ok_or_else(|| format!("decision {decision_id} should exist").into())
}

fn assert_hypothesis_status(
    decision: &DecisionView,
    hypothesis_id: &str,
    expected: HypothesisStatus,
) -> TestResult<()> {
    let actual = decision
        .hypotheses
        .iter()
        .find(|hypothesis| hypothesis.id == hypothesis_id)
        .ok_or_else(|| format!("decision {} missing {hypothesis_id}", decision.id))?
        .status;
    assert_eq!(actual, expected, "status mismatch for {hypothesis_id}");
    Ok(())
}

fn assert_edge(view: &NeighborhoodView, relation: RelationKind, from: &str, to: &str) {
    assert!(
        view.edges
            .iter()
            .any(|edge| edge.relation == relation && edge.from == from && edge.to == to),
        "missing {relation:?} edge {from} -> {to}"
    );
}

fn actor_ids(graph: &MemoryGraph) -> TestResult<Vec<String>> {
    let rows = graph.query(
        "MATCH (node:`Actor`) RETURN node.id AS id ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    let mut ids = Vec::with_capacity(rows.len());
    for row in rows {
        match row.get("id") {
            Some(GraphValue::String(id)) => ids.push(id.clone()),
            other => return Err(format!("actor row missing string id: {other:?}").into()),
        }
    }
    ids.sort();
    Ok(ids)
}
