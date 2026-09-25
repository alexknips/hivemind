// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};

use crate::events::HypothesisKind;
use crate::projector::RelationKind;
use crate::queries::test_fixtures::{ts, Scenario};
use crate::queries::{
    get_compact_view, get_decision, get_decision_brief_at, get_decision_neighborhood,
    get_decision_outcome_at, search_decisions, DecisionStatus, NeighborhoodRequest, OutcomeReason,
    SearchDecisionRequest,
};
use crate::Result;

use super::*;

fn now() -> DateTime<Utc> {
    ts("2026-06-01T00:00:00Z")
}

/// `d:goal` (accepted) plus `d:derived`, which follows from it at capture.
fn goal_and_derived() -> Result<Scenario> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.accept("d:goal", "human:alex", "2026-01-01T00:00:01Z")?;
    let proposal = scenario.decision(
        "d:derived",
        "Use Postgres for the MVP",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
    )?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:derived",
        "d:goal",
        "agent:claude:crew",
        Some(proposal),
        "2026-01-02T00:00:01Z",
    )?;
    Ok(scenario)
}

fn outcome_of(graph: &impl GraphView, id: &str) -> Result<crate::queries::DecisionOutcome> {
    Ok(get_decision_outcome_at(graph, id, now())?
        .data
        .expect("decision exists"))
}

// ── the states ────────────────────────────────────────────────────────────────

#[test]
fn a_decision_nobody_asked_about_reads_as_nothing_declared() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision("d:1", "Use Postgres", "human:alice", "2026-01-01T00:00:00Z")?;
    let graph = scenario.graph()?;

    let grounding = grounding_of_at(&graph, "d:1", now())?;

    assert!(grounding.items.is_empty());
    assert_eq!(grounding.state, GroundingState::NothingDeclared);
    assert_eq!(grounding.dependents_count, 0);
    let outcome = outcome_of(&graph, "d:1")?;
    assert_eq!(outcome.grounding_state, GroundingState::NothingDeclared);
    assert!(outcome.reasons.contains(&OutcomeReason::ThinStructure {
        no_options: true,
        nothing_declared: true,
    }));
    Ok(())
}

#[test]
fn a_prior_decision_named_at_capture_is_at_capture_and_holds() -> Result<()> {
    let graph = goal_and_derived()?.graph()?;

    let grounding = grounding_of_at(&graph, "d:derived", now())?;

    assert_eq!(
        grounding.items,
        vec![GroundingItem {
            kind: GroundingKind::Decision,
            id: "d:goal".to_owned(),
            label: "Ship the hosted MVP".to_owned(),
            state: GroundingItemState::Holds,
            added: GroundingAdded::AtCapture,
            would_change_if: None,
        }]
    );
    assert_eq!(grounding.state, GroundingState::Grounded);
    // ...and the premise knows who rests on it.
    assert_eq!(
        grounding_of_at(&graph, "d:goal", now())?.dependents_count,
        1
    );

    // A premise link is a positive answer: the derived decision is no longer "nothing declared".
    let outcome = outcome_of(&graph, "d:derived")?;
    assert!(outcome.held_up);
    assert_eq!(outcome.grounding_state, GroundingState::Grounded);
    assert!(!outcome.reasons.iter().any(|reason| matches!(
        reason,
        OutcomeReason::ThinStructure {
            nothing_declared: true,
            ..
        }
    )));
    Ok(())
}

#[test]
fn a_premise_attributed_later_carries_who_and_when() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision(
        "d:old",
        "Use Postgres",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
    )?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:old",
        "d:goal",
        "human:alex",
        None,
        "2026-03-05T10:00:00Z",
    )?;
    let graph = scenario.graph()?;

    let grounding = grounding_of_at(&graph, "d:old", now())?;

    assert_eq!(grounding.items.len(), 1);
    assert_eq!(
        grounding.items[0].added,
        GroundingAdded::Later {
            actor_id: Some("human:alex".to_owned()),
            ts: Some(ts("2026-03-05T10:00:00Z")),
        }
    );
    Ok(())
}

