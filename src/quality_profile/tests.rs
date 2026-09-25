// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::path::Path;

use uuid::Uuid;

use crate::commands::{Commands, DecisionProposalInput, Grounding};
use crate::events::HypothesisKind;
use crate::ledger::InMemoryEventLedger;
use crate::mcp::args::default_option_description;
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::queries::test_fixtures::{floor_scenario, ts, FLOOR_SCENARIO_DECISIONS};
use crate::queries::{
    EvidenceFact, GroundingAdded, HypothesisFact, OptionFact, PremiseFact, RecordFacts,
    RefutationFact, SupersessionFact,
};
use crate::Result;

use super::*;

// ── helpers ───────────────────────────────────────────────────────────────────

/// A record that states nothing beyond a rationale. The decision was recorded at ledger offset 10.
fn facts() -> RecordFacts {
    RecordFacts {
        decision_id: "d:1".to_owned(),
        event_origin: Some(10),
        occurred_at: None,
        question: None,
        rationale: Some("Because the numbers said so".to_owned()),
        expressed_confidence: None,
        chosen_option_id: None,
        options: Vec::new(),
        evidence: Vec::new(),
        premises: Vec::new(),
        hypotheses: Vec::new(),
    }
}

fn option(id: &str, label: &str, description: Option<&str>) -> OptionFact {
    OptionFact {
        id: id.to_owned(),
        label: Some(label.to_owned()),
        description: description.map(str::to_owned),
    }
}

fn evidence(
    id: &str,
    event_origin: Option<i64>,
    source: Option<&str>,
    added: GroundingAdded,
) -> EvidenceFact {
    EvidenceFact {
        id: id.to_owned(),
        event_origin,
        source: source.map(str::to_owned),
        added,
    }
}

fn premise(id: &str, event_origin: Option<i64>, added: GroundingAdded) -> PremiseFact {
    PremiseFact {
        id: id.to_owned(),
        event_origin,
        occurred_at: None,
        added,
        superseded: None,
    }
}

fn hypothesis(
    id: &str,
    kind: HypothesisKind,
    event_origin: Option<i64>,
    added: GroundingAdded,
) -> HypothesisFact {
    HypothesisFact {
        id: id.to_owned(),
        kind,
        event_origin,
        added,
        refuted_by: Vec::new(),
    }
}

fn refutation(evidence_id: &str, evidence_origin: Option<i64>) -> RefutationFact {
    RefutationFact {
        evidence_id: evidence_id.to_owned(),
        evidence_origin,
    }
}

fn attributed_later() -> GroundingAdded {
    GroundingAdded::Later {
        actor_id: Some("human:alex".to_owned()),
        ts: None,
    }
}

fn assessed(assessment: &Assessment) -> (Level, Vec<ReasonKind>, &[String]) {
    match assessment {
        Assessment::Assessed {
            level,
            reasons,
            node_ids,
        } => (
            *level,
            reasons.iter().map(|reason| reason.kind).collect(),
            node_ids.as_slice(),
        ),
        Assessment::NotAssessed { why } => panic!("expected an assessed dimension, not: {why}"),
    }
}

fn reason_ids(assessment: &Assessment, kind: ReasonKind) -> Vec<String> {
    match assessment {
        Assessment::Assessed { reasons, .. } => reasons
            .iter()
            .find(|reason| reason.kind == kind)
            .map(|reason| reason.node_ids.clone())
            .unwrap_or_default(),
        Assessment::NotAssessed { why } => panic!("expected an assessed dimension, not: {why}"),
    }
}

fn reason_text(assessment: &Assessment, kind: ReasonKind) -> String {
    match assessment {
        Assessment::Assessed { reasons, .. } => reasons
            .iter()
            .find(|reason| reason.kind == kind)
            .map(|reason| reason.text.clone())
            .unwrap_or_default(),
        Assessment::NotAssessed { why } => panic!("expected an assessed dimension, not: {why}"),
    }
}

fn kinds_of(assessment: &Assessment) -> Vec<ReasonKind> {
    assessed(assessment).1
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

// ── Framing ───────────────────────────────────────────────────────────────────

#[test]
fn framing_is_partial_when_the_question_is_recorded_and_never_more() {
    let mut record = facts();
    record.question = Some("Which store should hold the ledger?".to_owned());

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.framing);
    assert_eq!(level, Level::Partial);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::QuestionRecorded,
            ReasonKind::NotJudgedWithoutModel
        ]
    );
    assert_eq!(node_ids, ["d:1"]);
}

#[test]
fn framing_is_none_without_a_recorded_question() {
    for question in [None, Some("   ".to_owned())] {
        let mut record = facts();
        record.question = question;

        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.framing);
        assert_eq!(level, Level::None);
        assert_eq!(kinds, vec![ReasonKind::NoQuestionRecorded]);
        assert!(node_ids.is_empty());
    }
}

// ── Alternatives ──────────────────────────────────────────────────────────────

#[test]
fn alternatives_are_none_when_nothing_was_set_against_the_option_taken() {
    // No option at all; only the chosen option; a single option with none chosen.
    let mut no_options = facts();
    no_options.options.clear();
    let mut only_the_chosen = facts();
    only_the_chosen.options = vec![option("o:a", "Postgres", Some("One server"))];
    only_the_chosen.chosen_option_id = Some("o:a".to_owned());
    let mut one_unchosen = facts();
    one_unchosen.options = vec![option("o:a", "Postgres", Some("One server"))];

    for record in [no_options, only_the_chosen, one_unchosen] {
        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.alternatives);
        assert_eq!(level, Level::None);
        assert_eq!(kinds, vec![ReasonKind::NoAlternativeRecorded]);
        assert!(node_ids.is_empty());
    }
}

#[test]
fn alternatives_are_solid_when_every_alternative_has_a_description_of_its_own() {
    let mut record = facts();
    record.options = vec![
        option("o:pg", "Postgres", Some("One server for every tenant")),
        option("o:sqlite", "SQLite", Some("A file per tenant; no joins")),
        option("o:kuzu", "Kuzu", Some("Embedded graph; slow build")),
    ];
    record.chosen_option_id = Some("o:pg".to_owned());

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.alternatives);
    assert_eq!(level, Level::Solid);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::AlternativesRecorded,
            ReasonKind::AlternativesDescribed
        ]
    );
    // The chosen option is not an alternative to itself.
    assert_eq!(node_ids, ["o:kuzu", "o:sqlite"]);
}

#[test]
fn alternatives_are_partial_when_an_alternative_has_no_description_of_its_own() {
    let mut record = facts();
    record.options = vec![
        option("o:pg", "Postgres", Some("One server for every tenant")),
        option("o:sqlite", "SQLite", Some("A file per tenant; no joins")),
        option("o:kuzu", "Kuzu", None),
        option("o:blank", "Blank", Some("  ")),
    ];
    record.chosen_option_id = Some("o:pg".to_owned());

    let profile = profile_from_record(&record);

    let (level, kinds, _) = assessed(&profile.alternatives);
    assert_eq!(level, Level::Partial);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::AlternativesRecorded,
            ReasonKind::AlternativesUndescribed
        ]
    );
    // Only the alternatives that lack a description are named as lacking one.
    assert_eq!(
        reason_ids(&profile.alternatives, ReasonKind::AlternativesUndescribed),
        ids(&["o:blank", "o:kuzu"])
    );
    assert_eq!(
        reason_ids(&profile.alternatives, ReasonKind::AlternativesRecorded),
        ids(&["o:blank", "o:kuzu", "o:sqlite"])
    );
}

