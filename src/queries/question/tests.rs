// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::commands::{Commands, DecisionProposalInput, Grounding};
use crate::events::TenantId;
use crate::ledger::InMemoryEventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::{
    get_decision, get_decision_brief, get_decision_outcome, DecisionStatus, OutcomeReason,
};
use crate::Result;

use super::{
    answers_to, conflicting_answer_ids, newer_accepted_answers, other_answers, QuestionKey,
};

const QUESTION: &str = "Which storage engine should the prototype use?";
const ACTOR: &str = "actor:alice";

/// A captured decision that chose `chose` (accepted at capture) or, with `accept` false, is still
/// only a recommendation.
fn capture(
    commands: &Commands<'_, InMemoryEventLedger>,
    title: &str,
    chose: &str,
    question: Option<&str>,
    accept: bool,
) -> Result<String> {
    let option_id = commands.record_option(ACTOR, chose, chose)?;
    commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: ACTOR,
        title,
        rationale: "Rationale text long enough for the readable floor",
        topic_keys: &["storage".to_owned()],
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &[chose.to_owned()],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: None,
        still_proposed: !accept,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question,
        delegated_by: None,
        project: None,
    })
}

fn graph(ledger: &InMemoryEventLedger) -> Result<MemoryGraph> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, &TenantId::local(), &graph)?;
    Ok(graph)
}

fn ids(answers: &[super::QuestionAnswer]) -> Vec<&str> {
    answers
        .iter()
        .map(|answer| answer.decision_id.as_str())
        .collect()
}

#[test]
fn answers_to_lists_every_answer_in_event_order_with_status_and_choice() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let sqlite = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let postgres = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), false)?;
    capture(
        &commands,
        "Ship in October",
        "october",
        Some("When should it ship?"),
        true,
    )?;
    let graph = graph(&ledger)?;

    let by_text = answers_to(&graph, QuestionKey::Text(QUESTION))?
        .data
        .expect("the question exists");

    assert_eq!(by_text.text, QUESTION);
    assert_eq!(
        ids(&by_text.answers),
        vec![sqlite.as_str(), postgres.as_str()]
    );
    assert_eq!(by_text.answers[0].status, DecisionStatus::Accepted);
    assert_eq!(by_text.answers[0].chosen_option.as_deref(), Some("sqlite"));
    assert_eq!(by_text.answers[1].status, DecisionStatus::Proposed);
    assert_eq!(by_text.answers[1].title, "Use Postgres");

    let by_id = answers_to(&graph, QuestionKey::Id(&by_text.question_id))?
        .data
        .expect("the question exists");
    assert_eq!(by_id, by_text);
    Ok(())
}

#[test]
fn answers_to_matches_the_question_by_its_normalized_text() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let graph = graph(&ledger)?;

    let respelled = answers_to(
        &graph,
        QuestionKey::Text("  WHICH storage engine   should the prototype use "),
    )?;
    assert_eq!(respelled.result_count, 1);

    let other = answers_to(&graph, QuestionKey::Text("Which cache should it use?"))?;
    assert_eq!(
        other.data, None,
        "exact match only: a different question is a miss"
    );
    assert_eq!(answers_to(&graph, QuestionKey::Text("?"))?.data, None);
    assert_eq!(
        answers_to(&graph, QuestionKey::Id("question-nope"))?.data,
        None
    );
    Ok(())
}

#[test]
fn a_superseded_answer_stays_in_the_history_of_the_question() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let new = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    commands.supersede_decision(&old, &new, ACTOR)?;
    let graph = graph(&ledger)?;

    let answers = answers_to(&graph, QuestionKey::Text(QUESTION))?
        .data
        .expect("the question exists")
        .answers;

    assert_eq!(ids(&answers), vec![old.as_str(), new.as_str()]);
    assert_eq!(answers[0].status, DecisionStatus::Superseded);
    assert_eq!(answers[1].status, DecisionStatus::Accepted);
    // The replaced answer is not a conflict: it no longer stands.
    assert!(conflicting_answer_ids(&graph, &new)?.is_empty());
    assert!(conflicting_answer_ids(&graph, &old)?.is_empty());
    Ok(())
}

#[test]
fn other_answers_never_include_the_decision_itself() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let first = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let second = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    let alone = capture(&commands, "Ship it", "now", None, true)?;
    let graph = graph(&ledger)?;

    assert_eq!(ids(&other_answers(&graph, &first)?), vec![second.as_str()]);
    assert_eq!(ids(&other_answers(&graph, &second)?), vec![first.as_str()]);
    assert!(other_answers(&graph, &alone)?.is_empty());
    Ok(())
}

