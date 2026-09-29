// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::commands::{Commands, DecisionProposalInput, Grounding};
use crate::events::TenantId;
use crate::ledger::InMemoryEventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::DecisionStatus;
use crate::Result;

use super::{get_contested_decisions, Contest, ContestedDecisionsRequest};

const QUESTION: &str = "Which storage engine should the prototype use?";
const ACTOR: &str = "actor:alice";
const OTHER: &str = "actor:bob";

/// A captured decision that chose `chose`, accepted by `ACTOR` at capture, or with `accept`
/// false still only a recommendation.
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

fn contested(
    ledger: &InMemoryEventLedger,
    limit: usize,
    cursor: Option<&str>,
) -> Result<super::ContestedDecisionsResults> {
    let graph = graph(ledger)?;
    Ok(get_contested_decisions(
        &graph,
        &ContestedDecisionsRequest {
            limit,
            cursor: cursor.map(str::to_owned),
        },
    )?
    .data)
}

#[test]
fn an_empty_ledger_lists_nothing() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let results = contested(&ledger, 0, None)?;
    assert!(results.items.is_empty());
    assert_eq!(results.total_matches, 0);
    assert_eq!(results.next_cursor, None);
    Ok(())
}

#[test]
fn a_disagreement_names_who_accepted_and_who_rejected() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let sqlite = capture(&commands, "Use SQLite", "sqlite", None, true)?;
    commands.reject_decision(&sqlite, OTHER)?;

    let results = contested(&ledger, 0, None)?;

    assert_eq!(results.items.len(), 1);
    let item = &results.items[0];
    assert_eq!(item.decision_id, sqlite);
    assert_eq!(item.title, "Use SQLite");
    assert_eq!(item.status, DecisionStatus::Contested);
    assert_eq!(item.asked_at, None, "nobody asked");
    assert!(item.decided_at.is_some());
    assert_eq!(
        item.contest,
        Contest::Disagreement {
            accepted_by: vec![ACTOR.to_owned()],
            rejected_by: vec![OTHER.to_owned()],
        }
    );
    Ok(())
}

#[test]
fn an_accepted_or_merely_proposed_decision_is_not_contested() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    capture(&commands, "Use SQLite", "sqlite", None, true)?;
    capture(&commands, "Use Postgres", "postgres", None, false)?;

    let results = contested(&ledger, 0, None)?;

    assert!(results.items.is_empty());
    Ok(())
}

#[test]
fn a_superseded_decision_leaves_the_list() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let old = capture(&commands, "Use SQLite", "sqlite", None, true)?;
    let new = capture(&commands, "Use Postgres", "postgres", None, true)?;
    commands.reject_decision(&old, OTHER)?;
    assert_eq!(contested(&ledger, 0, None)?.items.len(), 1);

    commands.supersede_decision(&old, &new, ACTOR)?;

    // Somebody replaced it: the disagreement was dealt with, so it no longer needs attention.
    assert!(contested(&ledger, 0, None)?.items.is_empty());
    Ok(())
}

#[test]
fn conflicting_answers_list_both_decisions_each_naming_the_other() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let plan = commands.plan_ask(QUESTION)?;
    commands.record_ask("actor:carol", &plan)?;
    let sqlite = capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    let postgres = capture(&commands, "Use Postgres", "postgres", Some(QUESTION), true)?;
    capture(
        &commands,
        "Ship in October",
        "october",
        Some("When should it ship?"),
        true,
    )?;

    let results = contested(&ledger, 0, None)?;

    assert_eq!(results.total_matches, 2);
    let ids: Vec<&str> = results
        .items
        .iter()
        .map(|item| item.decision_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![sqlite.as_str(), postgres.as_str()],
        "oldest first"
    );
    for (item, other_id) in results.items.iter().zip([&postgres, &sqlite]) {
        assert_eq!(item.status, DecisionStatus::Accepted);
        let (asked_at, decided_at) = (item.asked_at.unwrap(), item.decided_at.unwrap());
        assert!(asked_at <= decided_at, "the ask came first");
        let Contest::ConflictingAnswers {
            question,
            conflicts_with,
            ..
        } = &item.contest
        else {
            panic!("a conflicting answer, got {:?}", item.contest);
        };
        assert_eq!(question, QUESTION);
        assert_eq!(conflicts_with.len(), 1);
        assert_eq!(&conflicts_with[0].decision_id, other_id);
    }
    Ok(())
}

#[test]
fn answers_that_choose_alike_or_were_never_accepted_do_not_conflict() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    capture(&commands, "Use SQLite", "sqlite", Some(QUESTION), true)?;
    capture(&commands, "Also SQLite", "SQLite", Some(QUESTION), true)?;
    capture(
        &commands,
        "Maybe Postgres",
        "postgres",
        Some(QUESTION),
        false,
    )?;

    let results = contested(&ledger, 0, None)?;

    assert!(results.items.is_empty());
    Ok(())
}

#[test]
fn the_list_pages_oldest_first_and_says_when_it_is_truncated() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let mut created = Vec::new();
    for (title, chose) in [("One", "a"), ("Two", "b"), ("Three", "c")] {
        let id = capture(&commands, title, chose, None, true)?;
        commands.reject_decision(&id, OTHER)?;
        created.push(id);
    }

    let graph = graph(&ledger)?;
    let first = get_contested_decisions(
        &graph,
        &ContestedDecisionsRequest {
            limit: 2,
            cursor: None,
        },
    )?;
    assert!(first.truncated);
    assert_eq!(first.result_count, 2);
    assert_eq!(first.data.total_matches, 3);
    assert_eq!(first.data.next_cursor.as_deref(), Some("2"));
    assert_eq!(first.data.items[0].decision_id, created[0]);
    assert_eq!(first.data.items[1].decision_id, created[1]);

    let second = get_contested_decisions(
        &graph,
        &ContestedDecisionsRequest {
            limit: 2,
            cursor: first.data.next_cursor.clone(),
        },
    )?;
    assert!(!second.truncated);
    assert_eq!(second.data.items.len(), 1);
    assert_eq!(second.data.items[0].decision_id, created[2]);
    assert_eq!(second.data.next_cursor, None);
    Ok(())
}

#[test]
fn a_cursor_that_is_not_an_offset_is_refused() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let graph = graph(&ledger)?;
    let refused = get_contested_decisions(
        &graph,
        &ContestedDecisionsRequest {
            limit: 5,
            cursor: Some("not-a-number".to_owned()),
        },
    );
    assert!(refused.is_err());
    Ok(())
}
