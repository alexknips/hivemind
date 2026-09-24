// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::projector::memory::MemoryGraph;
use crate::queries::test_fixtures::{ts, Scenario};
use crate::queries::{
    get_compact_view, get_decision_brief_at, get_situational_decisions, QueryContext,
    SituationalRequest,
};
use crate::Result;

use super::*;

fn brief_text(graph: &MemoryGraph, decision_id: &str) -> Result<String> {
    let brief = get_decision_brief_at(graph, decision_id, ts("2026-06-01T00:00:00Z"))?.data;
    Ok(render_decision_brief_summary(&brief))
}

/// `d:goal` (accepted by Alex) and `d:derived`, which follows from it, cites evidence and
/// carries a bet, all named at capture, with the decider's stated confidence.
fn grounded_scenario() -> Result<Scenario> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.accept("d:goal", "human:alex", "2026-01-01T00:00:01Z")?;
    scenario.evidence(
        "e:audit",
        "27 of 27 captures carry no premise",
        Some("mayor audit 2026-09-22"),
        "2026-01-01T00:00:02Z",
    )?;
    scenario.hypothesis(
        "h:bet",
        "Agents answer the question instead of skipping capture",
        "bet",
        Some("2026-10-15T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    let proposal = scenario.decision_with(
        "d:derived",
        "Premises are asked at capture",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
        false,
        &["e:audit"],
        &["h:bet"],
        Some("high"),
    )?;
    scenario.accept("d:derived", "human:alex", "2026-01-02T00:00:01Z")?;
    scenario.relation(
        "FOLLOWS_FROM",
        "d:derived",
        "d:goal",
        "agent:claude:crew",
        Some(proposal),
        "2026-01-02T00:00:02Z",
    )?;
    Ok(scenario)
}

#[test]
fn brief_says_nothing_declared_for_a_decision_nobody_asked_about() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:logo",
        "Pick a logo",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;

    let text = brief_text(&scenario.graph()?, "d:logo")?;

    assert!(
        text.contains(
            "  rests on: nothing declared (never asked; add with hivemind ground \"Pick a logo\" --rests-on-decision \"...\")\n"
        ),
        "{text}"
    );
    assert!(text.contains("  still holds: yes\n"), "{text}");
    assert!(!text.contains("confidence at capture"), "{text}");
    assert!(!text.contains("rest on this"), "{text}");
    Ok(())
}

#[test]
fn brief_lists_each_thing_it_rests_on_with_its_state_and_provenance() -> Result<()> {
    let graph = grounded_scenario()?.graph()?;

    let text = brief_text(&graph, "d:derived")?;

    for expected in [
        "  rests on:\n",
        "    decision   \"Ship the hosted MVP\"  still holds  (at capture)\n",
        "    evidence   \"27 of 27 captures carry no premise\" (mayor audit 2026-09-22)  (at capture)\n",
        "    bet        \"Agents answer the question instead of skipping capture\"  check by 2026-10-15, open  (at capture)\n",
        "  still holds: yes\n",
        "  confidence at capture: high (decider's words)\n",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }

    // The premise is told who rests on it.
    let goal = brief_text(&graph, "d:goal")?;
    assert!(goal.contains("  1 decision rests on this\n"), "{goal}");
    Ok(())
}

#[test]
fn brief_attributes_a_later_premise_to_whoever_added_it() -> Result<()> {
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

    let text = brief_text(&scenario.graph()?, "d:old")?;

    assert!(
        text.contains(
            "    decision   \"Ship the hosted MVP\"  still holds  (attributed later by human:alex on 2026-03-05)\n"
        ),
        "{text}"
    );
    Ok(())
}

