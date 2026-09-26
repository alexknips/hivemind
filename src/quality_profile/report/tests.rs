// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::projector::GraphView;
use crate::queries::test_fixtures::{attention_scenario, ts, CountingGraph, ATTENTION_NOW};
use crate::queries::MAX_QUERY_RESULTS;
use crate::Result;

use super::*;

// ── helpers ───────────────────────────────────────────────────────────────────

fn now() -> DateTime<Utc> {
    ts(ATTENTION_NOW)
}

fn everything() -> ScanRequest {
    ScanRequest {
        limit: MAX_QUERY_RESULTS,
        ..ScanRequest::default()
    }
}

fn scan(graph: &impl GraphView, request: &ScanRequest) -> Result<QueryResponse<ScanReport>> {
    scan_decision_quality_at(graph, request, now())
}

fn attention(graph: &impl GraphView) -> Result<Vec<AttentionFinding>> {
    Ok(attention_findings_at(
        graph,
        &AttentionRequest {
            kinds: Vec::new(),
            limit: MAX_QUERY_RESULTS,
            cursor: None,
            ..AttentionRequest::default()
        },
        &AttentionConfig::default(),
        now(),
    )?
    .findings)
}

fn finding_ids(findings: &[ScanFinding]) -> BTreeSet<String> {
    findings
        .iter()
        .map(|scanned| scanned.finding.finding_id.clone())
        .collect()
}

fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("the response serializes")
}

/// No number in a response says how good a decision is: nothing is a fraction, and no key names
/// a grade.
pub(crate) fn assert_no_grade(value: &Value, path: &str) {
    const GRADE_KEYS: [&str; 6] = ["score", "tier", "grade", "composite", "rank", "confidence"];
    match value {
        Value::Number(number) => assert!(!number.is_f64(), "{path} is a fraction: {number}"),
        Value::Object(map) => {
            for (key, child) in map {
                assert!(
                    !GRADE_KEYS.contains(&key.as_str()),
                    "{path}.{key} names a grade"
                );
                assert_no_grade(child, &format!("{path}.{key}"));
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                assert_no_grade(child, &format!("{path}[{index}]"));
            }
        }
        _ => {}
    }
}

/// Every page of `request` walked through its cursors, as (page sizes, all findings).
fn walk(graph: &impl GraphView, request: &ScanRequest) -> Result<(Vec<usize>, Vec<ScanFinding>)> {
    let mut request = request.clone();
    let mut sizes = Vec::new();
    let mut findings = Vec::new();
    loop {
        let page = scan(graph, &request)?;
        assert_eq!(page.data.next_cursor.is_some(), page.truncated);
        sizes.push(page.data.findings.len());
        findings.extend(page.data.findings);
        match page.data.next_cursor {
            Some(cursor) => request.cursor = Some(cursor),
            None => return Ok((sizes, findings)),
        }
        assert!(sizes.len() < 10_000, "the walk does not end");
    }
}