#[test]
fn with_no_option_chosen_every_option_is_an_alternative() {
    let mut described = facts();
    described.options = vec![
        option("o:a", "Shard by tenant", Some("One shard per tenant")),
        option("o:b", "Shard by hash", Some("Hash the decision id")),
    ];
    let mut half_described = facts();
    half_described.options = vec![
        option("o:a", "Shard by tenant", Some("One shard per tenant")),
        option("o:b", "Shard by hash", None),
    ];

    assert_eq!(
        assessed(&profile_from_record(&described).alternatives).0,
        Level::Solid
    );
    assert_eq!(
        assessed(&profile_from_record(&half_described).alternatives).0,
        Level::Partial
    );
}

#[test]
fn text_a_capture_surface_generates_does_not_count_as_a_description() {
    // Each of these is what a surface writes when the author gave the option no description of
    // their own (`GENERATED_QUOTED_PREFIXES` and friends), plus a description that only repeats
    // the label.
    let generated = [
        "Option generated from MCP value 'Direct'",
        "Option generated from CLI value 'Direct'",
        "Option generated from supersede value 'Direct'",
        "Option 'Direct'",
        "Slack option 'Direct' captured from slack://C123/p456",
        "Option imported from document block block-7",
        "Direct",
        "  direct  ",
    ];
    for description in generated {
        let mut record = facts();
        record.options = vec![
            option(
                "o:a",
                "Gateway",
                Some("Route every call through one gateway"),
            ),
            option("o:b", "Direct", Some(description)),
        ];
        record.chosen_option_id = Some("o:a".to_owned());

        let profile = profile_from_record(&record);

        let (level, _, _) = assessed(&profile.alternatives);
        assert_eq!(level, Level::Partial, "{description:?} must not count");
        assert_eq!(
            reason_ids(&profile.alternatives, ReasonKind::AlternativesUndescribed),
            ids(&["o:b"])
        );
    }
}

#[test]
fn the_mcp_placeholder_the_write_layer_fills_in_does_not_count() {
    // The recogniser and the MCP surface must agree on the text; this fails if either moves.
    let mut record = facts();
    record.options = vec![
        option(
            "o:a",
            "Gateway",
            Some("Route every call through one gateway"),
        ),
        option("o:b", "Direct", Some(&default_option_description("Direct"))),
    ];
    record.chosen_option_id = Some("o:a".to_owned());

    let profile = profile_from_record(&record);

    assert_eq!(assessed(&profile.alternatives).0, Level::Partial);
}

#[test]
fn a_description_that_only_starts_like_a_placeholder_is_the_authors_own() {
    let descriptions = [
        "Option generated from MCP value 'Direct' -- rejected: extra hop for no gain",
        "Slack option 'Direct' was floated in the thread",
        "Direct calls, rejected because they bypass the audit trail",
    ];
    for description in descriptions {
        let mut record = facts();
        record.options = vec![
            option(
                "o:a",
                "Gateway",
                Some("Route every call through one gateway"),
            ),
            option("o:b", "Direct", Some(description)),
        ];
        record.chosen_option_id = Some("o:a".to_owned());

        let profile = profile_from_record(&record);

        assert_eq!(
            assessed(&profile.alternatives).0,
            Level::Solid,
            "{description:?} is the author's own text"
        );
    }
}

// ── Information ───────────────────────────────────────────────────────────────

#[test]
fn information_is_none_when_nothing_is_rested_on() {
    let profile = profile_from_record(&facts());

    let (level, kinds, node_ids) = assessed(&profile.information);
    assert_eq!(level, Level::None);
    assert_eq!(kinds, vec![ReasonKind::NothingRestedOn]);
    assert!(node_ids.is_empty());
}

#[test]
fn information_is_solid_when_counted_evidence_says_where_it_was_observed() {
    let mut record = facts();
    record.evidence = vec![
        evidence("e:b", Some(4), None, GroundingAdded::AtCapture),
        evidence(
            "e:a",
            Some(3),
            Some("mayor audit 2026-09-22"),
            GroundingAdded::AtCapture,
        ),
    ];

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.information);
    assert_eq!(level, Level::Solid);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::EvidenceCounted,
            ReasonKind::EvidenceSourceStated
        ]
    );
    assert_eq!(node_ids, ["e:a", "e:b"]);
    assert_eq!(
        reason_ids(&profile.information, ReasonKind::EvidenceSourceStated),
        ids(&["e:a"])
    );
}

#[test]
fn information_is_partial_when_counted_evidence_names_no_source() {
    let mut record = facts();
    record.evidence = vec![
        evidence("e:a", Some(3), None, GroundingAdded::AtCapture),
        evidence("e:b", Some(4), Some("  "), GroundingAdded::AtCapture),
    ];

    let profile = profile_from_record(&record);

    let (level, kinds, _) = assessed(&profile.information);
    assert_eq!(level, Level::Partial);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::EvidenceCounted,
            ReasonKind::EvidenceSourceMissing
        ]
    );
}

#[test]
fn evidence_recorded_after_the_decision_is_shown_as_later_and_never_counted() {
    // Linked afterwards, recorded after the decision (offset 10): later.
    let mut linked_after = facts();
    linked_after.evidence = vec![evidence(
        "e:after",
        Some(11),
        Some("bench run"),
        attributed_later(),
    )];
    // Linked afterwards and its own offset is unknown: nothing shows it pre-dates the decision.
    let mut unknown_origin = facts();
    unknown_origin.evidence = vec![evidence(
        "e:after",
        None,
        Some("bench run"),
        attributed_later(),
    )];
    // The decision's own offset is unknown: only what was attached at capture can count.
    let mut unknown_decision = facts();
    unknown_decision.event_origin = None;
    unknown_decision.evidence = vec![evidence("e:after", Some(3), None, attributed_later())];

    for record in [linked_after, unknown_origin, unknown_decision] {
        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.information);
        assert_eq!(level, Level::None);
        assert_eq!(
            kinds,
            vec![ReasonKind::NothingRestedOn, ReasonKind::EvidenceLater]
        );
        assert_eq!(node_ids, ["e:after"]);
    }
}

#[test]
fn evidence_recorded_before_the_decision_counts_even_when_it_was_linked_later() {
    // The ex-ante boundary at decision offset 10: 9 counts, 10 and 11 do not.
    for (origin, counts) in [(9, true), (10, false), (11, false)] {
        let mut record = facts();
        record.evidence = vec![evidence("e:1", Some(origin), None, attributed_later())];

        let profile = profile_from_record(&record);

        let expected = if counts { Level::Partial } else { Level::None };
        assert_eq!(
            assessed(&profile.information).0,
            expected,
            "evidence recorded at offset {origin}"
        );
    }
}

#[test]
fn evidence_attached_at_capture_counts_without_an_offset() {
    let mut record = facts();
    record.evidence = vec![evidence("e:1", None, None, GroundingAdded::AtCapture)];

    let profile = profile_from_record(&record);

    assert_eq!(assessed(&profile.information).0, Level::Partial);
}

#[test]
fn later_evidence_never_raises_a_level() {
    let mut record = facts();
    record.evidence = vec![
        evidence("e:capture", Some(3), None, GroundingAdded::AtCapture),
        evidence(
            "e:after",
            Some(11),
            Some("bench run 2026-02-01"),
            attributed_later(),
        ),
    ];

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.information);
    assert_eq!(
        level,
        Level::Partial,
        "a sourced later item must not lift it"
    );
    assert_eq!(
        kinds,
        vec![
            ReasonKind::EvidenceCounted,
            ReasonKind::EvidenceSourceMissing,
            ReasonKind::EvidenceLater
        ]
    );
    assert_eq!(node_ids, ["e:after", "e:capture"]);
    assert_eq!(
        reason_ids(&profile.information, ReasonKind::EvidenceLater),
        ids(&["e:after"])
    );
}

