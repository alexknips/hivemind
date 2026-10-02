// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::collections::BTreeSet;

use super::acknowledged_finding_ids;
use crate::queries::test_fixtures::{ts, Scenario};
use crate::Result;

const NOW: &str = "2026-09-26T12:00:00Z";
const CREW: &str = "agent:claude:crew";

fn ids(scenario: &Scenario, now: &str) -> Result<BTreeSet<String>> {
    acknowledged_finding_ids(&scenario.graph()?, ts(now))
}

fn set(findings: &[&str]) -> BTreeSet<String> {
    findings.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn a_finding_is_acknowledged_once_a_surfaced_record_of_it_is_acknowledged() -> Result<()> {
    let s = Scenario::new();
    assert!(ids(&s, NOW)?.is_empty());

    let first = s.surfaced("finding-a", "d:1", CREW, "2026-09-01T00:00:00Z")?;
    s.surfaced("finding-b", "d:2", CREW, "2026-09-01T00:00:01Z")?;
    // Shown, not yet looked at: nothing is acknowledged.
    assert!(ids(&s, NOW)?.is_empty());

    s.acknowledged(&first, CREW, Some("seen"), None, "2026-09-02T00:00:00Z")?;
    assert_eq!(ids(&s, NOW)?, set(&["finding-a"]));
    Ok(())
}

#[test]
fn every_action_hides_the_finding_and_so_does_an_acknowledgement_with_none() -> Result<()> {
    let s = Scenario::new();
    for (finding, action) in [
        ("finding-seen", Some("seen")),
        ("finding-acted", Some("acted")),
        ("finding-dismissed", Some("dismissed")),
        ("finding-bare", None),
    ] {
        let notification = s.surfaced(finding, "d:1", CREW, "2026-09-01T00:00:00Z")?;
        s.acknowledged(&notification, CREW, action, None, "2026-09-02T00:00:00Z")?;
    }
    assert_eq!(
        ids(&s, NOW)?,
        set(&[
            "finding-seen",
            "finding-acted",
            "finding-dismissed",
            "finding-bare"
        ])
    );
    Ok(())
}

#[test]
fn a_snoozed_acknowledgement_hides_the_finding_only_until_the_snooze_ends() -> Result<()> {
    let s = Scenario::new();
    let notification = s.surfaced("finding-a", "d:1", CREW, "2026-09-01T00:00:00Z")?;
    s.acknowledged(
        &notification,
        CREW,
        Some("seen"),
        Some("2026-09-27T00:00:00Z"),
        "2026-09-26T00:00:00Z",
    )?;

    assert_eq!(ids(&s, "2026-09-26T23:59:59Z")?, set(&["finding-a"]));
    // From the moment it ends the acknowledgement no longer hides the finding.
    assert!(ids(&s, "2026-09-27T00:00:00Z")?.is_empty());
    assert!(ids(&s, "2026-10-30T00:00:00Z")?.is_empty());
    Ok(())
}

#[test]
fn another_acknowledged_surfacing_of_the_same_finding_still_hides_it() -> Result<()> {
    let s = Scenario::new();
    let snoozed = s.surfaced("finding-a", "d:1", CREW, "2026-09-01T00:00:00Z")?;
    s.acknowledged(
        &snoozed,
        CREW,
        None,
        Some("2026-09-02T00:00:00Z"),
        "2026-09-01T12:00:00Z",
    )?;
    let again = s.surfaced("finding-a", "d:1", "human:alex", "2026-09-10T00:00:00Z")?;
    assert!(
        ids(&s, NOW)?.is_empty(),
        "the only acknowledgement has lapsed"
    );

    s.acknowledged(
        &again,
        "human:alex",
        Some("dismissed"),
        None,
        "2026-09-11T00:00:00Z",
    )?;
    assert_eq!(ids(&s, NOW)?, set(&["finding-a"]));
    Ok(())
}

#[test]
fn an_acknowledgement_that_names_no_surfaced_finding_hides_nothing() -> Result<()> {
    let s = Scenario::new();
    // No event created this notification: the graph keeps a bare node with `ack_at` and no
    // finding. A blocker notification's acknowledgement looks the same from here.
    s.acknowledged(
        "notification-of-a-blocker",
        "human:alex",
        None,
        None,
        "2026-09-02T00:00:00Z",
    )?;
    assert!(ids(&s, NOW)?.is_empty());
    Ok(())
}