/// What every backend must return from `attention_scenario`: the attention findings and nothing
/// else, each with the dimensions its kind bears on at the level the profile gives them, and the
/// profile and provenance of any decision. Shared by the memory, Postgres and Kuzu tests.
pub(crate) fn assert_report_scenario(graph: &impl GraphView) -> Result<()> {
    let expected = attention(graph)?;
    let all = scan(graph, &everything())?;
    assert_eq!(all.result_count, 13);
    assert!(!all.truncated);
    assert!(all.data.next_cursor.is_none());
    assert_eq!(all.data.as_of, now());
    assert_eq!(all.data.evidence_window_days, DEFAULT_EVIDENCE_WINDOW_DAYS);
    let findings: Vec<&AttentionFinding> = all.data.findings.iter().map(|f| &f.finding).collect();
    assert_eq!(findings, expected.iter().collect::<Vec<_>>());

    for scanned in &all.data.findings {
        let finding = &scanned.finding;
        let profile = quality_profile_of(graph, &finding.decision_id)?
            .expect("a flagged decision has a profile");
        let want: Vec<DimensionLine> = dimensions_for(finding.kind)
            .iter()
            .map(|dimension| DimensionLine {
                dimension: *dimension,
                assessment: profile.assessment(*dimension).clone(),
            })
            .collect();
        assert_eq!(scanned.dimensions, want, "{}", finding.finding_id);
    }

    // Paged, the same findings in the same order.
    let (sizes, paged) = walk(
        graph,
        &ScanRequest {
            limit: 4,
            ..ScanRequest::default()
        },
    )?;
    assert_eq!(sizes, [4, 4, 4, 1]);
    assert_eq!(paged, all.data.findings);

    // The profile of a decision, with the provenance its shapes state.
    for decision_id in ["d:bet-overdue", "d:p-contested", "d:p-standing"] {
        let report = score_decision(graph, decision_id)?
            .data
            .expect("the decision exists");
        assert_eq!(
            Some(report.profile.clone()),
            quality_profile_of(graph, decision_id)?,
            "{decision_id}"
        );
        let context = get_decision_context(graph, decision_id)?
            .data
            .expect("the decision has a context");
        assert_eq!(report.provenance, Provenance::of(&context), "{decision_id}");
    }
    let unreviewed = score_decision(graph, "d:bet-overdue")?
        .data
        .expect("exists");
    assert_eq!(unreviewed.provenance.authorship, AuthorshipShape::AgentOnly);
    assert_eq!(unreviewed.provenance.review, ReviewShape::Unreviewed);
    assert_eq!(unreviewed.provenance.line, Some(NOT_REVIEWED_BY_A_HUMAN));
    let disputed = score_decision(graph, "d:p-contested")?
        .data
        .expect("exists");
    assert_eq!(disputed.provenance.review, ReviewShape::Disputed);
    assert_eq!(disputed.provenance.line, None);
    let accepted = score_decision(graph, "d:p-standing")?.data.expect("exists");
    assert_eq!(
        accepted.provenance.authorship,
        AuthorshipShape::AgentProposedHumanAccepted
    );
    assert_eq!(accepted.provenance.line, None);

    let missing = score_decision(graph, "d:does-not-exist")?;
    assert_eq!(missing.result_count, 0);
    assert!(missing.data.is_none());

    // Suggestions: nothing is acknowledged, so either setting is the scan; with the first three
    // findings acknowledged, a page is filled from what remains.
    for exclude_acknowledged in [true, false] {
        let request = SuggestionsRequest {
            scan: everything(),
            exclude_acknowledged,
        };
        assert_eq!(get_suggestions_at(graph, &request, now())?.data, all.data);
    }
    let acknowledged = finding_ids(&all.data.findings[..3]);
    let page = scan_page_at(
        graph,
        &ScanRequest {
            limit: 4,
            ..ScanRequest::default()
        },
        acknowledged,
        now(),
    )?;
    assert_eq!(page.data.findings, all.data.findings[3..7]);
    assert!(page.truncated);
    Ok(())
}

// ── what a scan returns ───────────────────────────────────────────────────────

#[test]
fn a_scan_returns_the_attention_findings_with_their_dimensions_on_the_scenario() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    assert_report_scenario(&graph)
}

#[test]
fn each_kind_bears_on_the_dimensions_the_docs_name() {
    use Dimension::{Calibration, Information, Reasoning};
    use FindingKind::{
        AssumptionRefuted, BetFailed, BetPastCheckDate, EvidenceNotRechecked, PremiseRejected,
        PremiseSuperseded,
    };
    let table: [(FindingKind, &[Dimension]); 6] = [
        (BetPastCheckDate, &[Information, Calibration]),
        (PremiseSuperseded, &[Information, Reasoning]),
        (PremiseRejected, &[Information, Reasoning]),
        (AssumptionRefuted, &[Information]),
        (BetFailed, &[Information, Calibration]),
        (EvidenceNotRechecked, &[Information]),
    ];
    for (kind, dimensions) in table {
        assert_eq!(dimensions_for(kind), dimensions, "{kind:?}");
    }
    // A new kind has to say what it bears on: this fails to compile until it does.
    for kind in FindingKind::ALL {
        assert!(!dimensions_for(kind).is_empty(), "{kind:?}");
    }
}