#[test]
fn evidence_carries_its_content_source_and_recorded_time() -> Result<()> {
    let scenario = Scenario::new();
    scenario.evidence(
        "e:1",
        "27 of 27 captures carry no premise",
        Some("mayor audit 2026-09-22"),
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision_with(
        "d:1",
        "Ask what a decision rests on",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
        false,
        &["e:1"],
        &[],
        None,
    )?;
    let graph = scenario.graph()?;

    let grounding = grounding_of_at(&graph, "d:1", now())?;

    assert_eq!(grounding.items.len(), 1);
    let item = &grounding.items[0];
    assert_eq!(item.kind, GroundingKind::Evidence);
    assert_eq!(item.label, "27 of 27 captures carry no premise");
    assert_eq!(
        item.state,
        GroundingItemState::Recorded {
            ts: Some(ts("2026-01-01T00:00:00Z")),
            source: Some("mayor audit 2026-09-22".to_owned()),
        }
    );
    assert_eq!(item.added, GroundingAdded::AtCapture);
    assert_eq!(grounding.state, GroundingState::Grounded);
    Ok(())
}

#[test]
fn an_assumption_is_open_supported_or_refuted_and_a_refuted_one_makes_the_decision_stale(
) -> Result<()> {
    let scenario = Scenario::new();
    for id in ["h:open", "h:supported", "h:refuted"] {
        scenario.hypothesis(
            id,
            &format!("Assumption {id}"),
            "assumption",
            None,
            "2026-01-01T00:00:00Z",
        )?;
        scenario.decision_with(
            &format!("d:{id}"),
            &format!("Decision on {id}"),
            "human:alice",
            "2026-01-02T00:00:00Z",
            true,
            &[],
            &[id],
            None,
        )?;
    }
    scenario.evidence("e:1", "Observed it", None, "2026-01-03T00:00:00Z")?;
    scenario.relation(
        "SUPPORTS",
        "e:1",
        "h:supported",
        "agent:tester",
        None,
        "2026-01-03T00:00:01Z",
    )?;
    scenario.relation(
        "REFUTES",
        "e:1",
        "h:refuted",
        "agent:tester",
        None,
        "2026-01-03T00:00:02Z",
    )?;
    let graph = scenario.graph()?;

    let states: Vec<(GroundingKind, GroundingItemState, GroundingAdded)> =
        ["h:open", "h:supported", "h:refuted"]
            .into_iter()
            .map(|id| {
                let items = grounding_of_at(&graph, &format!("d:{id}"), now())?.items;
                assert_eq!(items.len(), 1, "{id}");
                Ok((
                    items[0].kind,
                    items[0].state.clone(),
                    items[0].added.clone(),
                ))
            })
            .collect::<Result<_>>()?;
    assert_eq!(
        states,
        vec![
            (
                GroundingKind::Assumption,
                GroundingItemState::Open,
                GroundingAdded::AtCapture
            ),
            (
                GroundingKind::Assumption,
                GroundingItemState::Supported,
                GroundingAdded::AtCapture
            ),
            (
                GroundingKind::Assumption,
                GroundingItemState::Refuted,
                GroundingAdded::AtCapture
            ),
        ]
    );

    assert!(outcome_of(&graph, "d:h:open")?.held_up);
    assert!(outcome_of(&graph, "d:h:supported")?.held_up);
    let refuted = outcome_of(&graph, "d:h:refuted")?;
    assert!(!refuted.held_up);
    assert!(refuted.stale_premises);
    assert_eq!(refuted.refuted_hypothesis_ids, vec!["h:refuted".to_owned()]);
    Ok(())
}

#[test]
fn a_bet_is_open_held_or_failed_and_only_a_failed_one_makes_the_decision_stale() -> Result<()> {
    let scenario = Scenario::new();
    scenario.hypothesis(
        "h:open",
        "Bet open",
        "bet",
        Some("2026-12-01T00:00:00Z"),
        "2026-01-01T00:00:00Z",
    )?;
    scenario.hypothesis(
        "h:held",
        "Bet held",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:00Z",
    )?;
    scenario.hypothesis(
        "h:failed",
        "Bet failed",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:00Z",
    )?;
    for id in ["h:open", "h:held", "h:failed"] {
        scenario.decision_with(
            &format!("d:{id}"),
            &format!("Decision on {id}"),
            "human:alice",
            "2026-01-02T00:00:00Z",
            false,
            &[],
            &[id],
            None,
        )?;
    }
    scenario.evidence("e:1", "Checked it", None, "2026-03-01T00:00:00Z")?;
    scenario.relation(
        "SUPPORTS",
        "e:1",
        "h:held",
        "agent:tester",
        None,
        "2026-03-01T00:00:01Z",
    )?;
    scenario.relation(
        "REFUTES",
        "e:1",
        "h:failed",
        "agent:tester",
        None,
        "2026-03-01T00:00:02Z",
    )?;
    let graph = scenario.graph()?;

    let state_of = |id: &str| -> Result<(GroundingKind, GroundingItemState, GroundingState)> {
        let grounding = grounding_of_at(&graph, &format!("d:{id}"), now())?;
        Ok((
            grounding.items[0].kind,
            grounding.items[0].state.clone(),
            grounding.state,
        ))
    };
    assert_eq!(
        state_of("h:open")?,
        (
            GroundingKind::Bet,
            GroundingItemState::BetOpen {
                check_by: Some(ts("2026-12-01T00:00:00Z")),
                overdue: false,
            },
            GroundingState::Bet
        )
    );
    assert_eq!(state_of("h:held")?.1, GroundingItemState::BetHeld);
    assert_eq!(state_of("h:failed")?.1, GroundingItemState::BetFailed);

    // A bet is a positive answer, not thin; only the failed one is stale.
    for id in ["h:open", "h:held"] {
        let outcome = outcome_of(&graph, &format!("d:{id}"))?;
        assert!(outcome.held_up, "{id}");
        assert!(outcome.unchecked.is_empty(), "{id}");
        assert!(!outcome.reasons.iter().any(|reason| matches!(
            reason,
            OutcomeReason::ThinStructure {
                nothing_declared: true,
                ..
            }
        )));
    }
    let failed = outcome_of(&graph, "d:h:failed")?;
    assert!(!failed.held_up);
    assert!(failed.stale_premises);
    Ok(())
}

#[test]
fn an_overdue_bet_is_reported_unchecked_without_flipping_held_up() -> Result<()> {
    let scenario = Scenario::new();
    scenario.hypothesis(
        "h:late",
        "Nobody will notice",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision_with(
        "d:1",
        "Ship it anyway",
        "human:alice",
        "2026-01-02T00:00:00Z",
        false,
        &[],
        &["h:late"],
        None,
    )?;
    let graph = scenario.graph()?;

    let outcome = outcome_of(&graph, "d:1")?;
    assert!(outcome.held_up, "attention, not staleness");
    assert!(!outcome.stale_premises);
    assert_eq!(
        outcome.unchecked,
        vec![UncheckedBet {
            hypothesis_id: "h:late".to_owned(),
            check_by: ts("2026-02-01T00:00:00Z"),
        }]
    );
    let items = grounding_of_at(&graph, "d:1", now())?.items;
    assert_eq!(
        items[0].state,
        GroundingItemState::BetOpen {
            check_by: Some(ts("2026-02-01T00:00:00Z")),
            overdue: true,
        }
    );
    // Before the check date the same bet is not overdue.
    let early = get_decision_outcome_at(&graph, "d:1", ts("2026-01-15T00:00:00Z"))?
        .data
        .expect("decision exists");
    assert!(early.unchecked.is_empty());
    Ok(())
}

#[test]
fn a_decision_can_be_grounded_and_carry_a_bet_at_once() -> Result<()> {
    let scenario = Scenario::new();
    scenario.evidence(
        "e:1",
        "A measurement",
        Some("bench run 12"),
        "2026-01-01T00:00:00Z",
    )?;
    scenario.hypothesis("h:bet", "It scales", "bet", None, "2026-01-01T00:00:00Z")?;
    scenario.decision_with(
        "d:1",
        "Adopt it",
        "human:alice",
        "2026-01-02T00:00:00Z",
        false,
        &["e:1"],
        &["h:bet"],
        None,
    )?;
    let graph = scenario.graph()?;

    let grounding = grounding_of_at(&graph, "d:1", now())?;

    assert_eq!(grounding.state, GroundingState::Grounded);
    let kinds: Vec<GroundingKind> = grounding.items.iter().map(|item| item.kind).collect();
    assert_eq!(kinds, vec![GroundingKind::Evidence, GroundingKind::Bet]);
    Ok(())
}

#[test]
fn a_hypothesis_added_later_is_attributed_to_whoever_added_it() -> Result<()> {
    let scenario = Scenario::new();
    scenario.hypothesis(
        "h:later",
        "Load stays flat",
        "assumption",
        None,
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision_with(
        "d:1",
        "Adopt it",
        "human:alice",
        "2026-01-02T00:00:00Z",
        true,
        &[],
        &[],
        None,
    )?;
    // Grounded later: hangs off the decision itself, with no causing proposal.
    scenario.relation(
        "ASSUMES",
        "d:1",
        "h:later",
        "human:bob",
        None,
        "2026-04-01T00:00:00Z",
    )?;
    let graph = scenario.graph()?;

    let items = grounding_of_at(&graph, "d:1", now())?.items;

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, GroundingKind::Assumption);
    assert_eq!(
        items[0].added,
        GroundingAdded::Later {
            actor_id: Some("human:bob".to_owned()),
            ts: Some(ts("2026-04-01T00:00:00Z")),
        }
    );
    Ok(())
}

// ── staleness ─────────────────────────────────────────────────────────────────

#[test]
fn a_superseded_premise_makes_the_derived_decision_stale() -> Result<()> {
    let scenario = goal_and_derived()?;
    scenario.decision(
        "d:goal-2",
        "Ship the self-hosted MVP",
        "human:alex",
        "2026-02-01T00:00:00Z",
    )?;
    scenario.supersede("d:goal", "d:goal-2", "human:alex", "2026-02-01T00:00:01Z")?;
    let graph = scenario.graph()?;

    let outcome = outcome_of(&graph, "d:derived")?;
    assert!(!outcome.held_up);
    assert!(outcome.stale_premises);
    assert!(outcome.reasons.contains(&OutcomeReason::PremiseSuperseded {
        decision_id: "d:goal".to_owned(),
        by_id: "d:goal-2".to_owned(),
    }));
    // The premise itself is superseded; the derived decision is not.
    assert!(!outcome.superseded);

    let items = grounding_of_at(&graph, "d:derived", now())?.items;
    assert_eq!(
        items[0].state,
        GroundingItemState::Superseded {
            by_id: "d:goal-2".to_owned(),
            by_label: Some("Ship the self-hosted MVP".to_owned()),
            by_recorded_at: Some(ts("2026-02-01T00:00:00Z")),
        }
    );

    let brief = get_decision_brief_at(&graph, "d:derived", now())?
        .data
        .expect("decision exists");
    assert!(!brief.still_holds.held_up);
    assert_eq!(brief.rests_on, items);
    Ok(())
}

#[test]
fn a_rejected_premise_makes_the_derived_decision_stale() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    let proposal = scenario.decision(
        "d:derived",
        "Use Postgres",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
    )?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:derived",
        "d:goal",
        "agent:claude:crew",
        Some(proposal),
        "2026-01-02T00:00:01Z",
    )?;
    scenario.reject("d:goal", "human:bob", "2026-01-03T00:00:00Z")?;
    let graph = scenario.graph()?;

    let outcome = outcome_of(&graph, "d:derived")?;

    assert!(!outcome.held_up);
    assert!(outcome.stale_premises);
    assert!(outcome.reasons.contains(&OutcomeReason::PremiseRejected {
        decision_id: "d:goal".to_owned(),
    }));
    assert_eq!(
        grounding_of_at(&graph, "d:derived", now())?.items[0].state,
        GroundingItemState::Rejected
    );
    Ok(())
}