// ── Reasoning ─────────────────────────────────────────────────────────────────

#[test]
fn reasoning_is_partial_when_a_rationale_is_recorded_and_never_more() {
    let profile = profile_from_record(&facts());

    let (level, kinds, node_ids) = assessed(&profile.reasoning);
    assert_eq!(level, Level::Partial);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::RationaleStated,
            ReasonKind::NotJudgedWithoutModel
        ]
    );
    assert_eq!(node_ids, ["d:1"]);
}

#[test]
fn reasoning_is_none_without_a_rationale() {
    for rationale in [None, Some(" \n ".to_owned())] {
        let mut record = facts();
        record.rationale = rationale;

        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.reasoning);
        assert_eq!(level, Level::None);
        assert_eq!(kinds, vec![ReasonKind::NoRationale]);
        assert!(node_ids.is_empty());
    }
}

// ── Information: what the decision rests on ───────────────────────────────────

/// One counted item of each kind of grounding: `(name, record, the kind that counts it)`.
fn counted_grounding_of_each_kind() -> Vec<(&'static str, RecordFacts, ReasonKind)> {
    let mut prior = facts();
    prior.premises = vec![premise("d:prior", Some(3), GroundingAdded::AtCapture)];
    let mut assumed = facts();
    assumed.hypotheses = vec![hypothesis(
        "h:assumed",
        HypothesisKind::Assumption,
        Some(3),
        GroundingAdded::AtCapture,
    )];
    let mut bet = facts();
    bet.hypotheses = vec![hypothesis(
        "h:bet",
        HypothesisKind::Bet,
        Some(3),
        GroundingAdded::AtCapture,
    )];
    vec![
        ("a prior decision", prior, ReasonKind::PremiseCounted),
        ("an assumption", assumed, ReasonKind::AssumptionCounted),
        ("a declared bet", bet, ReasonKind::BetCounted),
    ]
}

#[test]
fn information_counts_a_prior_decision_an_assumption_or_a_bet_but_only_to_partial() {
    for (what, record, counted) in counted_grounding_of_each_kind() {
        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.information);
        assert_eq!(level, Level::Partial, "{what}");
        assert_eq!(kinds, vec![counted], "{what}");
        assert_eq!(node_ids.len(), 1, "{what}");
        // None of them is an observation, so none of them can say where it was observed.
        assert_ne!(level, Level::Solid, "{what}");
    }
}

/// One kind of grounding: its name, a record carrying one item of it (recorded at the given
/// offset, attached the given way), and the reason that shows it as later.
type Case = (
    &'static str,
    fn(Option<i64>, GroundingAdded) -> RecordFacts,
    ReasonKind,
);

#[test]
fn grounding_of_every_kind_follows_the_ex_ante_boundary() {
    // The ex-ante boundary at decision offset 10, for a prior decision, an assumption and a bet
    // (`recorded before` counts even when linked afterwards; `at capture` counts with no offset;
    // `later` never does).
    let cases: [Case; 3] = [
        (
            "prior decision",
            |origin, added| {
                let mut record = facts();
                record.premises = vec![premise("d:prior", origin, added)];
                record
            },
            ReasonKind::PremiseLater,
        ),
        (
            "assumption",
            |origin, added| {
                let mut record = facts();
                record.hypotheses =
                    vec![hypothesis("h:1", HypothesisKind::Assumption, origin, added)];
                record
            },
            ReasonKind::AssumptionLater,
        ),
        (
            "bet",
            |origin, added| {
                let mut record = facts();
                record.hypotheses = vec![hypothesis("h:1", HypothesisKind::Bet, origin, added)];
                record
            },
            ReasonKind::BetLater,
        ),
    ];
    for (what, build, later_kind) in cases {
        for (origin, added, counts, when) in [
            (
                Some(9),
                attributed_later(),
                true,
                "recorded before, linked later",
            ),
            (
                Some(10),
                attributed_later(),
                false,
                "recorded at the decision's offset",
            ),
            (
                Some(11),
                attributed_later(),
                false,
                "recorded after, linked later",
            ),
            (None, attributed_later(), false, "no offset, linked later"),
            (
                Some(11),
                GroundingAdded::AtCapture,
                true,
                "named at capture",
            ),
            (
                None,
                GroundingAdded::AtCapture,
                true,
                "named at capture, no offset",
            ),
        ] {
            let profile = profile_from_record(&build(origin, added));

            let (level, kinds, _) = assessed(&profile.information);
            let expected = if counts { Level::Partial } else { Level::None };
            assert_eq!(level, expected, "{what}: {when}");
            assert_eq!(kinds.contains(&later_kind), !counts, "{what}: {when}");
            if !counts {
                assert_eq!(kinds[0], ReasonKind::NothingRestedOn, "{what}: {when}");
            }
        }
    }
}

#[test]
fn later_grounding_never_lifts_information_beside_counted_grounding() {
    let mut record = facts();
    record.hypotheses = vec![hypothesis(
        "h:bet",
        HypothesisKind::Bet,
        Some(3),
        GroundingAdded::AtCapture,
    )];
    record.evidence = vec![evidence(
        "e:after",
        Some(11),
        Some("bench run 2026-02-01"),
        attributed_later(),
    )];
    record.premises = vec![premise("d:after", Some(12), attributed_later())];

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.information);
    assert_eq!(
        level,
        Level::Partial,
        "a sourced later item must not lift it"
    );
    assert_eq!(
        kinds,
        vec![
            ReasonKind::BetCounted,
            ReasonKind::EvidenceLater,
            ReasonKind::PremiseLater
        ]
    );
    assert_eq!(node_ids, ["d:after", "e:after", "h:bet"]);
}

#[test]
fn sourced_evidence_lifts_information_to_solid_beside_other_grounding() {
    let mut record = facts();
    record.evidence = vec![evidence(
        "e:1",
        Some(3),
        Some("load test 2026-09-20"),
        GroundingAdded::AtCapture,
    )];
    record.premises = vec![premise("d:prior", Some(2), GroundingAdded::AtCapture)];
    record.hypotheses = vec![hypothesis(
        "h:bet",
        HypothesisKind::Bet,
        Some(4),
        GroundingAdded::AtCapture,
    )];

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.information);
    assert_eq!(level, Level::Solid);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::EvidenceCounted,
            ReasonKind::EvidenceSourceStated,
            ReasonKind::PremiseCounted,
            ReasonKind::BetCounted
        ]
    );
    assert_eq!(node_ids, ["d:prior", "e:1", "h:bet"]);
}