#[test]
fn accepted_answers_that_choose_differently_conflict_on_both_sides() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let sqlite = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let postgres = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    let graph = graph(&ledger)?;

    assert_eq!(
        conflicting_answer_ids(&graph, &sqlite)?,
        vec![postgres.clone()]
    );
    assert_eq!(
        conflicting_answer_ids(&graph, &postgres)?,
        vec![sqlite.clone()]
    );

    for (decision_id, other_id) in [(&sqlite, &postgres), (&postgres, &sqlite)] {
        let outcome = get_decision_outcome(&graph, decision_id)?
            .data
            .expect("the decision exists");
        assert!(
            outcome.reasons.contains(&OutcomeReason::ConflictingAnswer {
                other_id: other_id.clone(),
            }),
            "{:?}",
            outcome.reasons
        );
        assert!(
            outcome.held_up,
            "a conflict is attention: neither answer has been shown wrong"
        );
    }
    Ok(())
}

#[test]
fn answers_that_choose_the_same_option_do_not_conflict() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let first = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let second = capture(&commands, "Keep SQLite", "SQLite", Some(QUESTION), true)?;
    let graph = graph(&ledger)?;

    assert!(conflicting_answer_ids(&graph, &first)?.is_empty());
    assert!(conflicting_answer_ids(&graph, &second)?.is_empty());
    Ok(())
}

#[test]
fn a_recommendation_nobody_accepted_is_not_a_conflict() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let accepted = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let proposed = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), false)?;
    let graph = graph(&ledger)?;

    assert!(conflicting_answer_ids(&graph, &accepted)?.is_empty());
    assert!(conflicting_answer_ids(&graph, &proposed)?.is_empty());
    Ok(())
}

#[test]
fn a_contested_answer_is_not_an_accepted_one() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let sqlite = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let postgres = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    commands.reject_decision(&postgres, "actor:bob")?;
    let graph = graph(&ledger)?;

    // Accepted by alice, rejected by bob: contested, a status of its own, never `accepted`.
    assert!(conflicting_answer_ids(&graph, &sqlite)?.is_empty());
    assert!(conflicting_answer_ids(&graph, &postgres)?.is_empty());
    Ok(())
}

#[test]
fn only_a_later_accepted_answer_is_newer() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let first = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let second = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    let third = capture(&commands, "Use DuckDB", "duckdb", Some(QUESTION), false)?;
    let graph = graph(&ledger)?;

    assert_eq!(
        ids(&newer_accepted_answers(&graph, &first)?),
        vec![second.as_str()],
        "the proposed third answer is not an answer anyone accepted"
    );
    assert!(newer_accepted_answers(&graph, &second)?.is_empty());
    assert!(newer_accepted_answers(&graph, &third)?.is_empty());
    Ok(())
}

#[test]
fn the_brief_and_the_view_carry_the_question_node_and_the_other_answers() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let first = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let second = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    let graph = graph(&ledger)?;

    let view = get_decision(&graph, &first)?.data.expect("decision");
    let brief = get_decision_brief(&graph, &first)?.data.expect("brief");

    let question_id = view.question_id.clone().expect("the view names the node");
    assert_eq!(view.question.as_deref(), Some(QUESTION));
    assert_eq!(brief.question_id.as_deref(), Some(question_id.as_str()));
    assert_eq!(brief.question.as_deref(), Some(QUESTION));
    assert_eq!(ids(&brief.other_answers), vec![second.as_str()]);
    assert_eq!(
        brief.other_answers[0].chosen_option.as_deref(),
        Some("postgres")
    );
    assert!(brief
        .still_holds
        .reasons
        .contains(&OutcomeReason::ConflictingAnswer { other_id: second }));
    Ok(())
}

#[test]
fn a_decision_linked_after_the_fact_reads_the_question_from_its_node() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision = capture(&commands, "Use SQLite", "sqlite", None, true)?;
    commands.answer_question("actor:bob", &decision, QUESTION)?;
    let graph = graph(&ledger)?;

    let view = get_decision(&graph, &decision)?.data.expect("decision");

    assert_eq!(
        view.question.as_deref(),
        Some(QUESTION),
        "no text of its own: the words come from the question node"
    );
    assert!(view.question_id.is_some());
    Ok(())
}

#[test]
fn a_decision_with_no_question_has_no_question_fields() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let decision = capture(&commands, "Use SQLite", "sqlite", None, true)?;
    let graph = graph(&ledger)?;

    let brief = get_decision_brief(&graph, &decision)?.data.expect("brief");

    assert_eq!(brief.question, None);
    assert_eq!(brief.question_id, None);
    assert!(brief.other_answers.is_empty());
    let json = serde_json::to_value(&brief).expect("brief serializes");
    assert!(json.get("question_id").is_none() && json.get("other_answers").is_none());
    Ok(())
}