#[test]
fn a_contested_premise_is_shown_but_does_not_flip_held_up() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.accept("d:goal", "human:alex", "2026-01-01T00:00:01Z")?;
    scenario.reject("d:goal", "human:bob", "2026-01-01T00:00:02Z")?;
    let proposal = scenario.decision(
        "d:derived",
        "Use Postgres",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
    )?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:derived",
        "d:goal",
        "agent:claude:crew",
        Some(proposal),
        "2026-01-02T00:00:01Z",
    )?;
    let graph = scenario.graph()?;

    assert!(outcome_of(&graph, "d:derived")?.held_up);
    assert_eq!(
        grounding_of_at(&graph, "d:derived", now())?.items[0].state,
        GroundingItemState::Contested
    );
    Ok(())
}

// ── the record around it ──────────────────────────────────────────────────────

#[test]
fn the_neighborhood_shows_what_a_decision_rests_on_and_what_rests_on_it() -> Result<()> {
    let graph = goal_and_derived()?.graph()?;

    let derived = get_decision_neighborhood(&graph, "d:derived", &NeighborhoodRequest::all())?.data;
    assert!(derived
        .edges
        .iter()
        .any(|edge| edge.relation == RelationKind::FollowsFrom
            && edge.from == "d:derived"
            && edge.to == "d:goal"));
    assert!(derived
        .nodes
        .iter()
        .any(|node| node.id == "d:goal" && node.decision_status == Some(DecisionStatus::Accepted)));

    let goal = get_decision_neighborhood(&graph, "d:goal", &NeighborhoodRequest::all())?.data;
    assert!(goal
        .edges
        .iter()
        .any(|edge| edge.relation == RelationKind::FollowsFrom
            && edge.from == "d:derived"
            && edge.to == "d:goal"));
    Ok(())
}