#[test]
fn a_superseded_prior_decision_is_shown_as_a_fact_and_changes_no_level() {
    let plain = {
        let mut record = facts();
        record.premises = vec![premise("d:prior", Some(3), GroundingAdded::AtCapture)];
        record
    };
    let baseline = profile_from_record(&plain);

    // The decision is at offset 10. Superseded at 20 (later), at 5 (already), or at an unknown
    // offset.
    for (superseded_at, wording) in [
        (Some(20), "since superseded (later)"),
        (
            Some(5),
            "already superseded when this decision was recorded",
        ),
        (None, "not recorded"),
    ] {
        let mut record = plain.clone();
        record.premises[0].superseded = Some(SupersessionFact {
            by_id: "d:newer".to_owned(),
            event_origin: superseded_at,
        });

        let profile = profile_from_record(&record);

        assert_eq!(
            assessed(&profile.information).0,
            assessed(&baseline.information).0,
            "{wording}: the level does not move"
        );
        assert_eq!(
            assessed(&profile.reasoning).0,
            assessed(&baseline.reasoning).0
        );
        assert_eq!(
            profile.bias_exposure.level(),
            baseline.bias_exposure.level()
        );
        assert_eq!(
            kinds_of(&profile.information),
            vec![ReasonKind::PremiseCounted, ReasonKind::PremiseSuperseded],
            "{wording}"
        );
        assert_eq!(
            reason_ids(&profile.information, ReasonKind::PremiseSuperseded),
            ids(&["d:newer", "d:prior"]),
            "{wording}"
        );
        assert!(
            reason_text(&profile.information, ReasonKind::PremiseSuperseded).contains(wording),
            "{wording}"
        );
        assert!(profile.attention.is_empty());
    }
}

#[test]
fn a_superseded_prior_decision_that_is_still_later_is_not_described() {
    // Only what counts is described: a prior decision attached too late is shown as later.
    let mut record = facts();
    record.premises = vec![premise("d:prior", Some(30), attributed_later())];
    record.premises[0].superseded = Some(SupersessionFact {
        by_id: "d:newer".to_owned(),
        event_origin: Some(40),
    });

    let profile = profile_from_record(&record);

    assert_eq!(
        kinds_of(&profile.information),
        vec![ReasonKind::NothingRestedOn, ReasonKind::PremiseLater]
    );
}

// ── Reasoning ─────────────────────────────────────────────────────────────────

#[test]
fn a_prior_decision_it_follows_from_counts_as_stated_reasoning_and_still_stops_at_partial() {
    for rationale in [None, Some("Because the numbers said so".to_owned())] {
        let mut record = facts();
        record.rationale = rationale.clone();
        record.premises = vec![premise("d:prior", Some(3), GroundingAdded::AtCapture)];

        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.reasoning);
        assert_eq!(level, Level::Partial, "{rationale:?}");
        let first = if rationale.is_some() {
            ReasonKind::RationaleStated
        } else {
            ReasonKind::NoRationale
        };
        assert_eq!(
            kinds,
            vec![
                first,
                ReasonKind::PremiseLinked,
                ReasonKind::NotJudgedWithoutModel
            ],
            "{rationale:?}"
        );
        assert!(node_ids.contains(&"d:prior".to_owned()), "{rationale:?}");
    }
}

#[test]
fn a_prior_decision_attached_too_late_is_not_reasoning() {
    let mut record = facts();
    record.rationale = None;
    record.premises = vec![premise("d:prior", Some(11), attributed_later())];

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.reasoning);
    assert_eq!(level, Level::None);
    assert_eq!(
        kinds,
        vec![ReasonKind::NoRationale, ReasonKind::PremiseLater]
    );
    assert_eq!(node_ids, ["d:prior"]);
}

// ── Calibration ───────────────────────────────────────────────────────────────

fn with_confidence(confidence: &str) -> RecordFacts {
    let mut record = facts();
    record.expressed_confidence = Some(confidence.to_owned());
    record
}

fn bet(id: &str) -> HypothesisFact {
    hypothesis(id, HypothesisKind::Bet, Some(3), GroundingAdded::AtCapture)
}

#[test]
fn calibration_is_not_assessed_without_a_declared_confidence_and_none_is_inferred() {
    // Whatever the decision rests on, and however confident its rationale sounds.
    let mut grounded = facts();
    grounded.rationale = Some("We are certain this is right".to_owned());
    grounded.evidence = vec![evidence(
        "e:1",
        Some(3),
        Some("audit"),
        GroundingAdded::AtCapture,
    )];
    let mut blank = facts();
    blank.expressed_confidence = Some("  ".to_owned());
    for record in [facts(), grounded, blank] {
        let profile = profile_from_record(&record);

        let Assessment::NotAssessed { why } = &profile.calibration else {
            panic!(
                "calibration must be not assessed, got {:?}",
                profile.calibration
            );
        };
        assert!(why.contains("No confidence was declared"), "{why}");
        assert_eq!(profile.calibration.level(), None);
        assert!(profile.attention.is_empty());
    }
}

#[test]
fn calibration_does_not_read_a_confidence_outside_the_vocabulary() {
    let profile = profile_from_record(&with_confidence("pretty sure"));

    let Assessment::NotAssessed { why } = &profile.calibration else {
        panic!(
            "calibration must be not assessed, got {:?}",
            profile.calibration
        );
    };
    assert!(why.contains("'pretty sure'"), "{why}");
    assert!(profile.attention.is_empty());
}

#[test]
fn calibration_is_none_when_nothing_counted_is_on_record_to_compare_the_confidence_with() {
    for confidence in ["low", "medium", "high", " HIGH "] {
        let profile = profile_from_record(&with_confidence(confidence));

        let (level, kinds, node_ids) = assessed(&profile.calibration);
        assert_eq!(level, Level::None, "{confidence:?}");
        assert_eq!(
            kinds,
            vec![ReasonKind::ConfidenceDeclared, ReasonKind::RestsOnNothing],
            "{confidence:?}"
        );
        assert_eq!(node_ids, ["d:1"]);
        // Only "high" over nothing is a line of attention, and it is not a deduction.
        let attention: Vec<AttentionKind> =
            profile.attention.iter().map(|line| line.kind).collect();
        if confidence.trim().eq_ignore_ascii_case("high") {
            assert_eq!(attention, vec![AttentionKind::HighConfidenceOverNothing]);
            assert_eq!(profile.attention[0].dimension, Dimension::Calibration);
            assert_eq!(profile.attention[0].node_ids, ["d:1"]);
        } else {
            assert!(attention.is_empty(), "{confidence:?}");
        }
    }
}

#[test]
fn high_confidence_over_a_bet_alone_is_attention_not_a_lower_level() {
    let mut levels = Vec::new();
    for confidence in ["low", "medium", "high"] {
        let mut record = with_confidence(confidence);
        record.hypotheses = vec![bet("h:bet")];

        let profile = profile_from_record(&record);

        let (level, kinds, node_ids) = assessed(&profile.calibration);
        levels.push(level);
        assert_eq!(
            kinds,
            vec![
                ReasonKind::ConfidenceDeclared,
                ReasonKind::RestsOnBetOnly,
                ReasonKind::NotJudgedWithoutModel
            ],
            "{confidence}"
        );
        assert_eq!(node_ids, ["d:1", "h:bet"]);
        let attention: Vec<AttentionKind> =
            profile.attention.iter().map(|line| line.kind).collect();
        if confidence == "high" {
            assert_eq!(attention, vec![AttentionKind::HighConfidenceOverBet]);
            assert_eq!(profile.attention[0].node_ids, ["h:bet"]);
        } else {
            assert!(attention.is_empty(), "{confidence}");
        }
    }
    // The level is the same whatever the confidence: the mismatch is not a deduction.
    assert_eq!(levels, vec![Level::Partial; 3]);
}