#[test]
fn a_kind_filter_returns_only_that_kind_with_its_dimensions() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let page = scan(
        &graph,
        &ScanRequest {
            kinds: vec![FindingKind::BetPastCheckDate],
            ..everything()
        },
    )?;

    let ids: Vec<&str> = page
        .data
        .findings
        .iter()
        .map(|f| f.finding.decision_id.as_str())
        .collect();
    assert_eq!(ids, ["d:bet-overdue", "d:bet-overdue-direct"]);
    for scanned in &page.data.findings {
        let dimensions: Vec<Dimension> = scanned.dimensions.iter().map(|l| l.dimension).collect();
        assert_eq!(dimensions, [Dimension::Information, Dimension::Calibration]);
    }
    Ok(())
}

#[test]
fn the_evidence_window_is_the_callers_and_is_echoed_back() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let stale = |page: &QueryResponse<ScanReport>| {
        page.data
            .findings
            .iter()
            .filter(|f| f.finding.kind == FindingKind::EvidenceNotRechecked)
            .count()
    };

    let default = scan(&graph, &everything())?;
    let wide = scan(
        &graph,
        &ScanRequest {
            evidence_window_days: Some(10_000),
            ..everything()
        },
    )?;

    assert_eq!(default.data.evidence_window_days, 90);
    assert_eq!(stale(&default), 3);
    assert_eq!(wide.data.evidence_window_days, 10_000);
    assert_eq!(stale(&wide), 0);
    Ok(())
}

#[test]
fn a_truncated_page_says_so_and_names_where_it_resumes() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let first = scan(
        &graph,
        &ScanRequest {
            limit: 5,
            ..ScanRequest::default()
        },
    )?;

    assert_eq!(first.result_count, 5);
    assert!(first.truncated);
    let cursor = first
        .data
        .next_cursor
        .clone()
        .expect("a cursor to resume from");
    let second = scan(
        &graph,
        &ScanRequest {
            limit: 5,
            cursor: Some(cursor),
            ..ScanRequest::default()
        },
    )?;
    assert_eq!(second.result_count, 5);
    let ids = |page: &QueryResponse<ScanReport>| -> Vec<String> {
        page.data
            .findings
            .iter()
            .map(|f| f.finding.finding_id.clone())
            .collect()
    };
    assert!(ids(&first).iter().all(|id| !ids(&second).contains(id)));
    Ok(())
}

#[test]
fn a_cursor_that_no_scan_returned_is_refused() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let refused = scan(
        &graph,
        &ScanRequest {
            cursor: Some("not a cursor".to_owned()),
            ..ScanRequest::default()
        },
    );

    assert!(refused.is_err());
    Ok(())
}

#[test]
fn a_page_costs_the_attention_reads_plus_one_profile_read_per_decision_on_it() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let request = ScanRequest {
        limit: 6,
        ..ScanRequest::default()
    };

    let counting = CountingGraph::new(&graph);
    let page = scan(&counting, &request)?;
    let total = counting.queries();

    let attention_only = CountingGraph::new(&graph);
    attention_findings_at(
        &attention_only,
        &AttentionRequest {
            kinds: Vec::new(),
            limit: 6,
            cursor: None,
            ..AttentionRequest::default()
        },
        &AttentionConfig::default(),
        now(),
    )?;
    let mut decisions: Vec<&str> = page
        .data
        .findings
        .iter()
        .map(|f| f.finding.decision_id.as_str())
        .collect();
    decisions.sort_unstable();
    decisions.dedup();
    let mut profile_reads = 0;
    for decision_id in &decisions {
        let one = CountingGraph::new(&graph);
        quality_profile_of(&one, decision_id)?;
        profile_reads += one.queries();
    }

    assert_eq!(page.result_count, 6);
    assert_eq!(total, attention_only.queries() + profile_reads);
    Ok(())
}

