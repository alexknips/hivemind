// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::commands::Commands;
use crate::events::CaptureItem;
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::queries::{resolve_decision_for_reading, ResolveOutcome};

use super::*;

const ALEX: &str = "human:alex";

fn decision(title: &str, rationale: &str) -> CaptureItem {
    CaptureItem {
        kind: "decision".to_owned(),
        title: title.to_owned(),
        rationale: rationale.to_owned(),
        topic_keys: Vec::new(),
        evidence_ids: Vec::new(),
        options: Some(vec!["Fable only".to_owned(), "Fable and Opus".to_owned()]),
        chosen_option: Some("Fable only".to_owned()),
        extraction_confidence: 0.9,
        expressed_confidence: None,
        supersedes_id: None,
        restates_id: None,
        source_turn_id: None,
        source_ts: None,
        question: None,
        premised_on_ids: Vec::new(),
        supports_ids: Vec::new(),
        refutes_ids: Vec::new(),
        actor_id: Some(ALEX.to_owned()),
        accepted_by: Vec::new(),
        rejected_by: Vec::new(),
        blocked_actor_id: None,
        decision_id: None,
        participants: Vec::new(),
        session_initiator: None,
    }
}

/// Alex's Fable ruling of 2026-09-27 as three sessions recorded it, then a decision, its
/// replacement (alike in words, a different decision), and an unrelated one.
struct Fixture {
    ledger: InMemoryEventLedger,
    /// The three records of the ruling, earliest first.
    fable: [String; 3],
    postgres: String,
    postgres_18: String,
}

fn fixture() -> Fixture {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    // An earlier event, so no capture is event 1 (`capture:0:*` is not a recorded id).
    commands
        .record_evidence(ALEX, "the town seats ran on two models last week")
        .expect("evidence");

    let mut replacement = decision(
        "Adopt Postgres 18 for the shared ledger",
        "The newer release fixes the planner",
    );
    replacement.supersedes_id = Some("Adopt Postgres for the shared ledger".to_owned());
    let recorded = commands
        .record_ingest_batch_classified(
            "agent:claude:classifier",
            &["batch-1".to_owned()],
            "claude-haiku-4-5-20251001",
            "2",
            vec![
                decision(
                    "Run every town seat on Fable only until Oct 1 with no added usage this week",
                    "Every town seat runs on the Fable model this week, to hold the spend",
                ),
                decision(
                    "Run every town seat on Fable only with no added usage this week",
                    "The model for every town seat is Fable this week, to hold the spend",
                ),
                decision(
                    "Run every seat on Fable only until the Oct 1 reset",
                    "Fable is the model until the October reset",
                ),
                decision(
                    "Adopt Postgres for the shared ledger",
                    "One server for every tenant",
                ),
                replacement,
                decision("Review the cost on Friday", "To check the spend"),
            ],
            None,
        )
        .expect("classified");
    let id = |index: usize| format!("capture:{}:{index}", recorded.event_id);
    Fixture {
        ledger,
        fable: [id(0), id(1), id(2)],
        postgres: id(3),
        postgres_18: id(4),
    }
}

fn graph_of(ledger: &InMemoryEventLedger) -> MemoryGraph {
    let graph = MemoryGraph::default();
    rebuild_graph(ledger, &graph).expect("graph rebuilds");
    graph
}

#[test]
fn an_answer_names_only_a_decision_it_was_shown() {
    let nearby = vec![
        NearbyDecision {
            decision_id: "capture:7:0".to_owned(),
            title: "Run every town seat on Fable only".to_owned(),
        },
        NearbyDecision {
            decision_id: "decision:abc".to_owned(),
            title: "Adopt Postgres".to_owned(),
        },
    ];
    let answer = |restates_id: Option<&str>| RestatementAnswer {
        restates_id: restates_id.map(str::to_owned),
    };

    assert_eq!(
        answer(Some("decision:abc")).named_among(&nearby).as_deref(),
        Some("decision:abc")
    );
    assert_eq!(
        answer(Some(" capture:7:0 "))
            .named_among(&nearby)
            .as_deref(),
        Some("capture:7:0"),
        "stray whitespace is not an invention"
    );
    assert_eq!(answer(None).named_among(&nearby), None);
    assert_eq!(
        answer(Some("capture:99:0")).named_among(&nearby),
        None,
        "an id the judge was not shown never reaches the ledger"
    );
}

#[test]
fn the_prompt_shows_the_capture_and_every_nearby_decision_and_allows_none() {
    let capture = decision("Fable only for every town seat", "To hold the spend");
    let nearby = vec![NearbyDecision {
        decision_id: "capture:7:0".to_owned(),
        title: "Run every town seat on Fable only".to_owned(),
    }];
    let prompt = restatement_prompt(&capture, &nearby);

    assert!(prompt.contains("title: Fable only for every town seat"));
    assert!(prompt.contains("chosen: Fable only"));
    assert!(prompt.contains("id: capture:7:0"));
    assert!(prompt.contains("Run every town seat on Fable only"));
    assert!(prompt.contains("null"), "none must be an allowed answer");
    assert!(restatement_schema()["required"]
        .as_array()
        .expect("required list")
        .contains(&serde_json::json!("restates_id")));
}

