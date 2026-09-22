// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::commands::Commands;
use crate::events::{ProjectAnchorKind, ProjectLinkKind};
use crate::ledger::InMemoryEventLedger;
use crate::Result;

use super::*;

#[test]
fn list_projects_returns_registered_projects_with_links_and_anchors() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    commands.register_project("human:alice", "platform", Some("Platform"), None)?;
    commands.register_project(
        "human:alice",
        "billing",
        Some("Billing"),
        Some("Per-seat and per-org pricing decisions"),
    )?;
    commands.register_project("human:alice", "auth", Some("Auth"), None)?;
    commands.link_project(
        "human:alice",
        "billing",
        "platform",
        ProjectLinkKind::PartOf,
    )?;
    commands.link_project("human:alice", "billing", "auth", ProjectLinkKind::DependsOn)?;
    commands.anchor_project("human:alice", "billing", ProjectAnchorKind::Rig, "billing")?;

    let response = list_projects(&ledger, &ProjectListRequest::default())?;
    assert_eq!(response.data.total_matches, 3);
    assert!(!response.truncated);

    let billing = response
        .data
        .items
        .iter()
        .find(|project| project.handle == "billing")
        .expect("billing present");
    assert_eq!(billing.display_name.as_deref(), Some("Billing"));
    assert_eq!(
        billing.purpose.as_deref(),
        Some("Per-seat and per-org pricing decisions")
    );
    assert!(!billing.personal);
    assert_eq!(
        billing.part_of.as_ref().map(|fact| fact.to.as_str()),
        Some("platform")
    );
    assert_eq!(billing.depends_on.len(), 1);
    assert_eq!(billing.depends_on[0].to, "auth");
    assert_eq!(billing.anchors.len(), 1);
    assert_eq!(billing.anchors[0].kind, "rig");
    assert_eq!(billing.anchors[0].value, "billing");
    assert!(billing.registered_event_origin.is_some());

    // Handles come back sorted, matching the graph's `ORDER BY node.id` convention.
    let handles: Vec<&str> = response
        .data
        .items
        .iter()
        .map(|project| project.handle.as_str())
        .collect();
    assert_eq!(handles, vec!["auth", "billing", "platform"]);

    Ok(())
}

#[test]
fn list_projects_pages_with_truncated_flag_and_cursor() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);

    for handle in ["alpha", "beta", "gamma"] {
        commands.register_project("human:alice", handle, None, None)?;
    }

    let first_page = list_projects(
        &ledger,
        &ProjectListRequest {
            limit: 2,
            cursor: None,
        },
    )?;
    assert_eq!(first_page.data.items.len(), 2);
    assert!(first_page.truncated);
    let next_cursor = first_page
        .data
        .next_cursor
        .clone()
        .expect("next_cursor present when truncated");

    let second_page = list_projects(
        &ledger,
        &ProjectListRequest {
            limit: 2,
            cursor: Some(next_cursor),
        },
    )?;
    assert_eq!(second_page.data.items.len(), 1);
    assert!(!second_page.truncated);
    assert!(second_page.data.next_cursor.is_none());

    Ok(())
}

#[test]
fn get_project_returns_not_found_for_unknown_handle() -> Result<()> {
    let ledger = InMemoryEventLedger::new();

    let response = get_project(&ledger, "nonexistent")?;
    assert_eq!(response.data, ProjectOutcome::NotFound);
    assert_eq!(response.result_count, 0);

    Ok(())
}

#[test]
fn get_project_resolves_personal_address_without_registration() -> Result<()> {
    let ledger = InMemoryEventLedger::new();

    let response = get_project(&ledger, "personal:human:alice")?;
    match response.data {
        ProjectOutcome::Found { project } => {
            assert_eq!(project.handle, "personal:human:alice");
            assert!(project.personal);
            assert!(project.registered_event_origin.is_none());
            assert!(project.anchors.is_empty());
            assert!(project.part_of.is_none());
            assert!(project.depends_on.is_empty());
        }
        other => panic!("expected Found, got {other:?}"),
    }
    assert_eq!(response.result_count, 1);

    Ok(())
}

#[test]
fn get_project_returns_found_for_registered_project() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", Some("Billing"), None)?;

    let response = get_project(&ledger, "billing")?;
    match response.data {
        ProjectOutcome::Found { project } => {
            assert_eq!(project.handle, "billing");
            assert_eq!(project.display_name.as_deref(), Some("Billing"));
            assert!(!project.personal);
        }
        other => panic!("expected Found, got {other:?}"),
    }

    Ok(())
}

#[test]
fn unlink_retracts_the_link_from_subsequent_reads() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "platform", None, None)?;
    commands.register_project("human:alice", "billing", None, None)?;
    commands.link_project(
        "human:alice",
        "billing",
        "platform",
        ProjectLinkKind::PartOf,
    )?;
    commands.unlink_project(
        "human:alice",
        "billing",
        "platform",
        ProjectLinkKind::PartOf,
    )?;

    let response = get_project(&ledger, "billing")?;
    match response.data {
        ProjectOutcome::Found { project } => {
            assert!(
                project.part_of.is_none(),
                "unlinked parent must not still show as active"
            );
        }
        other => panic!("expected Found, got {other:?}"),
    }

    Ok(())
}

#[test]
fn unanchor_retracts_the_anchor_from_subsequent_reads() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    commands.anchor_project("human:alice", "billing", ProjectAnchorKind::Rig, "billing")?;
    commands.unanchor_project("human:alice", "billing", ProjectAnchorKind::Rig, "billing")?;

    let response = get_project(&ledger, "billing")?;
    match response.data {
        ProjectOutcome::Found { project } => {
            assert!(project.anchors.is_empty());
        }
        other => panic!("expected Found, got {other:?}"),
    }

    Ok(())
}