// ── what suggestions return ───────────────────────────────────────────────────

#[test]
fn suggestions_leave_acknowledged_findings_out_unless_told_not_to() {
    assert!(SuggestionsRequest::default().exclude_acknowledged);
}

#[test]
fn nothing_is_acknowledged_yet_so_suggestions_are_the_findings_of_a_scan() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    assert!(acknowledged_finding_ids(&graph)?.is_empty());
    let scanned = scan(&graph, &everything())?;
    for exclude_acknowledged in [true, false] {
        let suggested = get_suggestions_at(
            &graph,
            &SuggestionsRequest {
                scan: everything(),
                exclude_acknowledged,
            },
            now(),
        )?;
        assert_eq!(suggested.data, scanned.data, "{exclude_acknowledged}");
        assert_eq!(suggested.result_count, scanned.result_count);
        assert_eq!(suggested.truncated, scanned.truncated);
    }
    Ok(())
}

#[test]
fn an_acknowledged_finding_is_left_out_and_the_page_is_filled_from_what_remains() -> Result<()> {
    let graph = attention_scenario()?.graph()?;
    let all = scan(&graph, &everything())?.data.findings;
    let acknowledged = finding_ids(&all[..3]);

    let first = scan_page_at(
        &graph,
        &ScanRequest {
            limit: 4,
            ..ScanRequest::default()
        },
        acknowledged.clone(),
        now(),
    )?;

    // The first three are gone and the page still holds four, each with its dimensions.
    assert_eq!(first.data.findings, all[3..7]);
    assert_eq!(first.result_count, 4);
    assert!(first.truncated);
    assert!(first
        .data
        .findings
        .iter()
        .all(|scanned| !scanned.dimensions.is_empty()));

    // Its cursor resumes after the seventh: the rest, and then nothing follows.
    let rest = scan_page_at(
        &graph,
        &ScanRequest {
            limit: 100,
            cursor: first.data.next_cursor.clone(),
            ..ScanRequest::default()
        },
        acknowledged,
        now(),
    )?;
    assert_eq!(rest.data.findings, all[7..]);
    assert!(!rest.truncated);
    assert!(rest.data.next_cursor.is_none());
    Ok(())
}

// ── what a score returns ──────────────────────────────────────────────────────

#[test]
fn the_provenance_line_appears_for_an_agent_alone_that_nobody_disputed() {
    use AuthorshipShape::{AgentOnly, AgentProposedHumanAccepted, HumanAuthored, Unknown};
    use ReviewShape::{Disputed, PeerReviewed, SelfAccepted, Unreviewed};
    let table = [
        (AgentOnly, Unreviewed, true),
        (AgentOnly, SelfAccepted, true),
        (AgentOnly, PeerReviewed, true),
        // Someone rejected it: the shape does not say the someone was not a human.
        (AgentOnly, Disputed, false),
        (AgentProposedHumanAccepted, Unreviewed, false),
        (AgentProposedHumanAccepted, PeerReviewed, false),
        (AgentProposedHumanAccepted, Disputed, false),
        (HumanAuthored, Unreviewed, false),
        (HumanAuthored, SelfAccepted, false),
        (Unknown, Unreviewed, false),
    ];
    for (authorship, review, marked) in table {
        let provenance = Provenance::from_shapes(authorship, review);
        assert_eq!(provenance.authorship, authorship);
        assert_eq!(provenance.review, review);
        assert_eq!(
            provenance.line,
            marked.then_some("not yet reviewed by a human"),
            "{authorship:?} / {review:?}"
        );
    }
}