#[test]
fn decision_views_carry_premise_ids_and_hypothesis_kinds() -> Result<()> {
    let scenario = goal_and_derived()?;
    scenario.hypothesis(
        "h:bet",
        "Users will come",
        "bet",
        Some("2026-10-15T00:00:00Z"),
        "2026-01-03T00:00:00Z",
    )?;
    scenario.relation(
        "ASSUMES",
        "d:derived",
        "h:bet",
        "human:alex",
        None,
        "2026-01-03T00:00:01Z",
    )?;
    let graph = scenario.graph()?;

    let view = get_decision(&graph, "d:derived")?
        .data
        .expect("decision exists");

    assert_eq!(view.premise_decision_ids, vec!["d:goal".to_owned()]);
    assert_eq!(view.hypotheses.len(), 1);
    assert_eq!(view.hypotheses[0].kind, HypothesisKind::Bet);
    assert_eq!(
        view.hypotheses[0].check_by,
        Some(ts("2026-10-15T00:00:00Z"))
    );
    assert_eq!(view.grounding_state(), GroundingState::Grounded);
    assert_eq!(
        view.rests_on_clause(),
        "rests on: decision x1, bet (check by 2026-10-15)"
    );

    // The premise itself carries no premise of its own.
    let goal = get_decision(&graph, "d:goal")?
        .data
        .expect("decision exists");
    assert!(goal.premise_decision_ids.is_empty());
    assert_eq!(goal.grounding_state(), GroundingState::NothingDeclared);
    assert_eq!(goal.rests_on_clause(), "rests on: nothing declared");
    Ok(())
}

