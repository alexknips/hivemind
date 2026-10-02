// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;
use crate::commands::CommandContext;
use crate::events::{EventProvenance, EventType};
use crate::quality_profile::{
    scan_decision_quality_at, Assessment, AttentionFinding, Dimension, DimensionLine, FindingKind,
    Level, Reason, ReasonKind,
};
use crate::queries::test_fixtures::{attention_scenario, ts, Scenario, ATTENTION_NOW};
use crate::queries::MAX_QUERY_RESULTS;

fn finding(kind: FindingKind, dimensions: Vec<DimensionLine>) -> ScanFinding {
    ScanFinding {
        finding: AttentionFinding {
            finding_id: "finding-0123456789abcdef".to_owned(),
            kind,
            decision_id: "d-001".to_owned(),
            decision_title: "d-001".to_owned(),
            basis_at: None,
            node_ids: vec!["d-001".to_owned(), "h-001".to_owned()],
            reason: "the bet 'Latency stays low' was to be checked by 2026-01-01".to_owned(),
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
    let mut scanned = finding(FindingKind::PremiseSuperseded, Vec::new());
    scanned.finding.decision_title = "Deploy to prod".to_owned();
    let t = format_issue_title(&scanned);
    assert_eq!(t, "[HiveMind] premise_superseded: Deploy to prod");
}

#[test]
fn format_title_falls_back_to_the_id_when_the_decision_has_no_title() {
    let mut scanned = finding(FindingKind::PremiseSuperseded, Vec::new());
    scanned.finding.decision_title = scanned.finding.decision_id.clone();
    let t = format_issue_title(&scanned);
    assert_eq!(t, "[HiveMind] premise_superseded: d-001");
}

#[test]
fn format_title_truncates_long_titles() {
    let mut scanned = finding(FindingKind::PremiseSuperseded, Vec::new());
    scanned.finding.decision_title = "A".repeat(100);
    let t = format_issue_title(&scanned);
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
         **Decision:** d-001  \n\
         **Decision ID:** `d-001`  \n\
         **Finding:** bet_past_check_date  \n\
         **Finding ID:** `finding-0123456789abcdef`\n\n\
         **Link:** https://hivemind.example.com/decisions/d-001\n\n\
         ### Why it needs a look\n\n\
         the bet 'Latency stays low' was to be checked by 2026-01-01\n\n\
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

// ── the connector files each finding once (hivemind-m306.4.3) ─────────────────

const CONNECTOR: &str = "agent:linear:connector";

fn evidence_nobody_rechecked(limit: usize) -> ScanRequest {
    ScanRequest {
        kinds: vec![FindingKind::EvidenceNotRechecked],
        limit,
        ..ScanRequest::default()
    }
}

fn ticket(number: usize) -> CreatedIssue {
    CreatedIssue {
        id: format!("issue-{number}"),
        identifier: format!("ENG-{number}"),
        url: format!("https://linear.app/team/issue/ENG-{number}"),
    }
}

fn connector_commands(scenario: &Scenario) -> Commands<'_, impl EventLedger> {
    Commands::new_with_context(
        scenario.ledger(),
        CommandContext::local(EventProvenance::agent(CONNECTOR)),
    )
}

/// One run of the connector against the ledger as it stands, the way `quality-scan` does it: a
/// graph rebuilt from the ledger, the findings nobody has acknowledged, a ticket for each. The
/// ticket titles it filed are appended to `tickets`.
fn run_connector(
    scenario: &Scenario,
    request: &ScanRequest,
    tickets: &mut Vec<String>,
) -> Result<Vec<FiledFinding>> {
    let graph = scenario.graph()?;
    let pending = pending_findings(&graph, request, ts(ATTENTION_NOW))?;
    file_findings(
        &pending.data.findings,
        &connector_commands(scenario),
        CONNECTOR,
        None,
        |title, _description| {
            tickets.push(title.to_owned());
            Ok(ticket(tickets.len()))
        },
    )
}

#[test]
fn a_repeated_run_over_unchanged_decisions_files_nothing_new() -> Result<()> {
    let scenario = attention_scenario()?;
    let request = evidence_nobody_rechecked(MAX_QUERY_RESULTS);
    let scanned = scan_decision_quality_at(&scenario.graph()?, &request, ts(ATTENTION_NOW))?
        .data
        .findings;
    assert!(scanned.len() >= 2, "the scenario has several to file");

    let mut tickets = Vec::new();
    let first = run_connector(&scenario, &request, &mut tickets)?;
    assert_eq!(
        first
            .iter()
            .map(|filed| filed.finding_id.as_str())
            .collect::<Vec<_>>(),
        scanned
            .iter()
            .map(|scanned| scanned.finding.finding_id.as_str())
            .collect::<Vec<_>>(),
        "the first run files each finding of the scan, once"
    );
    assert_eq!(tickets.len(), scanned.len());

    for run in 2..=4 {
        let again = run_connector(&scenario, &request, &mut tickets)?;
        assert!(again.is_empty(), "run {run} filed {again:?}");
    }
    assert_eq!(
        tickets.len(),
        scanned.len(),
        "no ticket after the first run"
    );
    Ok(())
}

#[test]
fn a_changed_signal_files_exactly_one_ticket() -> Result<()> {
    let scenario = attention_scenario()?;
    let request = evidence_nobody_rechecked(MAX_QUERY_RESULTS);
    let mut tickets = Vec::new();
    let first = run_connector(&scenario, &request, &mut tickets)?;
    let filed_before = tickets.len();
    let stale = first
        .iter()
        .find(|filed| filed.decision_id == "d:ev-stale")
        .expect("d:ev-stale is flagged for evidence nobody re-checked") // ubs:ignore: test-only; panicking is correct in tests
        .finding_id
        .clone();

    // Newer evidence is linked to one decision, still outside the evidence window: its finding
    // rests on other evidence now, so it is a new finding. Every other finding is as it was.
    scenario.evidence(
        "e:mid",
        "Benchmark from April",
        Some("bench run"),
        "2026-04-20T00:00:00Z",
    )?;
    scenario.relation(
        "BASED_ON",
        "d:ev-stale",
        "e:mid",
        "human:alex",
        None,
        "2026-09-16T00:00:00Z",
    )?;

    let changed = run_connector(&scenario, &request, &mut tickets)?;
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].decision_id, "d:ev-stale");
    assert_ne!(changed[0].finding_id, stale);
    assert_eq!(changed[0].kind, "evidence_not_rechecked");
    assert_eq!(tickets.len(), filed_before + 1);

    assert!(run_connector(&scenario, &request, &mut tickets)?.is_empty());
    assert_eq!(tickets.len(), filed_before + 1);
    Ok(())
}