#[test]
fn a_score_serializes_the_seven_dimensions_and_never_a_grade() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let response = score_decision(&graph, "d:bet-overdue")?;
    let json = to_json(&response);

    assert_no_grade(&json, "$");
    assert_eq!(json["result_count"], 1);
    assert_eq!(json["truncated"], false);
    let data = &json["data"];
    assert_eq!(data["decision_id"], "d:bet-overdue");
    assert_eq!(data["floor_version"], crate::quality_profile::FLOOR_VERSION);
    for dimension in [
        "framing",
        "alternatives",
        "information",
        "reasoning",
        "values_tradeoffs",
        "bias_exposure",
        "calibration",
    ] {
        let status = data[dimension]["status"].as_str().expect("a status");
        match status {
            "assessed" => {
                assert!(data[dimension]["level"].is_string(), "{dimension}");
                assert!(data[dimension]["reasons"].is_array(), "{dimension}");
                assert!(data[dimension]["node_ids"].is_array(), "{dimension}");
            }
            "not_assessed" => {
                assert!(data[dimension]["why"].is_string(), "{dimension}");
                assert!(data[dimension].get("level").is_none(), "{dimension}");
            }
            other => panic!("{dimension}: unknown status {other}"),
        }
    }
    assert_eq!(data["values_tradeoffs"]["status"], "not_assessed");
    assert!(data["attention"].is_array());
    assert_eq!(data["provenance"]["authorship"], "agent_only");
    assert_eq!(data["provenance"]["review"], "unreviewed");
    assert_eq!(data["provenance"]["line"], "not yet reviewed by a human");
    Ok(())
}

#[test]
fn a_decision_that_does_not_exist_scores_as_null_not_as_an_empty_profile() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let json = to_json(&score_decision(&graph, "d:nobody")?);

    assert_eq!(json["result_count"], 0);
    assert_eq!(json["data"], Value::Null);
    Ok(())
}

#[test]
fn a_scan_serializes_findings_with_their_dimensions_and_never_a_grade() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let json = to_json(&scan(
        &graph,
        &ScanRequest {
            limit: 3,
            ..ScanRequest::default()
        },
    )?);

    assert_no_grade(&json, "$");
    assert_eq!(json["result_count"], 3);
    assert_eq!(json["truncated"], true);
    let data = &json["data"];
    assert!(data["next_cursor"].is_string());
    assert_eq!(data["evidence_window_days"], 90);
    assert_eq!(data["as_of"], "2026-09-26T00:00:00Z");
    let first = &data["findings"][0];
    for key in ["finding_id", "kind", "decision_id", "reason"] {
        assert!(first[key].is_string(), "{key}");
    }
    assert!(first["node_ids"].is_array());
    let dimensions = first["dimensions"].as_array().expect("dimensions");
    assert!(!dimensions.is_empty());
    for line in dimensions {
        assert!(line["dimension"].is_string());
        assert!(line["status"].is_string());
    }
    Ok(())
}

#[test]
fn the_last_page_has_no_cursor() -> Result<()> {
    let graph = attention_scenario()?.graph()?;

    let json = to_json(&scan(&graph, &everything())?);

    assert_eq!(json["truncated"], false);
    assert!(json["data"].get("next_cursor").is_none());
    Ok(())
}

// ── the words a caller uses ───────────────────────────────────────────────────

#[test]
fn every_kind_name_parses_and_none_else() {
    for kind in FindingKind::ALL {
        assert_eq!(parse_kinds(&[kind.as_str()]), Ok(vec![kind]));
    }
    assert_eq!(parse_kinds::<&str>(&[]), Ok(Vec::new()));

    let refused = parse_kinds(&["bet_past_check_date", "high_concern"]).unwrap_err();

    assert!(refused.contains("`high_concern`"), "{refused}");
    for kind in FindingKind::ALL {
        assert!(refused.contains(kind.as_str()), "{refused}");
    }
}