#[test]
fn confidence_over_grounding_is_compared_and_never_raises_attention() {
    let mut on_evidence = with_confidence("high");
    on_evidence.evidence = vec![evidence("e:1", Some(3), None, GroundingAdded::AtCapture)];
    let mut on_a_prior_decision = with_confidence("high");
    on_a_prior_decision.premises = vec![premise("d:prior", Some(3), GroundingAdded::AtCapture)];
    let mut on_an_assumption = with_confidence("high");
    on_an_assumption.hypotheses = vec![hypothesis(
        "h:assumed",
        HypothesisKind::Assumption,
        Some(3),
        GroundingAdded::AtCapture,
    )];
    // Grounded wins over a bet, as everywhere else.
    let mut grounded_and_a_bet = with_confidence("high");
    grounded_and_a_bet.evidence = vec![evidence("e:1", Some(3), None, GroundingAdded::AtCapture)];
    grounded_and_a_bet.hypotheses = vec![bet("h:bet")];

    for record in [
        on_evidence,
        on_a_prior_decision,
        on_an_assumption,
        grounded_and_a_bet,
    ] {
        let profile = profile_from_record(&record);

        let (level, kinds, _) = assessed(&profile.calibration);
        assert_eq!(level, Level::Partial);
        assert_eq!(
            kinds,
            vec![
                ReasonKind::ConfidenceDeclared,
                ReasonKind::ConfidenceComparedWithGrounding,
                ReasonKind::NotJudgedWithoutModel
            ]
        );
        assert!(profile.attention.is_empty());
    }
}

#[test]
fn calibration_names_what_the_confidence_was_compared_with() {
    let mut record = with_confidence("medium");
    record.evidence = vec![
        evidence("e:b", Some(3), None, GroundingAdded::AtCapture),
        evidence("e:a", Some(3), None, GroundingAdded::AtCapture),
    ];
    record.premises = vec![premise("d:prior", Some(3), GroundingAdded::AtCapture)];
    record.hypotheses = vec![bet("h:bet")];

    let profile = profile_from_record(&record);

    assert_eq!(
        reason_text(
            &profile.calibration,
            ReasonKind::ConfidenceComparedWithGrounding
        ),
        "compared with what it rests on: 2 evidence items, 1 prior decision, 1 declared bet"
    );
    assert_eq!(
        reason_ids(
            &profile.calibration,
            ReasonKind::ConfidenceComparedWithGrounding
        ),
        ids(&["d:prior", "e:a", "e:b", "h:bet"])
    );
}

#[test]
fn grounding_attached_after_capture_does_not_calibrate_a_confidence_declared_at_capture() {
    // The ex-ante boundary at decision offset 10: 9 counts, 10 and 11 do not.
    for (origin, counts) in [(9, true), (10, false), (11, false)] {
        let mut record = with_confidence("high");
        record.evidence = vec![evidence("e:1", Some(origin), None, attributed_later())];

        let profile = profile_from_record(&record);

        let expected = if counts { Level::Partial } else { Level::None };
        assert_eq!(
            assessed(&profile.calibration).0,
            expected,
            "evidence recorded at offset {origin}"
        );
        assert_eq!(
            profile.attention.len(),
            usize::from(!counts),
            "evidence recorded at offset {origin}"
        );
    }
}

#[test]
fn a_bet_attached_after_capture_leaves_high_confidence_over_nothing() {
    let mut record = with_confidence("high");
    record.hypotheses = vec![hypothesis(
        "h:bet",
        HypothesisKind::Bet,
        Some(11),
        attributed_later(),
    )];

    let profile = profile_from_record(&record);

    assert_eq!(
        kinds_of(&profile.calibration),
        vec![ReasonKind::ConfidenceDeclared, ReasonKind::RestsOnNothing]
    );
    assert!(
        reason_text(&profile.calibration, ReasonKind::RestsOnNothing)
            .contains("pre-dates the decision")
    );
    assert_eq!(
        profile.attention[0].kind,
        AttentionKind::HighConfidenceOverNothing
    );
}

// ── Bias exposure ─────────────────────────────────────────────────────────────

#[test]
fn bias_exposure_is_none_when_neither_a_counter_option_nor_counter_evidence_is_recorded() {
    let profile = profile_from_record(&facts());

    let (level, kinds, node_ids) = assessed(&profile.bias_exposure);
    assert_eq!(level, Level::None);
    assert_eq!(
        kinds,
        vec![ReasonKind::NoCounterOption, ReasonKind::NoCounterEvidence]
    );
    assert!(node_ids.is_empty());
}

#[test]
fn a_counter_option_makes_bias_exposure_partial_and_never_more() {
    let mut record = facts();
    record.options = vec![
        option(
            "o:a",
            "Gateway",
            Some("Route every call through one gateway"),
        ),
        option("o:b", "Direct", None),
    ];
    record.chosen_option_id = Some("o:a".to_owned());

    let profile = profile_from_record(&record);

    let (level, kinds, node_ids) = assessed(&profile.bias_exposure);
    assert_eq!(level, Level::Partial);
    assert_eq!(
        kinds,
        vec![
            ReasonKind::CounterOptionRecorded,
            ReasonKind::NoCounterEvidence,
            ReasonKind::NotJudgedWithoutModel
        ]
    );
    // A counter-option is one set against the option taken, described or not.
    assert_eq!(node_ids, ["o:b"]);
}

#[test]
fn a_single_option_is_no_counter_option() {
    let mut record = facts();
    record.options = vec![option("o:a", "Gateway", Some("One gateway"))];
    record.chosen_option_id = Some("o:a".to_owned());

    let profile = profile_from_record(&record);

    assert_eq!(assessed(&profile.bias_exposure).0, Level::None);
}

/// A record at offset 10 resting on a counted assumption that `refutation` refutes.
fn refuted_before(refutation: RefutationFact) -> RecordFacts {
    let mut record = facts();
    let mut assumed = hypothesis(
        "h:assumed",
        HypothesisKind::Assumption,
        Some(3),
        GroundingAdded::AtCapture,
    );
    assumed.refuted_by = vec![refutation];
    record.hypotheses = vec![assumed];
    record
}

#[test]
fn counter_evidence_counts_only_when_the_evidence_pre_dates_the_decision() {
    // Decision offset 10: the evidence must have been recorded before it. The link that says it
    // refutes something may have been added afterwards, like any link to evidence.
    for (evidence_origin, counts) in [
        (Some(5), true),
        (Some(9), true),
        (Some(10), false),
        (Some(11), false),
        (None, false),
    ] {
        let profile =
            profile_from_record(&refuted_before(refutation("e:counter", evidence_origin)));

        let (level, kinds, node_ids) = assessed(&profile.bias_exposure);
        let context = format!("evidence recorded at {evidence_origin:?}");
        if counts {
            assert_eq!(level, Level::Partial, "{context}");
            assert_eq!(
                kinds,
                vec![
                    ReasonKind::NoCounterOption,
                    ReasonKind::CounterEvidenceRecorded,
                    ReasonKind::NotJudgedWithoutModel
                ],
                "{context}"
            );
            assert_eq!(node_ids, ["e:counter", "h:assumed"], "{context}");
        } else {
            assert_eq!(level, Level::None, "{context}");
            assert_eq!(
                kinds,
                vec![ReasonKind::NoCounterOption, ReasonKind::NoCounterEvidence],
                "{context}"
            );
        }
    }
}

#[test]
fn evidence_refuting_something_attached_after_capture_is_not_counter_evidence() {
    let mut record = facts();
    let mut assumed = hypothesis(
        "h:assumed",
        HypothesisKind::Assumption,
        Some(11),
        attributed_later(),
    );
    assumed.refuted_by = vec![refutation("e:counter", Some(5))];
    record.hypotheses = vec![assumed];

    let profile = profile_from_record(&record);

    assert_eq!(assessed(&profile.bias_exposure).0, Level::None);
}

