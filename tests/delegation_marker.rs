//! hivemind-zdsh.6: the delegation marker, end to end on the default (memory) graph backend.
//!
//! Alex's attribution ruling has three cases. Two were already distinguishable; the marker
//! makes the third distinguishable from the others:
//!
//! 1. an agent asks, a human chooses -> the human decided (`decided_by`)   [zdsh.3]
//! 2. a human delegated a scope, the agent decides within it -> agent decided, with the
//!    delegating human visible on the record (`delegated_by`)             [this bead]
//! 3. an agent decides alone, no delegation -> agent decided, no marker
//!
//! Every decision below is written through the real `Commands` write path, projected from the
//! ledger, and read back through the real query layer — no hand-built graph rows. The Postgres
//! parity of the same read path lives in `src/projector/postgres/tests.rs`.

use hivemind::commands::{Commands, DecisionProposalInput, Grounding};
use hivemind::ledger::InMemoryEventLedger;
use hivemind::projector::memory::MemoryGraph;
use hivemind::projector::project_from_ledger;
use hivemind::queries::{
    get_decision_brief, get_decision_context, get_decision_context_candidates,
    get_failure_attribution, search_decisions, AuthorshipShape, DecisionContextRequest,
    FailureAttributionRequest, ReviewShape, SearchDecisionRequest,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SCRIBE: &str = "agent:claude:scribe";
const BUILDER: &str = "agent:claude:builder";
const ALEX: &str = "human:alex";

struct ThreeCases {
    ledger: InMemoryEventLedger,
    human_decided: String,
    delegated: String,
    decided_alone: String,
}

/// Records one decision with a chosen option, exactly as `capture_decision` would.
fn capture(
    commands: &Commands<'_, InMemoryEventLedger>,
    actor_id: &str,
    title: &str,
    decided_by: Option<&str>,
    delegated_by: Option<&str>,
) -> TestResult<String> {
    let option_id = commands.record_option(actor_id, "Ship it", "The option that was chosen")?;
    let option_ids = [option_id];
    let labels = ["Ship it".to_owned()];
    let topics = ["governance".to_owned()];
    Ok(commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        project: None,
        actor_id,
        title,
        rationale: "A stated, self-contained reason that reads without the source conversation",
        topic_keys: &topics,
        option_ids: &option_ids,
        option_labels: &labels,
        chosen_option_id: option_ids.first().map(String::as_str),
        decided_by,
        delegated_by,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
    })?)
}

fn three_cases() -> TestResult<ThreeCases> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let human_decided = capture(
        &commands,
        SCRIBE,
        "Case 1: agent asked, the human chose",
        Some(ALEX),
        None,
    )?;
    let delegated = capture(
        &commands,
        BUILDER,
        "Case 2: agent decided within a delegated scope",
        None,
        Some(ALEX),
    )?;
    let decided_alone = capture(
        &commands,
        BUILDER,
        "Case 3: agent decided alone",
        None,
        None,
    )?;
    drop(commands);
    Ok(ThreeCases {
        ledger,
        human_decided,
        delegated,
        decided_alone,
    })
}

fn project(cases: &ThreeCases) -> TestResult<MemoryGraph> {
    let graph = MemoryGraph::default();
    project_from_ledger(&cases.ledger, &graph, 0)?;
    Ok(graph)
}

#[test]
fn the_three_cases_are_distinguishable_in_decision_context() -> TestResult {
    let cases = three_cases()?;
    let graph = project(&cases)?;

    let human = get_decision_context(&graph, &cases.human_decided)?
        .data
        .ok_or("case 1 context")?;
    assert_eq!(
        human.authorship,
        AuthorshipShape::AgentProposedHumanAccepted
    );
    assert_eq!(human.accepted_by, vec![ALEX.to_owned()]);
    assert_eq!(human.delegated_by, None);

    let delegated = get_decision_context(&graph, &cases.delegated)?
        .data
        .ok_or("case 2 context")?;
    let alone = get_decision_context(&graph, &cases.decided_alone)?
        .data
        .ok_or("case 3 context")?;

    // The enums are deliberately unchanged (Alex chose the Decision-node property over a new
    // AuthorshipShape variant): cases 2 and 3 still share `AgentOnly` + `SelfAccepted`...
    for context in [&delegated, &alone] {
        assert_eq!(context.authorship, AuthorshipShape::AgentOnly);
        assert_eq!(context.review, ReviewShape::SelfAccepted);
        assert_eq!(context.accepted_by, vec![BUILDER.to_owned()]);
    }
    // ...and the marker alone tells them apart.
    assert_eq!(delegated.delegated_by.as_deref(), Some(ALEX));
    assert_eq!(alone.delegated_by, None);

    // An agent deciding alone serializes with no `delegated_by` key at all: absence is the
    // signal, never a null someone could mistake for "delegation unknown".
    let delegated_json = serde_json::to_value(&delegated)?;
    let alone_json = serde_json::to_value(&alone)?;
    assert_eq!(delegated_json["delegated_by"], ALEX);
    assert!(alone_json.get("delegated_by").is_none());
    Ok(())
}