#[test]
fn nearby_decisions_are_what_recall_would_answer_and_a_linked_decision_shows_once() {
    let f = fixture();
    let capture = decision("Fable only for every town seat", "Relayed once more");

    let before = nearby_decisions(&graph_of(&f.ledger), &capture).expect("nearby");
    // The two records that hold every word of the title; the third lacks "town", so it is only
    // a close match and is not shown while full matches exist.
    assert_eq!(
        before.len(),
        2,
        "two unlinked records of the ruling are two candidates: {before:?}"
    );
    assert!(before
        .iter()
        .all(|nearby| f.fable.contains(&nearby.decision_id)));

    let commands = Commands::new(&f.ledger);
    for later in &f.fable[1..] {
        commands
            .link_same_as(ALEX, later, &f.fable[0])
            .expect("links");
    }
    let after = nearby_decisions(&graph_of(&f.ledger), &capture).expect("nearby");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0].decision_id, f.fable[0], "the earliest record");
}

#[test]
fn a_capture_with_no_title_has_no_neighbours() {
    let f = fixture();
    let capture = decision("   ", "x");
    assert!(nearby_decisions(&graph_of(&f.ledger), &capture)
        .expect("nearby")
        .is_empty());
}

#[test]
fn proposals_link_each_later_record_to_the_earlier_one_it_overlaps_and_show_the_basis() {
    let f = fixture();
    let proposals = propose_same_as_links(&graph_of(&f.ledger), None).expect("proposals");

    let pairs: Vec<(&str, &str)> = proposals
        .proposed
        .iter()
        .map(|link| (link.decision_id.as_str(), link.restates_id.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            (f.fable[1].as_str(), f.fable[0].as_str()),
            (f.fable[2].as_str(), f.fable[0].as_str()),
        ],
        "the replacement of a decision and the unrelated one are not proposed"
    );
    assert_eq!(proposals.decisions_scanned, 6);

    let second = &proposals.proposed[0];
    assert!(second.overlap > 0.8, "{}", second.overlap);
    assert!(second.shared_terms.contains(&"fable".to_owned()));
    assert!(second.shared_terms.contains(&"usage".to_owned()));
    // Exactly at the bar: seven shared words of fourteen.
    assert!((proposals.proposed[1].overlap - 0.5).abs() < f64::EPSILON);
    assert!(f.postgres != f.postgres_18);
}

#[test]
fn a_pair_already_linked_is_not_proposed_again_even_through_a_third_decision() {
    let f = fixture();
    let commands = Commands::new(&f.ledger);
    commands
        .link_same_as(ALEX, &f.fable[1], &f.fable[0])
        .expect("links");
    commands
        .link_same_as(ALEX, &f.fable[2], &f.fable[1])
        .expect("links");

    let proposals = propose_same_as_links(&graph_of(&f.ledger), None).expect("proposals");
    assert!(proposals.proposed.is_empty(), "{:?}", proposals.proposed);
}

#[test]
fn proposals_can_be_limited_to_one_project() {
    let f = fixture();
    let graph = graph_of(&f.ledger);
    assert!(propose_same_as_links(&graph, Some("personal:human:nobody"))
        .expect("proposals")
        .proposed
        .is_empty());
    let proposals =
        propose_same_as_links(&graph, Some("personal:agent:claude")).expect("proposals");
    assert_eq!(proposals.proposed.len(), 2);
}

#[test]
fn apply_links_writes_each_link_once_and_nothing_when_any_is_refused() {
    let f = fixture();
    let commands = Commands::new(&f.ledger);
    let links = vec![
        (f.fable[1].clone(), f.fable[0].clone()),
        (f.fable[2].clone(), f.fable[0].clone()),
    ];

    let before = f.ledger.latest_offset().expect("offset");
    let mut refused = links.clone();
    refused.push((
        "decision:nobody-recorded-this".to_owned(),
        f.fable[0].clone(),
    ));
    assert!(apply_links(&commands, ALEX, &refused).is_err());
    assert_eq!(f.ledger.latest_offset().expect("offset"), before);

    let applied = apply_links(&commands, ALEX, &links).expect("applies");
    assert!(applied.iter().all(|link| link.event_id.is_some()));
    assert_eq!(f.ledger.latest_offset().expect("offset"), before + 2);

    let again = apply_links(&commands, ALEX, &links).expect("applies again");
    assert!(again.iter().all(|link| link.event_id.is_none()));
    assert_eq!(f.ledger.latest_offset().expect("offset"), before + 2);
}

/// The bead's own check: the question that was ambiguous among three records of one ruling
/// resolves to one decision once the links are applied, and not before.
#[test]
fn why_resolves_the_ruling_without_a_pick_once_the_proposed_links_are_applied() {
    let f = fixture();
    let question = "which model do the town seats run on this week?";

    let before = resolve_decision_for_reading(&graph_of(&f.ledger), question, None)
        .expect("resolves")
        .data;
    assert!(
        matches!(before, ResolveOutcome::Ambiguous { .. }),
        "three records of one ruling are a tie: {before:?}"
    );

    let commands = Commands::new(&f.ledger);
    let proposals = propose_same_as_links(&graph_of(&f.ledger), None).expect("proposals");
    let links: Vec<(String, String)> = proposals
        .proposed
        .into_iter()
        .map(|link| (link.decision_id, link.restates_id))
        .collect();
    apply_links(&commands, ALEX, &links).expect("applies");

    match resolve_decision_for_reading(&graph_of(&f.ledger), question, None)
        .expect("resolves")
        .data
    {
        ResolveOutcome::Resolved { candidate } => {
            assert_eq!(candidate.decision_id, f.fable[0]);
            assert!(candidate.missing_terms.is_empty(), "{candidate:?}");
            let copies: Vec<&str> = candidate
                .also_recorded_as
                .iter()
                .map(|copy| copy.decision_id.as_str())
                .collect();
            assert_eq!(copies, vec![f.fable[1].as_str(), f.fable[2].as_str()]);
        }
        other => panic!("expected one decision, got {other:?}"),
    }
}