#[test]
fn premise_age_is_reported_as_a_fact_and_never_moves_the_level() {
    let decided = ts("2026-03-01T00:00:00Z");
    let mut fresh = facts();
    fresh.occurred_at = Some(decided);
    fresh.premises = vec![premise("d:fresh", Some(3), GroundingAdded::AtCapture)];
    fresh.premises[0].occurred_at = Some(ts("2026-02-28T00:00:00Z"));
    let mut old = facts();
    old.occurred_at = Some(decided);
    old.premises = vec![premise("d:old", Some(3), GroundingAdded::AtCapture)];
    old.premises[0].occurred_at = Some(ts("2024-01-02T00:00:00Z"));

    let fresh_profile = profile_from_record(&fresh);
    let old_profile = profile_from_record(&old);

    assert_eq!(
        reason_text(&fresh_profile.bias_exposure, ReasonKind::PremiseAge),
        "age of the prior decisions it rests on when this was recorded: d:fresh 1 day"
    );
    assert_eq!(
        reason_text(&old_profile.bias_exposure, ReasonKind::PremiseAge),
        "age of the prior decisions it rests on when this was recorded: d:old 789 days"
    );
    assert_eq!(
        reason_ids(&old_profile.bias_exposure, ReasonKind::PremiseAge),
        ids(&["d:old"])
    );
    assert_eq!(
        fresh_profile.bias_exposure.level(),
        old_profile.bias_exposure.level(),
        "an old premise is not a lower level"
    );
}

#[test]
fn premise_age_says_when_it_cannot_be_derived() {
    let decided = ts("2026-03-01T00:00:00Z");
    // No timestamp on the decision, none on the premise, or the premise is stamped later.
    let mut no_decision_time = facts();
    no_decision_time.premises = vec![premise("d:a", Some(3), GroundingAdded::AtCapture)];
    no_decision_time.premises[0].occurred_at = Some(ts("2026-01-01T00:00:00Z"));
    let mut no_premise_time = facts();
    no_premise_time.occurred_at = Some(decided);
    no_premise_time.premises = vec![premise("d:a", Some(3), GroundingAdded::AtCapture)];
    let mut stamped_later = facts();
    stamped_later.occurred_at = Some(decided);
    stamped_later.premises = vec![premise("d:a", Some(3), GroundingAdded::AtCapture)];
    stamped_later.premises[0].occurred_at = Some(ts("2026-04-01T00:00:00Z"));

    for record in [no_decision_time, no_premise_time, stamped_later] {
        let profile = profile_from_record(&record);

        assert!(reason_text(&profile.bias_exposure, ReasonKind::PremiseAge)
            .contains("cannot be derived"));
    }

    // One premise with an age and one without: the age is given and the gap is counted.
    let mut mixed = facts();
    mixed.occurred_at = Some(decided);
    mixed.premises = vec![
        premise("d:a", Some(3), GroundingAdded::AtCapture),
        premise("d:b", Some(4), GroundingAdded::AtCapture),
    ];
    mixed.premises[0].occurred_at = Some(ts("2026-02-01T00:00:00Z"));
    assert_eq!(
        reason_text(
            &profile_from_record(&mixed).bias_exposure,
            ReasonKind::PremiseAge
        ),
        "age of the prior decisions it rests on when this was recorded: d:a 28 days; not derivable for 1"
    );
}

#[test]
fn a_prior_decision_attached_too_late_has_no_age() {
    let mut record = facts();
    record.occurred_at = Some(ts("2026-03-01T00:00:00Z"));
    record.premises = vec![premise("d:late", Some(11), attributed_later())];
    record.premises[0].occurred_at = Some(ts("2026-01-01T00:00:00Z"));

    let profile = profile_from_record(&record);

    assert_eq!(
        kinds_of(&profile.bias_exposure),
        vec![ReasonKind::NoCounterOption, ReasonKind::NoCounterEvidence]
    );
}

// ── Not assessed ──────────────────────────────────────────────────────────────

#[test]
fn values_and_tradeoffs_is_always_not_assessed_and_says_why() {
    let profile = profile_from_record(&facts());

    let assessment = profile.assessment(Dimension::ValuesTradeoffs);
    let Assessment::NotAssessed { why } = assessment else {
        panic!("values_tradeoffs must be not assessed, got {assessment:?}");
    };
    assert!(!why.trim().is_empty());
    assert_eq!(assessment.level(), None);
}

#[test]
fn a_not_assessed_entry_never_carries_a_level_on_the_wire() {
    // No declared confidence: two of the seven have no basis.
    let profile = profile_from_record(&facts());

    let wire = serde_json::to_value(&profile).expect("profile serializes");

    for dimension in ["values_tradeoffs", "calibration"] {
        let entry = &wire[dimension];
        assert_eq!(entry["status"], "not_assessed", "{dimension}");
        assert!(entry["why"].is_string(), "{dimension}");
        assert!(entry.get("level").is_none(), "{dimension} carries a level");
        assert!(entry.get("reasons").is_none(), "{dimension}");
    }
    for dimension in [
        "framing",
        "alternatives",
        "information",
        "reasoning",
        "bias_exposure",
    ] {
        let entry = &wire[dimension];
        assert_eq!(entry["status"], "assessed", "{dimension}");
        assert!(entry["level"].is_string(), "{dimension}");
        assert!(entry["reasons"].is_array(), "{dimension}");
        assert!(entry["node_ids"].is_array(), "{dimension}");
    }
    assert_eq!(wire["attention"], serde_json::json!([]));
}

#[test]
fn a_declared_confidence_makes_calibration_assessed_and_attention_names_its_dimension() {
    let mut record = with_confidence("high");
    record.hypotheses = vec![bet("h:bet")];

    let wire = serde_json::to_value(profile_from_record(&record)).expect("profile serializes");

    assert_eq!(wire["calibration"]["status"], "assessed");
    assert_eq!(wire["calibration"]["level"], "partial");
    assert_eq!(wire["attention"][0]["kind"], "high_confidence_over_bet");
    assert_eq!(wire["attention"][0]["dimension"], "calibration");
    assert_eq!(
        wire["attention"][0]["node_ids"],
        serde_json::json!(["h:bet"])
    );
}

// ── Shape and determinism ─────────────────────────────────────────────────────

#[test]
fn every_profile_lists_the_seven_dimensions_in_order_with_the_floor_version() {
    let profile = profile_from_record(&facts());

    let dimensions: Vec<Dimension> = profile.iter().map(|(dimension, _)| dimension).collect();

    assert_eq!(dimensions, Dimension::ALL);
    assert_eq!(profile.floor_version, FLOOR_VERSION);
    let wire = serde_json::to_value(&profile).expect("profile serializes");
    assert_eq!(wire["floor_version"], FLOOR_VERSION);
    assert_eq!(wire["decision_id"], "d:1");
}

#[test]
fn the_same_record_gives_the_same_profile_whatever_order_it_was_read_in() {
    let mut forward = with_confidence("high");
    forward.options = vec![
        option("o:a", "A", Some("first")),
        option("o:b", "B", None),
        option("o:c", "C", None),
    ];
    forward.evidence = vec![
        evidence("e:1", Some(1), None, GroundingAdded::AtCapture),
        evidence("e:2", Some(2), Some("audit"), attributed_later()),
        evidence("e:3", Some(30), None, attributed_later()),
    ];
    forward.premises = vec![
        premise("d:1", Some(1), GroundingAdded::AtCapture),
        premise("d:2", Some(2), attributed_later()),
        premise("d:3", Some(30), attributed_later()),
    ];
    forward.hypotheses = vec![
        bet("h:1"),
        hypothesis(
            "h:2",
            HypothesisKind::Assumption,
            Some(2),
            attributed_later(),
        ),
        hypothesis("h:3", HypothesisKind::Bet, Some(30), attributed_later()),
    ];
    let mut backward = forward.clone();
    backward.options.reverse();
    backward.evidence.reverse();
    backward.premises.reverse();
    backward.hypotheses.reverse();

    assert_eq!(
        profile_from_record(&forward),
        profile_from_record(&backward)
    );
}

