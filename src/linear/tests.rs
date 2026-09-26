// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::quality_profile::{
    Assessment, AttentionFinding, Dimension, DimensionLine, FindingKind, Level, Reason, ReasonKind,
};

fn finding(kind: FindingKind, dimensions: Vec<DimensionLine>) -> ScanFinding {
    ScanFinding {
        finding: AttentionFinding {
            finding_id: "finding-0123456789abcdef".to_owned(),
            kind,
            decision_id: "d-001".to_owned(),
            basis_at: None,
            node_ids: vec!["d-001".to_owned(), "h-001".to_owned()],
            reason: "the bet h-001 was to be checked by 2026-01-01".to_owned(),
        },
        dimensions,
    }
}

fn information_and_calibration() -> Vec<DimensionLine> {
    vec![
        DimensionLine {
            dimension: Dimension::Information,
            assessment: Assessment::Assessed {
                level: Level::Partial,
                reasons: vec![Reason {
                    kind: ReasonKind::BetCounted,
                    text: "1 bet counted".to_owned(),
                    node_ids: vec!["h-001".to_owned()],
                }],
                node_ids: vec!["h-001".to_owned()],
            },
        },
        DimensionLine {
            dimension: Dimension::Calibration,
            assessment: Assessment::NotAssessed {
                why: "No confidence was declared at capture".to_owned(),
            },
        },
    ]
}

#[test]
fn format_title_with_title() {
    let scanned = finding(FindingKind::PremiseSuperseded, Vec::new());
    let t = format_issue_title(&scanned, Some("Deploy to prod"));
    assert_eq!(t, "[HiveMind] premise_superseded: Deploy to prod");
}

#[test]
fn format_title_without_title() {
    let scanned = finding(FindingKind::PremiseSuperseded, Vec::new());
    let t = format_issue_title(&scanned, None);
    assert_eq!(t, "[HiveMind] premise_superseded: decision d-001");
}

#[test]
fn format_title_truncates_long_titles() {
    let scanned = finding(FindingKind::PremiseSuperseded, Vec::new());
    let long = "A".repeat(100);
    let t = format_issue_title(&scanned, Some(&long));
    assert!(t.len() <= 140);
    assert!(t.contains('…'));
}

#[test]
fn format_description_contains_required_fields() {
    let scanned = finding(FindingKind::BetPastCheckDate, information_and_calibration());
    let desc = format_issue_description(&scanned, Some("https://hivemind.example.com/"));
    assert_eq!(
        desc,
        "## Decision needs a look\n\n\
         **Decision ID:** `d-001`  \n\
         **Finding:** bet_past_check_date  \n\
         **Finding ID:** `finding-0123456789abcdef`\n\n\
         **Link:** https://hivemind.example.com/decisions/d-001\n\n\
         ### Why it needs a look\n\n\
         the bet h-001 was to be checked by 2026-01-01\n\n\
         ### Node IDs\n\n\
         - `d-001`\n\
         - `h-001`\n\n\
         ### Dimensions it bears on\n\n\
         - **Information** — partial\n\
         \x20 - 1 bet counted (`h-001`)\n\
         - **Calibration** — not assessed: No confidence was declared at capture\n\n\
         ---\n\
         *Filed by HiveMind quality-scan. Human review required — HiveMind never auto-acts on decisions.*\n"
    );
}

#[test]
fn format_description_omits_the_link_and_empty_sections_when_there_is_nothing_for_them() {
    let mut scanned = finding(FindingKind::BetPastCheckDate, Vec::new());
    scanned.finding.node_ids.clear();
    let desc = format_issue_description(&scanned, None);
    assert!(!desc.contains("**Link:**"));
    assert!(!desc.contains("### Node IDs"));
    assert!(!desc.contains("### Dimensions it bears on"));
    assert!(desc.contains("### Why it needs a look"));
}

#[test]
fn format_description_carries_no_score_or_tier() {
    let scanned = finding(FindingKind::BetPastCheckDate, information_and_calibration());
    let desc = format_issue_description(&scanned, None);
    let words: Vec<String> = desc
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .collect();
    for banned in ["score", "scores", "tier", "tiers"] {
        assert!(!words.iter().any(|word| word == banned), "{banned}: {desc}");
    }
}
