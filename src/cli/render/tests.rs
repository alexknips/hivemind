// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::projector::memory::MemoryGraph;
use crate::queries::test_fixtures::{project_first_fixture, ts, Scenario};
use crate::queries::{
    get_compact_view, get_decision_brief_at, get_decisions_changed_since, get_recent_activity,
    get_situational_decisions, search_decisions_with_ledger, ChangedSinceRequest, QueryContext,
    RecentActivityRequest, SearchDecisionRequest, SituationalRequest,
};
use crate::summarize::{DecisionSummary, RecallRanked, RecallResponse, SummarizeMode};
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
fn situational_summary_of_a_project_scoped_answer_says_where_each_decision_came_from() -> Result<()>
{
    let fixture = project_first_fixture()?;
    let graph = MemoryGraph::default();
    crate::projector::rebuild_graph(&fixture.ledger, &graph)?;
    let ask = |project: Option<&str>, paths: &str| {
        get_situational_decisions(
            &QueryContext::local(),
            &graph,
            &fixture.ledger,
            &SituationalRequest {
                paths: vec![paths.to_owned()],
                limit: 50,
                project: project.map(str::to_owned),
                ..SituationalRequest::default()
            },
        )
    };

    let text = render_situational_summary(&ask(Some("billing"), "pricing")?.data);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("terms\tpricing"));
    assert_eq!(
        lines.next(),
        Some("scope\tLooked in Billing (own project), Platform (Billing is part of it), Auth (Billing depends on it); not followed: 1 more level up the part_of chain, 2 more linked projects.")
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 4, "{text}");
    assert!(
        rows[0].contains("\tproject=Billing\tscope=own project\t"),
        "{}",
        rows[0]
    );
    assert!(
        rows[1].contains("\tproject=Platform\tscope=from Platform; Billing is part of it\t"),
        "{}",
        rows[1]
    );
    for row in &rows[2..] {
        assert!(
            row.contains("\tproject=Auth\tscope=from Auth; Billing depends on it\t"),
            "{row}"
        );
    }
    // The superseded Auth decision keeps its staleness label across the hop.
    assert!(
        rows.iter().any(|row| row.contains("STALE(superseded)")),
        "{text}"
    );

    // An empty scoped answer still says which projects it looked in.
    let empty = render_situational_summary(&ask(Some("billing"), "nothingmatches")?.data);
    assert!(
        empty.starts_with(
            "No decisions bear on this situation (terms: nothingmatches)\nscope\tLooked in Billing"
        ),
        "{empty}"
    );

    // Without a project the rows and header are exactly what they were.
    let unscoped = render_situational_summary(&ask(None, "pricing")?.data);
    assert!(!unscoped.contains("scope"), "{unscoped}");
    Ok(())
}

#[test]
fn recall_summary_of_a_project_scoped_answer_says_where_each_decision_came_from() -> Result<()> {
    let fixture = project_first_fixture()?;
    let graph = MemoryGraph::default();
    crate::projector::rebuild_graph(&fixture.ledger, &graph)?;
    // The recall response around the search it is built on; the digest is not what is tested.
    let ask = |project: Option<&str>, question: &str| -> Result<RecallResponse> {
        let search = search_decisions_with_ledger(
            &QueryContext::local(),
            &fixture.ledger,
            &graph,
            &SearchDecisionRequest {
                query: Some(question.to_owned()),
                limit: 10,
                project: project.map(str::to_owned),
                ..SearchDecisionRequest::default()
            },
        )?
        .data;
        Ok(RecallResponse {
            query: Some(question.to_owned()),
            ignored_words: Vec::new(),
            ranked: RecallRanked {
                total_matches: search.total_matches,
                truncated: false,
                items: search.items,
            },
            digest: DecisionSummary {
                summary: "digest".to_owned(),
                cited_decision_ids: Vec::new(),
                unit: SummarizeMode::Cluster,
            },
            scope: search.scope,
        })
    };

    let text = render_recall_summary(&ask(Some("billing"), "pricing")?);
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("scope\tLooked in Billing (own project), Platform (Billing is part of it), Auth (Billing depends on it); not followed: 1 more level up the part_of chain, 2 more linked projects.")
    );
    assert_eq!(lines.next(), Some("digest\tdigest"));
    assert_eq!(lines.next(), Some("cited\t"));
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 4, "{text}");
    assert!(
        rows[0].ends_with("\tproject=Billing\tscope=own project"),
        "{}",
        rows[0]
    );
    assert!(
        rows[1].ends_with("\tproject=Platform\tscope=from Platform; Billing is part of it"),
        "{}",
        rows[1]
    );
    for row in &rows[2..] {
        assert!(
            row.ends_with("\tproject=Auth\tscope=from Auth; Billing depends on it"),
            "{row}"
        );
    }

    // An empty scoped answer still says which projects it looked in.
    let empty = render_recall_summary(&ask(Some("billing"), "nothingmatches")?);
    assert!(
        empty.starts_with("No decisions found matching the query.\nscope\tLooked in Billing"),
        "{empty}"
    );

    // Without a project the rows and header are exactly what they were.
    let unscoped = render_recall_summary(&ask(None, "pricing")?);
    assert!(!unscoped.contains("scope"), "{unscoped}");
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

#[test]
fn history_change_kind_label_names_a_project_move() {
    assert_eq!(
        change_kind_label(HistoryChangeKind::ProjectMoved),
        "project_moved"
    );
}

#[test]
fn history_lines_say_where_a_moved_decision_went_and_when_and_leave_other_lines_alone() -> Result<()>
{
    // Mayor's ruling on hivemind-s15q.10: the history entry for a move carries from, to, actor
    // and time; the finished "moved from A to B by X on <date>" sentence ships with the verb.
    let scenario = Scenario::new();
    scenario.decision(
        "d:pricing",
        "Per-seat pricing",
        "human:alice",
        "2026-01-01T00:00:00Z",
    )?;
    scenario.moved(
        "d:pricing",
        "billing",
        "pricing",
        "human:alex",
        "2026-01-02T03:04:05Z",
    )?;

    let activity = get_recent_activity(scenario.ledger(), &RecentActivityRequest::default())?.data;
    let activity_text = render_recent_activity_summary(&activity);
    let mut activity_lines = activity_text.lines();
    let moved_line = activity_lines.next().expect("the move is the newest row");
    assert!(
        moved_line.contains("\tproject_moved\tdecision.moved\tactor=human:alex\t"),
        "{moved_line}"
    );
    assert!(
        moved_line.ends_with("\tmoved=billing->pricing\tat=2026-01-02T03:04:05+00:00"),
        "{moved_line}"
    );
    let proposal_line = activity_lines.next().expect("the proposal row");
    assert!(
        !proposal_line.contains("moved="),
        "only a move line carries the move: {proposal_line}"
    );

    let changed = get_decisions_changed_since(
        scenario.ledger(),
        &ChangedSinceRequest {
            since_offset: Some(0),
            limit: 10,
            ..ChangedSinceRequest::default()
        },
    )?
    .data;
    let changed_text = render_changed_since_summary(&changed);
    assert!(
        changed_text
            .lines()
            .any(|line| line.contains("\tproject_moved\t")
                && line.ends_with("\tmoved=billing->pricing\tat=2026-01-02T03:04:05+00:00")),
        "{changed_text}"
    );
    Ok(())
}
