// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::projector::GraphView;
use crate::quality_profile::Dimension;
use crate::queries::test_fixtures::attention_scenario;
use crate::queries::{
    get_decision_quality_candidates, get_failure_attribution, DecisionQualityCandidatesRequest,
};
use crate::Result;

use super::*;

/// A decision of `attention_scenario` that rests on an overdue bet and has a chosen option.
const A_DECISION: &str = "d:bet-overdue";

/// What every backend must return from `attention_scenario`: the profile's seven dimensions as
/// conditions, one group per (dimension, level or `not_assessed`), each decision in exactly one
/// group of each dimension, and everything else in the report exactly what the analysis without
/// the profile says. Shared by the memory, Postgres and Kuzu tests.
pub(crate) fn assert_failure_mode_scenario(graph: &impl GraphView) -> Result<()> {
    let request = FailureAttributionRequest::default();
    let plain = get_failure_attribution(graph, &request)?;
    let report = analyze_failure_modes(graph, &request)?;

    // The profile only adds groups: who failed and every other breakdown are untouched.
    assert!(plain.data.by_condition.is_empty());
    assert_eq!(report.result_count, plain.result_count);
    assert_eq!(report.data.corpus_stats, plain.data.corpus_stats);
    assert_eq!(report.data.by_authorship, plain.data.by_authorship);
    assert_eq!(report.data.by_review, plain.data.by_review);
    assert_eq!(report.data.by_delegation, plain.data.by_delegation);
    assert_eq!(report.data.by_source, plain.data.by_source);
    assert_eq!(
        report.data.by_context_richness,
        plain.data.by_context_richness
    );

    let total = report.data.corpus_stats.total_decisions;
    let failed = report.data.corpus_stats.failed_decisions;
    assert!(
        0 < failed && failed < total,
        "the scenario has decisions on both sides: {failed} of {total} failed"
    );

    // Every decision is in exactly one group of each dimension, and only the seven are named.
    let names: BTreeSet<&str> = Dimension::ALL.iter().map(|d| d.as_str()).collect();
    let named: BTreeSet<&str> = report
        .data
        .by_condition
        .iter()
        .map(|group| group.dimension.as_str())
        .collect();
    assert_eq!(named, names);
    for dimension in Dimension::ALL {
        let groups: Vec<_> = report
            .data
            .by_condition
            .iter()
            .filter(|group| group.dimension == dimension.as_str())
            .collect();
        assert_eq!(
            groups.iter().map(|group| group.total).sum::<usize>(),
            total,
            "{dimension:?}"
        );
        assert_eq!(
            groups.iter().map(|group| group.failed).sum::<usize>(),
            failed,
            "{dimension:?}"
        );
    }

    // No model is in the loop, so Values / Tradeoffs is one group: not assessed. It is not `none`.
    let values: Vec<_> = report
        .data
        .by_condition
        .iter()
        .filter(|group| group.dimension == Dimension::ValuesTradeoffs.as_str())
        .collect();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].group_label, NOT_ASSESSED);
    assert_eq!(values[0].total, total);

    // The same groups, counted from each decision's profile as it serializes and its outcome.
    let outcomes = get_decision_quality_candidates(
        graph,
        &DecisionQualityCandidatesRequest {
            limit: 0,
            ..DecisionQualityCandidatesRequest::default()
        },
    )?
    .data;
    assert_eq!(outcomes.len(), total);
    let mut expected: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    for outcome in &outcomes {
        let profile = quality_profile_of(graph, &outcome.decision_id)?
            .expect("a decision with an outcome has a profile");
        let wire = serde_json::to_value(&profile).expect("the profile serializes");
        for dimension in Dimension::ALL {
            let entry = &wire[dimension.as_str()];
            let label = match entry["status"].as_str() {
                Some("assessed") => entry["level"]
                    .as_str()
                    .expect("an assessed level is a name"),
                _ => NOT_ASSESSED,
            };
            let bucket = expected
                .entry((dimension.as_str().to_owned(), label.to_owned()))
                .or_insert((0, 0));
            bucket.0 += 1;
            if !outcome.held_up {
                bucket.1 += 1;
            }
        }
    }
    let actual: BTreeMap<(String, String), (usize, usize)> = report
        .data
        .by_condition
        .iter()
        .map(|group| {
            (
                (group.dimension.clone(), group.group_label.clone()),
                (group.total, group.failed),
            )
        })
        .collect();
    assert_eq!(actual, expected);

    // The scenario puts decisions on more than one rung, so agreeing is not agreeing on emptiness.
    let information_levels = actual
        .keys()
        .filter(|(dimension, _)| dimension == Dimension::Information.as_str())
        .count();
    assert!(information_levels >= 2, "{actual:?}");

    // Every group takes part in the findings like any other: named by dimension and label.
    let findings = &report.data.findings;
    assert!(findings
        .iter()
        .any(|finding| names.contains(finding.dimension.as_str())));
    Ok(())
}

#[test]
fn the_dimensions_group_failures_on_the_attention_scenario() -> Result<()> {
    assert_failure_mode_scenario(&attention_scenario()?.graph()?)
}

#[test]
fn a_profile_states_seven_conditions_in_dimension_order() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let profile = quality_profile_of(&graph, A_DECISION)?.expect("the decision has a profile");

    let conditions = profile_conditions(&profile);

    let dimensions: Vec<&str> = conditions.iter().map(|c| c.dimension.as_str()).collect();
    let want: Vec<&str> = Dimension::ALL.iter().map(|d| d.as_str()).collect();
    assert_eq!(dimensions, want);
    for condition in &conditions {
        let level = profile
            .assessment(
                *Dimension::ALL
                    .iter()
                    .find(|d| d.as_str() == condition.dimension)
                    .expect("a condition names a dimension"),
            )
            .level();
        match level {
            Some(level) => assert_eq!(condition.label, level.as_str()),
            None => assert_eq!(condition.label, NOT_ASSESSED),
        }
    }
    // Values / Tradeoffs is judged only.
    assert_eq!(conditions[4].dimension, "values_tradeoffs");
    assert_eq!(conditions[4].label, NOT_ASSESSED);
    Ok(())
}

#[test]
fn wire_names_are_the_names_serialization_gives() -> Result<()> {
    for dimension in Dimension::ALL {
        assert_eq!(
            serde_json::to_value(dimension).expect("a dimension serializes"),
            Value::String(dimension.as_str().to_owned())
        );
    }
    for level in [Level::None, Level::Partial, Level::Solid] {
        assert_eq!(
            serde_json::to_value(level).expect("a level serializes"),
            Value::String(level.as_str().to_owned())
        );
    }
    Ok(())
}