#[test]
fn every_filed_finding_is_recorded_as_acted_on_over_the_linear_channel() -> Result<()> {
    let scenario = attention_scenario()?;
    let request = evidence_nobody_rechecked(MAX_QUERY_RESULTS);
    let before = scenario.ledger().latest_offset()?;
    let filed = run_connector(&scenario, &request, &mut Vec::new())?;

    let events: Vec<_> = scenario
        .ledger()
        .read(before, 10_000)?
        .into_iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::SuggestionSurfaced | EventType::NotificationAcknowledged
            )
        })
        .collect();
    assert_eq!(
        events.len(),
        2 * filed.len(),
        "a surfacing and an acknowledgement each"
    );
    for event in &events {
        assert_eq!(event.actor_id, CONNECTOR);
    }
    let surfaced: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == EventType::SuggestionSurfaced)
        .collect();
    let mut recorded: Vec<&str> = surfaced
        .iter()
        .map(|event| event.payload["finding_id"].as_str().unwrap_or_default())
        .collect();
    let mut expected: Vec<&str> = filed.iter().map(|done| done.finding_id.as_str()).collect();
    recorded.sort_unstable();
    expected.sort_unstable();
    assert_eq!(recorded, expected);
    for event in &surfaced {
        assert_eq!(event.payload["channel"], ACK_CHANNEL);
    }
    for event in events
        .iter()
        .filter(|event| event.event_type == EventType::NotificationAcknowledged)
    {
        assert_eq!(event.payload["action"], "acted");
    }
    Ok(())
}

