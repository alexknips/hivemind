// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::path::Path;

use uuid::Uuid;

use crate::commands::{Commands, DecisionProposalInput, Grounding};
use crate::ledger::InMemoryEventLedger;
use crate::mcp::args::default_option_description;
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::queries::test_fixtures::{floor_scenario, FLOOR_SCENARIO_DECISIONS};
use crate::queries::{EvidenceFact, GroundingAdded, OptionFact, RecordFacts};
use crate::Result;

use super::*;

// ── helpers ───────────────────────────────────────────────────────────────────

/// A record that states nothing beyond a rationale. The decision was recorded at ledger offset 10.
fn facts() -> RecordFacts {
    RecordFacts {
        decision_id: "d:1".to_owned(),
        event_origin: Some(10),
        question: None,
        rationale: Some("Because the numbers said so".to_owned()),
        chosen_option_id: None,
        options: Vec::new(),
        evidence: Vec::new(),
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
fn information_is_none_without_evidence() {
    let profile = profile_from_record(&facts());

    let (level, kinds, node_ids) = assessed(&profile.information);
    assert_eq!(level, Level::None);
    assert_eq!(kinds, vec![ReasonKind::NoEvidenceLinked]);
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
            vec![ReasonKind::NoEvidenceLinked, ReasonKind::EvidenceLater]
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

// ── Not assessed ──────────────────────────────────────────────────────────────

#[test]
fn values_bias_and_calibration_are_not_assessed_and_say_why() {
    let profile = profile_from_record(&facts());

    for dimension in [
        Dimension::ValuesTradeoffs,
        Dimension::BiasExposure,
        Dimension::Calibration,
    ] {
        let assessment = profile.assessment(dimension);
        let Assessment::NotAssessed { why } = assessment else {
            panic!("{dimension:?} must be not assessed, got {assessment:?}");
        };
        assert!(!why.trim().is_empty(), "{dimension:?} must say why");
        assert_eq!(assessment.level(), None);
    }
}

#[test]
fn a_not_assessed_entry_never_carries_a_level_on_the_wire() {
    let profile = profile_from_record(&facts());

    let wire = serde_json::to_value(&profile).expect("profile serializes");

    for dimension in ["values_tradeoffs", "bias_exposure", "calibration"] {
        let entry = &wire[dimension];
        assert_eq!(entry["status"], "not_assessed", "{dimension}");
        assert!(entry["why"].is_string(), "{dimension}");
        assert!(entry.get("level").is_none(), "{dimension} carries a level");
        assert!(entry.get("reasons").is_none(), "{dimension}");
    }
    for dimension in ["framing", "alternatives", "information", "reasoning"] {
        let entry = &wire[dimension];
        assert_eq!(entry["status"], "assessed", "{dimension}");
        assert!(entry["level"].is_string(), "{dimension}");
        assert!(entry["reasons"].is_array(), "{dimension}");
        assert!(entry["node_ids"].is_array(), "{dimension}");
    }
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
    let mut forward = facts();
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
    let mut backward = forward.clone();
    backward.options.reverse();
    backward.evidence.reverse();

    assert_eq!(
        profile_from_record(&forward),
        profile_from_record(&backward)
    );
}

#[test]
fn node_ids_are_sorted_and_distinct() {
    let mut record = facts();
    record.options = vec![
        option("o:z", "Z", None),
        option("o:a", "A", None),
        option("o:m", "M", None),
    ];
    record.evidence = vec![
        evidence("e:z", Some(1), None, GroundingAdded::AtCapture),
        evidence("e:a", Some(2), None, GroundingAdded::AtCapture),
    ];

    let profile = profile_from_record(&record);

    for dimension in [Dimension::Alternatives, Dimension::Information] {
        let Assessment::Assessed { node_ids, .. } = profile.assessment(dimension) else {
            panic!("{dimension:?} is assessed");
        };
        let mut sorted = node_ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(*node_ids, sorted, "{dimension:?}");
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
    assert_eq!(profile.values_tradeoffs.level(), None);
    Ok(())
}

/// The profile of every decision in `floor_scenario`, on whichever backend `graph` is:
/// each floor lands on the rung the scenario was built for, and a decision that is not in the
/// graph has no profile. Shared by the memory, Postgres and Kuzu tests.
pub(crate) fn assert_scenario_profiles(graph: &impl GraphView) -> Result<()> {
    let levels = |decision_id: &str| -> Result<[Level; 4]> {
        let profile = quality_profile_of(graph, decision_id)?
            .unwrap_or_else(|| panic!("{decision_id} has a profile"));
        Ok([
            assessed(&profile.framing).0,
            assessed(&profile.alternatives).0,
            assessed(&profile.information).0,
            assessed(&profile.reasoning).0,
        ])
    };
    use Level::{None as N, Partial as P, Solid as S};
    // [framing, alternatives, information, reasoning]
    let expected = [
        ("d:solid", [P, S, S, P]),
        ("d:placeholders", [N, P, P, P]),
        ("d:bare", [N, N, N, P]),
        ("d:later-evidence", [N, N, N, P]),
        ("d:backfilled", [N, N, S, P]),
        ("d:mixed", [N, N, P, P]),
        ("d:single", [N, N, N, P]),
        ("d:undecided", [N, S, N, P]),
        // A node a request named but nobody proposed: nothing recorded, nothing to count.
        ("d:stub", [N, N, N, N]),
    ];
    for (decision_id, want) in expected {
        assert_eq!(levels(decision_id)?, want, "{decision_id}");
    }

    // The explanations behind the rungs, not only the rungs.
    let placeholders = quality_profile_of(graph, "d:placeholders")?.expect("profile");
    assert_eq!(
        reason_ids(
            &placeholders.alternatives,
            ReasonKind::AlternativesUndescribed
        ),
        ids(&["d:placeholders:b", "d:placeholders:c"])
    );
    let later = quality_profile_of(graph, "d:later-evidence")?.expect("profile");
    assert_eq!(
        reason_ids(&later.information, ReasonKind::EvidenceLater),
        ids(&["e:after"])
    );
    let backfilled = quality_profile_of(graph, "d:backfilled")?.expect("profile");
    assert_eq!(
        reason_ids(&backfilled.information, ReasonKind::EvidenceCounted),
        ids(&["e:sourced"])
    );
    let mixed = quality_profile_of(graph, "d:mixed")?.expect("profile");
    assert_eq!(
        reason_ids(&mixed.information, ReasonKind::EvidenceCounted),
        ids(&["e:unsourced"])
    );
    assert_eq!(
        reason_ids(&mixed.information, ReasonKind::EvidenceLater),
        ids(&["e:after-too"])
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
