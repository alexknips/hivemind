// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::collections::BTreeSet;

use crate::linear::format_issue_description;
use crate::projector::GraphView;
use crate::queries::test_fixtures::{attention_scenario, ts, ATTENTION_NOW};
use crate::queries::MAX_QUERY_RESULTS;
use crate::Result;

use super::*;
use crate::quality_profile::{scan_decision_quality_at, Reason, ReasonKind, ScanRequest};

// ── helpers ───────────────────────────────────────────────────────────────────

fn assessed(level: Level, reasons: Vec<(&str, Vec<&str>)>) -> Assessment {
    let reasons: Vec<Reason> = reasons
        .into_iter()
        .map(|(text, ids)| Reason {
            kind: ReasonKind::EvidenceCounted,
            text: text.to_owned(),
            node_ids: ids.into_iter().map(str::to_owned).collect(),
        })
        .collect();
    let node_ids = reasons
        .iter()
        .flat_map(|reason| reason.node_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Assessment::Assessed {
        level,
        reasons,
        node_ids,
    }
}

/// The words in `text`, lower-cased: the way "the word score" is told from "frontier".
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .collect()
}

fn assert_grades_nothing(text: &str) {
    let words = words(text);
    for banned in ["score", "scores", "tier", "tiers", "grade", "composite"] {
        assert!(!words.contains(banned), "`{banned}` in: {text}");
    }
}

// ── one dimension ─────────────────────────────────────────────────────────────

#[test]
fn a_dimension_is_its_level_and_one_bullet_per_reason_with_its_ids() {
    let assessment = assessed(
        Level::Partial,
        vec![
            ("2 evidence items counted", vec!["e:a", "e:b"]),
            ("no evidence item says where it was observed", vec![]),
        ],
    );

    assert_eq!(
        dimension_markdown(Dimension::Information, &assessment),
        "- **Information** — partial\n  - 2 evidence items counted (`e:a`, `e:b`)\n  - no evidence item says where it was observed"
    );
}

#[test]
fn a_dimension_that_was_not_assessed_says_why_and_names_no_level() {
    let assessment = Assessment::NotAssessed {
        why: "No confidence was declared at capture".to_owned(),
    };

    let line = dimension_markdown(Dimension::Calibration, &assessment);

    assert_eq!(
        line,
        "- **Calibration** — not assessed: No confidence was declared at capture"
    );
    for level in ["none", "partial", "solid"] {
        assert!(!words(&line).contains(level), "{level} in {line}");
    }
}

#[test]
fn every_level_has_a_word_and_every_dimension_a_label() {
    let words: Vec<&str> = [Level::None, Level::Partial, Level::Solid]
        .into_iter()
        .map(level_word)
        .collect();
    assert_eq!(words, ["none", "partial", "solid"]);

    let labels: Vec<&str> = Dimension::ALL.into_iter().map(dimension_label).collect();
    assert_eq!(
        labels,
        [
            "Framing",
            "Alternatives",
            "Information",
            "Reasoning",
            "Values / Tradeoffs",
            "Bias exposure",
            "Calibration"
        ]
    );
}

#[test]
fn a_line_break_in_a_reason_cannot_end_its_bullet() {
    let assessment = assessed(
        Level::Solid,
        vec![("first line\n\n- **Forged**   second\tline", vec!["e:a"])],
    );

    let text = dimension_markdown(Dimension::Information, &assessment);

    assert_eq!(
        text,
        "- **Information** — solid\n  - first line - **Forged** second line (`e:a`)"
    );
    assert_eq!(text.lines().count(), 2);
}

// ── the profile as a section ──────────────────────────────────────────────────

/// What every backend must give for the attention scenario: the whole profile of a decision as a
/// section, and each finding's ticket carrying the dimension lines of its kind. Shared by the
/// memory, Postgres and Kuzu tests.
pub(crate) fn assert_markdown_scenario(graph: &impl GraphView) -> Result<()> {
    let now = ts(ATTENTION_NOW);

    for decision_id in [
        "d:bet-overdue",
        "d:p-contested",
        "d:p-standing",
        "d:ev-none",
    ] {
        let profile = quality_profile_of(graph, decision_id)?.expect("the decision has a profile");
        let section = decision_log_section(graph, decision_id)?;

        assert_eq!(section, profile_markdown(&profile), "{decision_id}");
        assert!(
            section.starts_with("Floor rules version 2: what the record states, not whether it is sound.\n\n- **Framing** — "),
            "{decision_id}: {section}"
        );
        // Seven dimensions, in order, each once.
        let bullets: Vec<&str> = section
            .lines()
            .filter(|line| line.starts_with("- **"))
            .filter_map(|line| line.strip_prefix("- **")?.split("**").next())
            .take(7)
            .collect();
        let labels: Vec<&str> = Dimension::ALL.into_iter().map(dimension_label).collect();
        assert_eq!(bullets, labels, "{decision_id}");
        // Every reason and every id the profile holds is in the text.
        for (dimension, assessment) in profile.iter() {
            assert!(
                section.contains(&dimension_markdown(dimension, assessment)),
                "{decision_id} {dimension:?}: {section}"
            );
            match assessment {
                Assessment::Assessed { reasons, .. } => {
                    for reason in reasons {
                        assert!(
                            section.contains(&reason.text),
                            "{decision_id}: {}",
                            reason.text
                        );
                        for id in &reason.node_ids {
                            assert!(section.contains(id.as_str()), "{decision_id}: {id}");
                        }
                    }
                }
                Assessment::NotAssessed { why } => assert!(section.contains(why.as_str())),
            }
        }
        assert_eq!(
            section.contains("Worth a second look:"),
            !profile.attention.is_empty(),
            "{decision_id}"
        );
        for attention in &profile.attention {
            assert!(section.contains(&attention.text), "{decision_id}");
        }
        assert_grades_nothing(&section);
    }

    // A decision that is not in the graph is an error, never an empty section.
    assert!(decision_log_section(graph, "d:does-not-exist").is_err());

    // Every finding's ticket carries the dimension lines of its kind, the way the profile has them.
    let page = scan_decision_quality_at(
        graph,
        &ScanRequest {
            limit: MAX_QUERY_RESULTS,
            ..ScanRequest::default()
        },
        now,
    )?;
    assert!(!page.data.findings.is_empty());
    for scanned in &page.data.findings {
        let body = format_issue_description(scanned, Some("https://hivemind.example.com"));
        let finding = &scanned.finding;

        assert!(
            body.contains(&format!("`{}`", finding.finding_id)),
            "{body}"
        );
        assert!(body.contains(&finding.reason), "{body}");
        assert!(!scanned.dimensions.is_empty(), "{}", finding.finding_id);
        for line in &scanned.dimensions {
            assert!(
                body.contains(&dimension_markdown(line.dimension, &line.assessment)),
                "{}: {body}",
                finding.finding_id
            );
        }
        let shown: Vec<&str> = body
            .lines()
            .filter(|line| line.starts_with("- **"))
            .collect();
        assert_eq!(shown.len(), scanned.dimensions.len(), "{body}");
        assert_grades_nothing(&body);
    }
    Ok(())
}

#[test]
fn the_profile_and_the_tickets_read_as_markdown_on_the_scenario() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    assert_markdown_scenario(&graph)
}