#[test]
fn bulk_context_candidates_carry_the_marker_too() -> TestResult {
    let cases = three_cases()?;
    let graph = project(&cases)?;

    let candidates = get_decision_context_candidates(
        &graph,
        &DecisionContextRequest {
            limit: 10,
            ..Default::default()
        },
    )?
    .data;

    let marker_of = |decision_id: &str| {
        candidates
            .iter()
            .find(|context| context.decision_id == decision_id)
            .map(|context| context.delegated_by.clone())
    };
    assert_eq!(marker_of(&cases.delegated), Some(Some(ALEX.to_owned())));
    assert_eq!(marker_of(&cases.decided_alone), Some(None));
    assert_eq!(marker_of(&cases.human_decided), Some(None));
    Ok(())
}

#[test]
fn the_brief_behind_verify_shows_the_delegation() -> TestResult {
    let cases = three_cases()?;
    let graph = project(&cases)?;

    let delegated = get_decision_brief(&graph, &cases.delegated)?
        .data
        .ok_or("case 2 brief")?;
    assert_eq!(delegated.decided_by.delegated_by.as_deref(), Some(ALEX));
    assert_eq!(delegated.decided_by.proposer_id.as_deref(), Some(BUILDER));
    assert_eq!(delegated.decided_by.decider_ids, vec![BUILDER.to_owned()]);

    let alone = get_decision_brief(&graph, &cases.decided_alone)?
        .data
        .ok_or("case 3 brief")?;
    assert_eq!(alone.decided_by.delegated_by, None);
    assert_eq!(alone.decided_by.decider_ids, vec![BUILDER.to_owned()]);
    assert!(serde_json::to_value(&alone)?["decided_by"]
        .get("delegated_by")
        .is_none());
    Ok(())
}

#[test]
fn search_results_carry_the_marker_for_the_digest() -> TestResult {
    let cases = three_cases()?;
    let graph = project(&cases)?;

    let results = search_decisions(&graph, &SearchDecisionRequest::default())?.data;
    let marker_of = |decision_id: &str| {
        results
            .items
            .iter()
            .find(|item| item.decision.id == decision_id)
            .map(|item| item.graph_context.delegated_by.clone())
    };
    assert_eq!(marker_of(&cases.delegated), Some(Some(ALEX.to_owned())));
    assert_eq!(marker_of(&cases.decided_alone), Some(None));
    assert_eq!(marker_of(&cases.human_decided), Some(None));
    Ok(())
}

#[test]
fn attribution_splits_delegated_from_agent_alone_and_leaves_human_decisions_out() -> TestResult {
    let cases = three_cases()?;
    let graph = project(&cases)?;

    let report = get_failure_attribution(&graph, &FailureAttributionRequest::default())?.data;
    let group = |label: &str| {
        report
            .by_delegation
            .iter()
            .find(|group| group.group_label == label)
    };

    let delegated = group("delegated").ok_or("delegated group present")?;
    let alone = group("agent_alone").ok_or("agent_alone group present")?;
    assert_eq!(delegated.dimension, "delegation");
    assert_eq!((delegated.total, alone.total), (1, 1));
    assert_eq!(
        report.by_delegation.len(),
        2,
        "the human-decided decision is neither delegated nor agent-alone"
    );
    // The existing dimensions still see all three decisions, unchanged.
    assert_eq!(report.corpus_stats.total_decisions, 3);
    Ok(())
}

#[test]
fn the_marker_survives_a_later_human_review() -> TestResult {
    // The delegation is a fact about how the agent decided; a human peer-reviewing the
    // decision afterwards changes its review shape, not that fact.
    let cases = three_cases()?;
    {
        let commands = Commands::new(&cases.ledger);
        commands.accept_decision(&cases.delegated, ALEX)?;
    }
    let graph = project(&cases)?;

    let context = get_decision_context(&graph, &cases.delegated)?
        .data
        .ok_or("case 2 context")?;
    assert_eq!(
        context.authorship,
        AuthorshipShape::AgentProposedHumanAccepted
    );
    assert_eq!(context.review, ReviewShape::PeerReviewed);
    assert_eq!(context.delegated_by.as_deref(), Some(ALEX));

    let report = get_failure_attribution(&graph, &FailureAttributionRequest::default())?.data;
    let delegated = report
        .by_delegation
        .iter()
        .find(|group| group.group_label == "delegated")
        .ok_or("delegated group present")?;
    assert_eq!(delegated.total, 1);
    Ok(())
}

#[test]
fn replaying_the_ledger_rebuilds_the_same_marker() -> TestResult {
    // The graph is a pure projection of the ledger: wipe-and-replay (a fresh graph here) must
    // land the marker on the same decision, never on a different one or on none.
    let cases = three_cases()?;
    let first = project(&cases)?;
    let second = project(&cases)?;

    for decision_id in [&cases.human_decided, &cases.delegated, &cases.decided_alone] {
        assert_eq!(
            get_decision_context(&first, decision_id)?.data,
            get_decision_context(&second, decision_id)?.data
        );
    }
    Ok(())
}