#[test]
fn search_and_compact_view_carry_premise_ids_and_grounding_state() -> Result<()> {
    let scenario = goal_and_derived()?;
    scenario.decision(
        "d:goal-2",
        "Ship the self-hosted MVP",
        "human:alex",
        "2026-02-01T00:00:00Z",
    )?;
    scenario.supersede("d:goal", "d:goal-2", "human:alex", "2026-02-01T00:00:01Z")?;
    let graph = scenario.graph()?;

    let search = search_decisions(
        &graph,
        &SearchDecisionRequest {
            query: Some("Use Postgres".to_owned()),
            topic_keys: vec![],
            statuses: vec![],
            actor_ids: vec![],
            sources: vec![],
            since: None,
            until: None,
            limit: 10,
            cursor: None,
            project: None,
        },
    )?
    .data;
    let hit = search
        .items
        .iter()
        .find(|item| item.decision.id == "d:derived")
        .expect("derived decision found");
    assert_eq!(hit.decision.premise_decision_ids, vec!["d:goal".to_owned()]);
    assert_eq!(hit.graph_context.grounding_state, GroundingState::Grounded);

    let compact = get_compact_view(&graph, "d:derived")?
        .data
        .expect("decision exists");
    assert_eq!(
        compact.decision.premise_decision_ids,
        vec!["d:goal".to_owned()]
    );
    assert_eq!(compact.grounding_state, GroundingState::Grounded);
    assert_eq!(
        compact.premises,
        vec![crate::queries::PremiseSummaryView {
            decision_id: "d:goal".to_owned(),
            status: DecisionStatus::Superseded,
        }]
    );
    assert_eq!(compact.dependents_count, 0);
    Ok(())
}