#[test]
fn a_backlog_longer_than_the_limit_is_worked_through_one_page_per_run() -> Result<()> {
    let scenario = attention_scenario()?;
    let everything = evidence_nobody_rechecked(MAX_QUERY_RESULTS);
    let total =
        scan_decision_quality_at(&scenario.graph()?, &everything, ts(ATTENTION_NOW))?.result_count;
    assert!(total >= 3, "the scenario has a backlog to page through");

    let one_at_a_time = evidence_nobody_rechecked(1);
    let mut tickets = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for run in 1..=total {
        let filed = run_connector(&scenario, &one_at_a_time, &mut tickets)?;
        assert_eq!(filed.len(), 1, "run {run}");
        assert!(
            seen.insert(filed[0].finding_id.clone()),
            "run {run} refiled one"
        );
    }
    assert!(run_connector(&scenario, &one_at_a_time, &mut tickets)?.is_empty());
    assert_eq!(seen.len(), total);
    Ok(())
}

#[test]
fn a_ticket_that_cannot_be_filed_stops_the_run_and_loses_and_repeats_nothing() -> Result<()> {
    let scenario = attention_scenario()?;
    let request = evidence_nobody_rechecked(MAX_QUERY_RESULTS);
    let pending = pending_findings(&scenario.graph()?, &request, ts(ATTENTION_NOW))?
        .data
        .findings;
    assert!(pending.len() >= 3);

    let mut attempts = 0;
    let error = file_findings(
        &pending,
        &connector_commands(&scenario),
        CONNECTOR,
        None,
        |_, _| {
            attempts += 1;
            if attempts == 2 {
                Err(CliError::InvalidInput("Linear API returned 503".to_owned()).into())
            } else {
                Ok(ticket(attempts))
            }
        },
    )
    .expect_err("the second ticket cannot be filed"); // ubs:ignore: test-only; panicking is correct in tests
    let message = error.to_string();
    assert!(message.contains("503"), "{message}");
    assert!(
        message.contains("ENG-1"),
        "names what was already filed: {message}"
    );
    assert_eq!(attempts, 2, "nothing after the failure was attempted");

    // The next run files everything but the first finding: not lost, not filed twice.
    let mut tickets = Vec::new();
    let rest = run_connector(&scenario, &request, &mut tickets)?;
    assert_eq!(rest.len(), pending.len() - 1);
    assert!(rest
        .iter()
        .all(|filed| filed.finding_id != pending[0].finding.finding_id));
    Ok(())
}

#[test]
fn a_ticket_filed_but_not_recorded_is_named_so_it_is_not_filed_twice() -> Result<()> {
    let scenario = attention_scenario()?;
    let request = evidence_nobody_rechecked(MAX_QUERY_RESULTS);
    let pending = pending_findings(&scenario.graph()?, &request, ts(ATTENTION_NOW))?
        .data
        .findings;

    // The CLI's default provenance has no source_ref, which an acknowledgement is refused without.
    let commands = Commands::new_with_context(
        scenario.ledger(),
        CommandContext::local(EventProvenance::cli()),
    );
    let error = file_findings(&pending, &commands, CONNECTOR, None, |_, _| Ok(ticket(1)))
        .expect_err("the acknowledgement is refused"); // ubs:ignore: test-only; panicking is correct in tests
    let message = error.to_string();
    assert!(message.contains("ENG-1"), "{message}");
    assert!(message.contains("acknowledge_suggestion"), "{message}");
    assert!(
        message.contains(&pending[0].finding.finding_id),
        "{message}"
    );

    // Nothing was recorded, so the finding is still pending.
    let after = pending_findings(&scenario.graph()?, &request, ts(ATTENTION_NOW))?
        .data
        .findings;
    assert_eq!(after, pending);
    Ok(())
}