#[test]
fn node_ids_are_sorted_and_distinct() {
    let mut record = with_confidence("medium");
    record.options = vec![
        option("o:z", "Z", None),
        option("o:a", "A", None),
        option("o:m", "M", None),
    ];
    record.evidence = vec![
        evidence("e:z", Some(1), None, GroundingAdded::AtCapture),
        evidence("e:a", Some(2), None, GroundingAdded::AtCapture),
    ];
    record.premises = vec![
        premise("d:z", Some(1), GroundingAdded::AtCapture),
        premise("d:a", Some(2), GroundingAdded::AtCapture),
    ];
    record.hypotheses = vec![bet("h:z"), bet("h:a")];

    let profile = profile_from_record(&record);

    for (dimension, assessment) in profile.iter() {
        let Assessment::Assessed {
            node_ids, reasons, ..
        } = assessment
        else {
            continue;
        };
        let mut sorted = node_ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(*node_ids, sorted, "{dimension:?}");
        for reason in reasons {
            let mut sorted = reason.node_ids.clone();
            sorted.sort();
            assert_eq!(reason.node_ids, sorted, "{dimension:?} {:?}", reason.kind);
        }
    }
}

// ── Through the graph ─────────────────────────────────────────────────────────

/// A decision captured directly through the write layer: no classifier, no scorer, no API key.
#[test]
fn a_directly_captured_decision_gets_a_profile() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let actor = "human:alex";
    let chosen =
        commands.record_option(actor, "Gateway", "Route every call through one gateway")?;
    let described = commands.record_option(actor, "Direct", "Calls go straight to the service")?;
    let placeholder =
        commands.record_option(actor, "Queue", &default_option_description("Queue"))?;
    commands.record_evidence_with_id(
        actor,
        "e:load-test",
        "p95 stays under 200ms at 10x load",
        Some("load test 2026-09-20"),
        Uuid::new_v4(),
    )?;
    let decision_id = commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: actor,
        title: "Route calls through one gateway",
        rationale: "One choke point keeps the audit trail in one place",
        topic_keys: &["gateway".to_owned()],
        option_ids: &[chosen.clone(), described.clone(), placeholder.clone()],
        option_labels: &[
            "Gateway".to_owned(),
            "Direct".to_owned(),
            "Queue".to_owned(),
        ],
        chosen_option_id: Some(chosen.as_str()),
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &["e:load-test".to_owned()],
        quote: Some("Yes, route everything through the gateway"),
        question: Some("Should every call go through a single gateway?"),
        delegated_by: None,
        project: None,
    })?;
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;

    let profile = quality_profile_of(&graph, &decision_id)?.expect("the decision has a profile");

    assert_eq!(profile.decision_id, decision_id);
    assert_eq!(assessed(&profile.framing).0, Level::Partial);
    // The "Queue" placeholder does not count, so the alternatives are only partly described.
    assert_eq!(assessed(&profile.alternatives).0, Level::Partial);
    assert_eq!(
        reason_ids(&profile.alternatives, ReasonKind::AlternativesUndescribed),
        vec![placeholder]
    );
    assert_eq!(assessed(&profile.information).0, Level::Solid);
    assert_eq!(assessed(&profile.reasoning).0, Level::Partial);
    // Three options were recorded, so something was set against the one taken.
    assert_eq!(assessed(&profile.bias_exposure).0, Level::Partial);
    // No confidence was declared: not assessed, never inferred.
    assert_eq!(profile.calibration.level(), None);
    assert_eq!(profile.values_tradeoffs.level(), None);
    assert!(profile.attention.is_empty());
    Ok(())
}

/// A decision captured with a confidence and a premise decision through the write layer: the
/// premise, the confidence and the bet reach the profile through the graph.
#[test]
fn a_captured_premise_bet_and_confidence_reach_the_profile() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let actor = "human:alex";
    let one_store = commands.record_option(actor, "One store", "Keep a single store")?;
    let prior = commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: actor,
        title: "Keep one store",
        rationale: "Two stores double the operational load",
        topic_keys: &["storage".to_owned()],
        option_ids: std::slice::from_ref(&one_store),
        option_labels: &["One store".to_owned()],
        chosen_option_id: Some(one_store.as_str()),
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
        delegated_by: None,
        project: None,
    })?;
    let bet = commands.record_hypothesis_with_kind(
        actor,
        "Load stays flat",
        HypothesisKind::Bet,
        None,
        None,
    )?;
    let stay = commands.record_option(actor, "Stay", "Stay on the store we have")?;
    let decision_id = commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::Declared {
            premise_decision_ids: std::slice::from_ref(&prior),
            evidence_ids: &[],
            hypothesis_ids: std::slice::from_ref(&bet),
        },
        expressed_confidence: Some("high"),
        actor_id: actor,
        title: "Stay on one store",
        rationale: "Follows the earlier call",
        topic_keys: &["storage".to_owned()],
        option_ids: std::slice::from_ref(&stay),
        option_labels: &["Stay".to_owned()],
        chosen_option_id: Some(stay.as_str()),
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: std::slice::from_ref(&bet),
        evidence_ids: &[],
        quote: None,
        question: None,
        delegated_by: None,
        project: None,
    })?;
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;

    let profile = quality_profile_of(&graph, &decision_id)?.expect("the decision has a profile");

    assert_eq!(
        reason_ids(&profile.information, ReasonKind::PremiseCounted),
        vec![prior.clone()]
    );
    assert_eq!(
        reason_ids(&profile.information, ReasonKind::BetCounted),
        vec![bet.clone()]
    );
    assert_eq!(assessed(&profile.reasoning).0, Level::Partial);
    assert_eq!(
        reason_ids(&profile.reasoning, ReasonKind::PremiseLinked),
        vec![prior]
    );
    // Grounded on a prior decision as well as a bet: the confidence is compared, not flagged.
    assert_eq!(assessed(&profile.calibration).0, Level::Partial);
    assert!(profile.attention.is_empty());
    Ok(())
}