#[test]
fn grounding_state_and_digest_clause_are_derived_from_what_is_declared() {
    use HypothesisKind::{Assumption, Bet};
    assert_eq!(
        grounding_state_of(0, 0, []),
        GroundingState::NothingDeclared
    );
    assert_eq!(grounding_state_of(0, 0, [Bet]), GroundingState::Bet);
    assert_eq!(
        grounding_state_of(0, 0, [Assumption]),
        GroundingState::Grounded
    );
    assert_eq!(grounding_state_of(1, 0, [Bet]), GroundingState::Grounded);
    assert_eq!(grounding_state_of(0, 2, []), GroundingState::Grounded);

    assert_eq!(rests_on_clause(0, 0, 0, &[]), "rests on: nothing declared");
    assert_eq!(
        rests_on_clause(1, 2, 0, &[Some(ts("2026-10-15T00:00:00Z"))]),
        "rests on: decision x1, evidence x2, bet (check by 2026-10-15)"
    );
    assert_eq!(
        rests_on_clause(0, 0, 1, &[None]),
        "rests on: assumption x1, bet"
    );
}

#[test]
fn item_state_and_provenance_read_the_same_in_every_text_surface() {
    assert_eq!(
        GroundingItemState::Holds.describe().as_deref(),
        Some("still holds")
    );
    assert_eq!(
        GroundingItemState::Superseded {
            by_id: "d:2".to_owned(),
            by_label: Some("M6 rescoped".to_owned()),
            by_recorded_at: Some(ts("2026-09-21T09:00:00Z")),
        }
        .describe()
        .as_deref(),
        Some("SUPERSEDED 2026-09-21 by \"M6 rescoped\"")
    );
    assert_eq!(
        GroundingItemState::BetOpen {
            check_by: Some(ts("2026-10-15T00:00:00Z")),
            overdue: true,
        }
        .describe()
        .as_deref(),
        Some("check by 2026-10-15 (OVERDUE), open")
    );
    assert_eq!(
        GroundingItemState::BetOpen {
            check_by: None,
            overdue: false,
        }
        .describe()
        .as_deref(),
        Some("no check date, open")
    );
    assert_eq!(
        GroundingItemState::Recorded {
            ts: None,
            source: None,
        }
        .describe(),
        None
    );
    assert_eq!(GroundingAdded::AtCapture.describe(), "at capture");
    assert_eq!(
        GroundingAdded::Later {
            actor_id: Some("human:alex".to_owned()),
            ts: Some(ts("2026-09-22T09:00:00Z")),
        }
        .describe(),
        "attributed later by human:alex on 2026-09-22"
    );
}

#[test]
fn item_state_serializes_with_a_flat_state_tag() -> Result<()> {
    let item = GroundingItem {
        kind: GroundingKind::Bet,
        id: "h:1".to_owned(),
        label: "It scales".to_owned(),
        state: GroundingItemState::BetOpen {
            check_by: Some(ts("2026-10-15T00:00:00Z")),
            overdue: false,
        },
        added: GroundingAdded::AtCapture,
        would_change_if: None,
    };

    let json = serde_json::to_value(&item).expect("item serializes");

    assert_eq!(json["kind"], "bet");
    assert_eq!(json["state"], "open");
    assert_eq!(json["overdue"], false);
    assert_eq!(json["added"]["when"], "at_capture");
    Ok(())
}