#[test]
fn brief_says_no_when_a_premise_was_superseded() -> Result<()> {
    let scenario = grounded_scenario()?;
    scenario.decision(
        "d:goal-2",
        "Ship the self-hosted MVP",
        "human:alex",
        "2026-02-01T00:00:00Z",
    )?;
    scenario.supersede("d:goal", "d:goal-2", "human:alex", "2026-02-01T00:00:01Z")?;

    let text = brief_text(&scenario.graph()?, "d:derived")?;

    for expected in [
        "    decision   \"Ship the hosted MVP\"  SUPERSEDED 2026-02-01 by \"Ship the self-hosted MVP\"  (at capture)\n",
        "  still holds: NO: premise superseded\n",
        "    - follows from \"Ship the hosted MVP\", which was superseded by \"Ship the self-hosted MVP\"\n",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    Ok(())
}

#[test]
fn brief_says_no_when_a_premise_was_rejected() -> Result<()> {
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

    let text = brief_text(&scenario.graph()?, "d:derived")?;

    assert!(
        text.contains("  still holds: NO: premise rejected\n"),
        "{text}"
    );
    assert!(
        text.contains("    decision   \"Ship the hosted MVP\"  REJECTED  (at capture)\n"),
        "{text}"
    );
    Ok(())
}

#[test]
fn brief_puts_a_lone_bet_on_one_line() -> Result<()> {
    let scenario = Scenario::new();
    scenario.hypothesis(
        "h:judgement",
        "Judgement call: Pick a logo",
        "bet",
        None,
        "2026-01-01T00:00:00Z",
    )?;
    scenario.decision_with(
        "d:logo",
        "Pick a logo",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
        false,
        &[],
        &["h:judgement"],
        None,
    )?;
    scenario.accept("d:logo", "human:alex", "2026-01-02T00:00:01Z")?;

    let text = brief_text(&scenario.graph()?, "d:logo")?;

    assert!(
        text.contains(
            "  rests on: a bet, declared at capture by human:alex: \"Judgement call: Pick a logo\", no check date\n"
        ),
        "{text}"
    );
    Ok(())
}

#[test]
fn brief_reports_an_overdue_bet_as_unchecked_while_it_still_holds() -> Result<()> {
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

    let text = brief_text(&scenario.graph()?, "d:1")?;

    assert!(text.contains("  still holds: yes\n"), "{text}");
    assert!(
        text.contains(
            "  unchecked: bet \"Nobody will notice\" — check date 2026-02-01 has passed, no evidence recorded either way\n"
        ),
        "{text}"
    );
    Ok(())
}

#[test]
fn situational_summary_names_why_a_decision_is_stale() -> Result<()> {
    let scenario = grounded_scenario()?;
    scenario.decision(
        "d:goal-2",
        "Ship the self-hosted MVP",
        "human:alex",
        "2026-02-01T00:00:00Z",
    )?;
    scenario.supersede("d:goal", "d:goal-2", "human:alex", "2026-02-01T00:00:01Z")?;
    let graph = scenario.graph()?;

    let results = get_situational_decisions(
        &QueryContext::local(),
        &graph,
        scenario.ledger(),
        &SituationalRequest {
            paths: vec!["grounding".to_owned()],
            limit: 10,
            ..SituationalRequest::default()
        },
    )?
    .data;
    let text = render_situational_summary(&results);

    let derived_row = text
        .lines()
        .find(|line| line.contains("d:derived"))
        .expect("derived decision listed");
    assert!(
        derived_row.contains("STALE(premise superseded)"),
        "{derived_row}"
    );
    // A decision that still holds is unchanged.
    let holding_row = text
        .lines()
        .find(|line| line.contains("d:goal-2"))
        .expect("superseding decision listed");
    assert!(holding_row.contains("\tholds\t"), "{holding_row}");
    Ok(())
}

#[test]
fn compact_view_summary_says_what_it_rests_on_and_flags_a_premise_that_no_longer_stands(
) -> Result<()> {
    let scenario = grounded_scenario()?;
    scenario.decision(
        "d:goal-2",
        "Ship the self-hosted MVP",
        "human:alex",
        "2026-02-01T00:00:00Z",
    )?;
    scenario.supersede("d:goal", "d:goal-2", "human:alex", "2026-02-01T00:00:01Z")?;
    let graph = scenario.graph()?;

    let view = get_compact_view(&graph, "d:derived")?.data;
    let text = render_compact_view_summary(&view);

    assert!(
        text.contains("  rests on: decision x1, evidence x1, bet (check by 2026-10-15)\n"),
        "{text}"
    );
    assert!(
        text.contains("  STALE: follows from d:goal, which is superseded\n"),
        "{text}"
    );
    Ok(())
}

#[test]
fn compact_view_summary_says_nothing_declared_for_a_decision_nobody_asked_about() -> Result<()> {
    let scenario = Scenario::new();
    scenario.decision(
        "d:logo",
        "Pick a logo",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;

    let view = get_compact_view(&scenario.graph()?, "d:logo")?.data;
    let text = render_compact_view_summary(&view);

    assert!(
        text.contains("  rests on: nothing declared (never asked; add with hivemind ground"),
        "{text}"
    );
    Ok(())
}

#[test]
fn history_change_kind_label_covers_both_kinds_of_stale_premise() {
    assert_eq!(
        change_kind_label(HistoryChangeKind::StalePremise),
        "stale_premise"
    );
}