/// The profile of every decision in `floor_scenario`, on whichever backend `graph` is:
/// each floor lands on the rung the scenario was built for, and a decision that is not in the
/// graph has no profile. Shared by the memory, Postgres and Kuzu tests.
pub(crate) fn assert_scenario_profiles(graph: &impl GraphView) -> Result<()> {
    let profile_of = |decision_id: &str| -> Result<QualityProfile> {
        Ok(quality_profile_of(graph, decision_id)?
            .unwrap_or_else(|| panic!("{decision_id} has a profile")))
    };
    let levels = |decision_id: &str| -> Result<([Level; 5], Option<Level>)> {
        let profile = profile_of(decision_id)?;
        Ok((
            [
                assessed(&profile.framing).0,
                assessed(&profile.alternatives).0,
                assessed(&profile.information).0,
                assessed(&profile.reasoning).0,
                assessed(&profile.bias_exposure).0,
            ],
            profile.calibration.level(),
        ))
    };
    use Level::{None as N, Partial as P, Solid as S};
    // ([framing, alternatives, information, reasoning, bias exposure], calibration)
    let expected = [
        ("d:solid", ([P, S, S, P, P], None)),
        ("d:placeholders", ([N, P, P, P, P], None)),
        ("d:bare", ([N, N, N, P, N], None)),
        ("d:later-evidence", ([N, N, N, P, N], None)),
        ("d:backfilled", ([N, N, S, P, N], None)),
        ("d:mixed", ([N, N, P, P, N], None)),
        ("d:single", ([N, N, N, P, N], None)),
        ("d:undecided", ([N, S, N, P, P], None)),
        // A node a request named but nobody proposed: nothing recorded, nothing to count.
        ("d:stub", ([N, N, N, N, N], None)),
        ("d:premised", ([N, S, P, P, P], Some(P))),
        ("d:bet-high", ([N, N, P, P, N], Some(P))),
        ("d:high-nothing", ([N, N, N, P, N], Some(N))),
        ("d:backfilled-premise", ([N, N, P, P, N], None)),
        ("d:late-premise", ([N, N, N, P, N], None)),
        ("d:on-superseded", ([N, N, P, P, N], None)),
        ("d:challenged", ([N, N, P, P, P], None)),
        ("d:refuted-later", ([N, N, P, P, N], None)),
    ];
    for (decision_id, want) in expected {
        assert_eq!(levels(decision_id)?, want, "{decision_id}");
    }

    // The explanations behind the rungs, not only the rungs.
    let placeholders = profile_of("d:placeholders")?;
    assert_eq!(
        reason_ids(
            &placeholders.alternatives,
            ReasonKind::AlternativesUndescribed
        ),
        ids(&["d:placeholders:b", "d:placeholders:c"])
    );
    let later = profile_of("d:later-evidence")?;
    assert_eq!(
        reason_ids(&later.information, ReasonKind::EvidenceLater),
        ids(&["e:after"])
    );
    let backfilled = profile_of("d:backfilled")?;
    assert_eq!(
        reason_ids(&backfilled.information, ReasonKind::EvidenceCounted),
        ids(&["e:sourced"])
    );
    let mixed = profile_of("d:mixed")?;
    assert_eq!(
        reason_ids(&mixed.information, ReasonKind::EvidenceCounted),
        ids(&["e:unsourced"])
    );
    assert_eq!(
        reason_ids(&mixed.information, ReasonKind::EvidenceLater),
        ids(&["e:after-too"])
    );

    // What a decision rests on, and how the profile reads it.
    let premised = profile_of("d:premised")?;
    assert_eq!(
        reason_ids(&premised.information, ReasonKind::PremiseCounted),
        ids(&["d:solid"])
    );
    assert_eq!(
        reason_ids(&premised.reasoning, ReasonKind::PremiseLinked),
        ids(&["d:solid"])
    );
    // 2026-01-02 to 2026-03-01.
    assert_eq!(
        reason_text(&premised.bias_exposure, ReasonKind::PremiseAge),
        "age of the prior decisions it rests on when this was recorded: d:solid 58 days"
    );
    assert!(premised.attention.is_empty());

    let bet_high = profile_of("d:bet-high")?;
    assert_eq!(
        reason_ids(&bet_high.information, ReasonKind::BetCounted),
        ids(&["h:bet"])
    );
    assert_eq!(
        reason_ids(&bet_high.calibration, ReasonKind::RestsOnBetOnly),
        ids(&["h:bet"])
    );
    assert_eq!(
        bet_high.attention,
        vec![Attention {
            kind: AttentionKind::HighConfidenceOverBet,
            dimension: Dimension::Calibration,
            text: bet_high.attention[0].text.clone(),
            node_ids: ids(&["h:bet"]),
        }]
    );

    let high_nothing = profile_of("d:high-nothing")?;
    assert_eq!(
        high_nothing
            .attention
            .iter()
            .map(|line| line.kind)
            .collect::<Vec<_>>(),
        vec![AttentionKind::HighConfidenceOverNothing]
    );

    // Recorded before, attributed afterwards: counts. Recorded after: shown, never counted.
    let backfilled_premise = profile_of("d:backfilled-premise")?;
    assert_eq!(
        reason_ids(&backfilled_premise.information, ReasonKind::PremiseCounted),
        ids(&["d:solid"])
    );
    let late_premise = profile_of("d:late-premise")?;
    assert_eq!(
        reason_ids(&late_premise.information, ReasonKind::PremiseLater),
        ids(&["d:after-premise"])
    );
    assert_eq!(
        reason_ids(&late_premise.reasoning, ReasonKind::PremiseLater),
        ids(&["d:after-premise"])
    );

    // Superseded afterwards: a fact next to the premise, no change of level.
    let on_superseded = profile_of("d:on-superseded")?;
    assert_eq!(
        reason_ids(&on_superseded.information, ReasonKind::PremiseSuperseded),
        ids(&["d:new-premise", "d:old-premise"])
    );
    assert!(
        reason_text(&on_superseded.information, ReasonKind::PremiseSuperseded)
            .contains("since superseded (later)")
    );

    // Counter-evidence on record before the decision, and evidence that refuted only afterwards.
    let challenged = profile_of("d:challenged")?;
    assert_eq!(
        reason_ids(
            &challenged.bias_exposure,
            ReasonKind::CounterEvidenceRecorded
        ),
        ids(&["e:counter", "h:doubtful"])
    );
    let refuted_later = profile_of("d:refuted-later")?;
    assert_eq!(
        kinds_of(&refuted_later.bias_exposure),
        vec![ReasonKind::NoCounterOption, ReasonKind::NoCounterEvidence]
    );

    assert!(quality_profile_of(graph, "d:missing")?.is_none());
    Ok(())
}

#[test]
fn the_scenario_lands_every_floor_on_the_rung_it_was_built_for() -> Result<()> {
    let graph = floor_scenario()?.graph()?;

    assert_scenario_profiles(&graph)
}

#[test]
fn the_same_graph_gives_an_identical_profile_on_every_read() -> Result<()> {
    let scenario = floor_scenario()?;
    let graph = scenario.graph()?;
    let rebuilt = scenario.graph()?;

    for decision_id in FLOOR_SCENARIO_DECISIONS {
        let first = quality_profile_of(&graph, decision_id)?;
        assert_eq!(
            first,
            quality_profile_of(&graph, decision_id)?,
            "{decision_id}"
        );
        assert_eq!(
            first,
            quality_profile_of(&rebuilt, decision_id)?,
            "{decision_id}"
        );
        // ...and on the wire, byte for byte.
        assert_eq!(
            serde_json::to_string(&first).expect("serializes"),
            serde_json::to_string(&quality_profile_of(&rebuilt, decision_id)?).expect("serializes"),
            "{decision_id}"
        );
    }
    Ok(())
}

// ── Layer boundary ────────────────────────────────────────────────────────────

/// The profile is Layer 3: queries stay pure and the write layer stays dumb. Nothing in
/// `src/queries/` or `src/commands/` may reach for it.
#[test]
fn queries_and_commands_never_import_the_profile() {
    fn rust_files(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory reads") {
            let path = entry.expect("directory entry reads").path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for layer in ["queries", "commands"] {
        rust_files(&source.join(layer), &mut files);
    }
    assert!(
        !files.is_empty(),
        "found no query or command sources to check"
    );

    for path in files {
        let text = std::fs::read_to_string(&path).expect("source file reads");
        assert!(
            !text.contains("quality_profile"),
            "{} names the quality profile; the profile is Layer 3 and nothing below it may depend on it",
            path.display()
        );
    }
}
