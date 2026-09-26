// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::commands::Commands;
use crate::commands::{DecisionProposalInput, DeterminedProject, Grounding, SupersedeInput};
use crate::events::{ProjectAnchorKind, ProjectLinkKind};
use crate::ledger::InMemoryEventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph};
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

/// Records decisions into a ledger for the decision-list tests: owns the option and topic
/// data a `DecisionProposalInput` borrows, so one call proposes one decision.
struct DecisionRecorder<'a> {
    commands: &'a Commands<'a, InMemoryEventLedger>,
    option_id: String,
    option_labels: [String; 1],
    topic_keys: [String; 1],
}

impl<'a> DecisionRecorder<'a> {
    fn new(commands: &'a Commands<'a, InMemoryEventLedger>) -> Result<Self> {
        Ok(Self {
            commands,
            option_id: commands.record_option("human:alice", "A", "Option A")?,
            option_labels: ["Option A".to_owned()],
            topic_keys: ["topic".to_owned()],
        })
    }

    fn record(&self, actor_id: &str, title: &str, project: Option<&str>) -> Result<String> {
        self.record_with(actor_id, title, project, false)
    }

    fn record_with(
        &self,
        actor_id: &str,
        title: &str,
        project: Option<&str>,
        still_proposed: bool,
    ) -> Result<String> {
        // A registered project's vocabulary is explicit (hivemind-zywz): declare the one
        // topic these fixtures use before capturing under it.
        if let Some(handle) = project {
            for topic_key in &self.topic_keys {
                self.commands
                    .declare_project_topic(actor_id, handle, topic_key)?;
            }
        }
        self.commands.propose_decision(DecisionProposalInput {
            actor_id,
            title,
            rationale: "The review list has to show what it is listing",
            topic_keys: &self.topic_keys,
            option_ids: std::slice::from_ref(&self.option_id),
            option_labels: &self.option_labels,
            chosen_option_id: Some(self.option_id.as_str()),
            decided_by: None,
            delegated_by: None,
            still_proposed,
            hypothesis_ids: &[],
            evidence_ids: &[],
            quote: None,
            question: None,
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            project: project.map(DeterminedProject::stated),
        })
    }
}

fn graph_of(ledger: &InMemoryEventLedger) -> Result<MemoryGraph> {
    let graph = MemoryGraph::default();
    rebuild_graph(ledger, &graph)?;
    Ok(graph)
}

fn list_decisions(
    graph: &MemoryGraph,
    handle: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<QueryResponse<ProjectDecisionsOutcome>> {
    decisions_in_project(
        graph,
        &ProjectDecisionsRequest {
            handle: handle.to_owned(),
            limit,
            cursor: cursor.map(str::to_owned),
        },
    )
}

fn found(response: QueryResponse<ProjectDecisionsOutcome>) -> ProjectDecisionsPage {
    match response.data {
        ProjectDecisionsOutcome::Found(page) => page,
        other => panic!("expected Found, got {other:?}"),
    }
}

#[test]
fn decisions_in_project_lists_a_shared_projects_decisions_with_status_and_provenance() -> Result<()>
{
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", Some("Billing"), None)?;
    commands.register_project("human:alice", "platform", None, None)?;
    let recorder = DecisionRecorder::new(&commands)?;

    let per_seat = recorder.record("human:alice", "Price per seat", Some("billing"))?;
    let open = recorder.record_with(
        "agent:claude:session-1",
        "Bill monthly",
        Some("billing"),
        true,
    )?;
    recorder.record("human:alice", "Adopt the platform SDK", Some("platform"))?;
    recorder.record("human:alice", "Kept to myself", None)?;

    let response = list_decisions(&graph_of(&ledger)?, "billing", 25, None)?;
    assert_eq!(response.result_count, 2);
    assert!(!response.truncated);
    let page = found(response);
    assert_eq!(page.handle, "billing");
    assert!(!page.personal);
    assert_eq!(page.owner, None);
    assert_eq!(page.total_matches, 2);
    assert_eq!(page.next_cursor, None);

    // Oldest first, and each item carries who recorded it and how its project was decided.
    let ids: Vec<&str> = page
        .items
        .iter()
        .map(|item| item.decision_id.as_str())
        .collect();
    assert_eq!(ids, vec![per_seat.as_str(), open.as_str()]);

    let per_seat_item = &page.items[0];
    assert_eq!(per_seat_item.title, "Price per seat");
    assert_eq!(per_seat_item.status, DecisionStatus::Accepted);
    assert_eq!(per_seat_item.proposed_by.as_deref(), Some("human:alice"));
    assert_eq!(per_seat_item.session, None);
    assert_eq!(per_seat_item.project_source.as_deref(), Some("stated"));
    assert!(per_seat_item.event_origin.is_some());

    let open_item = &page.items[1];
    assert_eq!(open_item.status, DecisionStatus::Proposed);
    assert_eq!(
        open_item.proposed_by.as_deref(),
        Some("agent:claude:session-1")
    );
    assert_eq!(open_item.session.as_deref(), Some("session-1"));

    Ok(())
}

#[test]
fn a_personal_project_lists_every_session_of_one_agent_tool_and_shows_each_session() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    let recorder = DecisionRecorder::new(&commands)?;

    // Two sessions of one tool share one personal project; another tool, a human, and a
    // decision filed under a shared project do not belong to it.
    let first = recorder.record("agent:claude:session-1", "Retry with backoff", None)?;
    let other_tool = recorder.record("agent:codex:session-1", "Cache the token", None)?;
    let second = recorder.record("agent:claude:session-2", "Queue the exports", None)?;
    let human = recorder.record("human:alice", "Ship on Fridays", None)?;
    recorder.record(
        "agent:claude:session-3",
        "Filed in billing",
        Some("billing"),
    )?;

    let graph = graph_of(&ledger)?;
    let page = found(list_decisions(&graph, "personal:agent:claude", 25, None)?);
    assert_eq!(page.handle, "personal:agent:claude");
    assert!(page.personal);
    assert_eq!(page.owner.as_deref(), Some("agent:claude"));
    assert_eq!(page.total_matches, 2);
    let listed: Vec<(&str, Option<&str>)> = page
        .items
        .iter()
        .map(|item| (item.decision_id.as_str(), item.session.as_deref()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (first.as_str(), Some("session-1")),
            (second.as_str(), Some("session-2")),
        ]
    );
    assert!(page
        .items
        .iter()
        .all(|item| item.project_source.as_deref() == Some("personal_fallback")));

    // Naming the actor with its session lists the same per-tool project, under its
    // canonical address.
    let with_session = found(list_decisions(
        &graph,
        "personal:agent:claude:session-2",
        25,
        None,
    )?);
    assert_eq!(with_session.handle, "personal:agent:claude");
    assert_eq!(with_session.items, page.items);

    let codex = found(list_decisions(&graph, "personal:agent:codex", 25, None)?);
    assert_eq!(codex.items.len(), 1);
    assert_eq!(codex.items[0].decision_id, other_tool);

    let alice = found(list_decisions(&graph, "personal:human:alice", 25, None)?);
    assert_eq!(alice.owner.as_deref(), Some("human:alice"));
    assert_eq!(alice.items.len(), 1);
    assert_eq!(alice.items[0].decision_id, human);
    assert_eq!(alice.items[0].session, None);

    Ok(())
}

#[test]
fn decisions_in_project_pages_with_truncated_flag_cursor_and_the_whole_count() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    let recorder = DecisionRecorder::new(&commands)?;
    let ids = ["First", "Second", "Third"]
        .into_iter()
        .map(|title| recorder.record("human:alice", title, Some("billing")))
        .collect::<Result<Vec<_>>>()?;

    let graph = graph_of(&ledger)?;
    let first = list_decisions(&graph, "billing", 2, None)?;
    assert!(first.truncated);
    assert_eq!(first.result_count, 2);
    let first = found(first);
    assert_eq!(first.total_matches, 3, "the count covers the whole project");
    assert_eq!(first.next_cursor.as_deref(), Some("2"));
    assert_eq!(first.items[0].decision_id, ids[0]);
    assert_eq!(first.items[1].decision_id, ids[1]);

    let second = list_decisions(&graph, "billing", 2, first.next_cursor.as_deref())?;
    assert!(!second.truncated);
    let second = found(second);
    assert_eq!(second.cursor.as_deref(), Some("2"));
    assert_eq!(second.total_matches, 3);
    assert_eq!(second.next_cursor, None);
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].decision_id, ids[2]);

    // A cursor past the end is an empty page, not an error.
    let past = list_decisions(&graph, "billing", 2, Some("9"))?;
    assert!(!past.truncated);
    assert!(found(past).items.is_empty());

    Ok(())
}

#[test]
fn decisions_in_project_shows_a_superseded_decision_as_superseded() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    let recorder = DecisionRecorder::new(&commands)?;
    let old = recorder.record("human:alice", "Price per org", Some("billing"))?;
    let outcome = commands.supersede(SupersedeInput {
        actor_id: "human:alice",
        old_decision_id: &old,
        new_title: "Price per seat",
        new_rationale: "Per-seat pricing replaces per-org because accounts vary too much",
        topic_keys: &["topic".to_owned()],
        option_labels: &["Per seat".to_owned()],
        chosen_option_label: Some("Per seat"),
        hypothesis_ids: &[],
        evidence_ids: &[],
        project: None,
        grounding: None,
        expressed_confidence: None,
    })?;

    let page = found(list_decisions(&graph_of(&ledger)?, "billing", 25, None)?);
    let statuses: Vec<(&str, DecisionStatus)> = page
        .items
        .iter()
        .map(|item| (item.decision_id.as_str(), item.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            (old.as_str(), DecisionStatus::Superseded),
            // `supersede` proposes the replacement but never auto-accepts it
            // (commands::HiveMindCommands::supersede, `still_proposed: true`).
            (outcome.new_decision_id.as_str(), DecisionStatus::Proposed),
        ],
        "the replacement inherits the project, and the replaced decision says so"
    );

    Ok(())
}

#[test]
fn decisions_in_project_says_not_found_for_an_unregistered_handle_but_empty_for_a_known_one(
) -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    let graph = graph_of(&ledger)?;

    // A typo is never an empty list.
    let missing = list_decisions(&graph, "billng", 25, None)?;
    assert_eq!(missing.result_count, 0);
    assert!(!missing.truncated);
    assert_eq!(
        missing.data,
        ProjectDecisionsOutcome::NotFound {
            handle: "billng".to_owned()
        }
    );

    // A registered project nobody recorded into yet is data: empty, and it says so.
    let empty = found(list_decisions(&graph, "billing", 25, None)?);
    assert_eq!(empty.total_matches, 0);
    assert!(empty.items.is_empty());

    // A personal address always resolves, even before its owner has recorded anything.
    let personal = found(list_decisions(&graph, "personal:human:nobody", 25, None)?);
    assert!(personal.personal);
    assert_eq!(personal.owner.as_deref(), Some("human:nobody"));
    assert_eq!(personal.total_matches, 0);

    Ok(())
}

#[test]
fn decisions_in_project_rejects_an_empty_address_and_a_bad_cursor() -> Result<()> {
    let graph = graph_of(&InMemoryEventLedger::new())?;

    assert!(list_decisions(&graph, "  ", 25, None).is_err());
    assert!(list_decisions(&graph, "personal:", 25, None).is_err());
    assert!(list_decisions(&graph, "personal:  ", 25, None).is_err());
    assert!(list_decisions(&graph, "personal:human:alice", 25, Some("later")).is_err());

    Ok(())
}

#[test]
fn get_project_by_anchor_finds_the_project_holding_an_active_anchor() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", Some("Billing"), None)?;
    commands.register_project("human:alice", "auth", None, None)?;
    commands.anchor_project(
        "human:alice",
        "billing",
        ProjectAnchorKind::Rig,
        "billing-rig",
    )?;
    commands.anchor_project(
        "human:alice",
        "auth",
        ProjectAnchorKind::Folder,
        "services/auth",
    )?;

    let found = get_project_by_anchor(&ledger, ProjectAnchorKind::Rig, "billing-rig")?;
    assert_eq!(found.result_count, 1);
    match found.data {
        ProjectOutcome::Found { project } => assert_eq!(project.handle, "billing"),
        other => panic!("expected Found, got {other:?}"),
    }

    // The kind is part of the key: a folder anchor is not a rig anchor with the same value.
    let wrong_kind = get_project_by_anchor(&ledger, ProjectAnchorKind::Rig, "services/auth")?;
    assert_eq!(wrong_kind.result_count, 0);
    assert_eq!(wrong_kind.data, ProjectOutcome::NotFound);

    let unknown = get_project_by_anchor(&ledger, ProjectAnchorKind::Rig, "no-such-rig")?;
    assert_eq!(unknown.data, ProjectOutcome::NotFound);

    assert!(
        get_project_by_anchor(&ledger, ProjectAnchorKind::Rig, "  ").is_err(),
        "an empty anchor value is a malformed question, not a miss"
    );
    Ok(())
}

#[test]
fn get_project_by_anchor_ignores_an_unanchored_anchor() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    commands.anchor_project(
        "human:alice",
        "billing",
        ProjectAnchorKind::Rig,
        "billing-rig",
    )?;
    commands.unanchor_project(
        "human:alice",
        "billing",
        ProjectAnchorKind::Rig,
        "billing-rig",
    )?;

    let response = get_project_by_anchor(&ledger, ProjectAnchorKind::Rig, "billing-rig")?;
    assert_eq!(response.data, ProjectOutcome::NotFound);
    Ok(())
}

#[test]
fn get_project_ancestries_walks_part_of_nearest_parent_first_in_one_read() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    for handle in ["city", "platform", "billing", "auth", "loner"] {
        commands.register_project("human:alice", handle, None, None)?;
    }
    commands.link_project("human:alice", "platform", "city", ProjectLinkKind::PartOf)?;
    commands.link_project(
        "human:alice",
        "billing",
        "platform",
        ProjectLinkKind::PartOf,
    )?;
    commands.link_project("human:alice", "auth", "platform", ProjectLinkKind::PartOf)?;
    // A dependency is not a parent: only `part_of` builds the ancestry.
    commands.link_project(
        "human:alice",
        "loner",
        "billing",
        ProjectLinkKind::DependsOn,
    )?;

    let response = get_project_ancestries(
        &ledger,
        &[
            "billing".to_owned(),
            "city".to_owned(),
            "loner".to_owned(),
            "not-registered".to_owned(),
        ],
    )?;

    let ancestries: Vec<(&str, Vec<&str>)> = response
        .data
        .iter()
        .map(|item| {
            (
                item.handle.as_str(),
                item.ancestors.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        ancestries,
        vec![
            ("billing", vec!["platform", "city"]),
            ("city", vec![]),
            ("loner", vec![]),
            ("not-registered", vec![]),
        ],
        "in the order asked, nearest parent first; a miss has no ancestors"
    );
    assert_eq!(response.result_count, 4);
    assert!(!response.truncated);
    Ok(())
}

#[test]
fn get_project_ancestries_stops_at_a_part_of_cycle() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    for handle in ["alpha", "beta", "gamma"] {
        commands.register_project("human:alice", handle, None, None)?;
    }
    commands.link_project("human:alice", "alpha", "beta", ProjectLinkKind::PartOf)?;
    commands.link_project("human:alice", "beta", "gamma", ProjectLinkKind::PartOf)?;
    commands.link_project("human:alice", "gamma", "alpha", ProjectLinkKind::PartOf)?;

    let response = get_project_ancestries(&ledger, &["alpha".to_owned()])?;

    assert_eq!(
        response.data[0].ancestors,
        vec!["beta".to_owned(), "gamma".to_owned()],
        "the walk ends before it would come back around to alpha"
    );
    Ok(())
}

#[test]
fn get_project_ancestries_ignores_a_removed_part_of_link() -> Result<()> {
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

    let response = get_project_ancestries(&ledger, &["billing".to_owned()])?;

    assert!(response.data[0].ancestors.is_empty(), "no silent staleness");
    Ok(())
}

#[test]
fn get_project_ancestries_refuses_an_empty_handle() {
    let ledger = InMemoryEventLedger::new();

    assert!(
        get_project_ancestries(&ledger, &["  ".to_owned()]).is_err(),
        "an empty handle is a malformed question, not a miss"
    );
}

#[test]
fn project_show_lists_the_declared_topics_and_where_each_was_declared() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "billing", None, None)?;
    let first = commands.declare_project_topic("human:alice", "billing", "Pricing")?;
    commands.declare_project_topic("human:bob", "billing", "auth")?;
    // A key already there is not a second fact.
    commands.declare_project_topic("human:bob", "billing", "pricing")?;
    commands.register_project("human:alice", "platform", None, None)?;

    let billing = match get_project(&ledger, "billing")?.data {
        ProjectOutcome::Found { project } => project,
        other => panic!("expected Found, got {other:?}"),
    };
    let keys: Vec<&str> = billing
        .topics
        .iter()
        .map(|topic| topic.topic_key.as_str())
        .collect();
    assert_eq!(keys, vec!["auth", "pricing"], "in key order, each once");
    let pricing = billing
        .topics
        .iter()
        .find(|topic| topic.topic_key == "pricing")
        .expect("pricing declared");
    assert_eq!(
        Some(pricing.event_origin),
        first
            .event_id
            .map(|event_id| i64::try_from(event_id).expect("offset fits")),
        "the origin is the declaration that added the key"
    );

    let platform = match get_project(&ledger, "platform")?.data {
        ProjectOutcome::Found { project } => project,
        other => panic!("expected Found, got {other:?}"),
    };
    assert!(
        platform.topics.is_empty(),
        "another project's keys are not its own"
    );

    let personal = match get_project(&ledger, "personal:human:alice")?.data {
        ProjectOutcome::Found { project } => project,
        other => panic!("expected Found, got {other:?}"),
    };
    assert!(
        personal.topics.is_empty(),
        "a personal project has no vocabulary"
    );

    let listed = list_projects(&ledger, &ProjectListRequest::default())?;
    let billing_listed = listed
        .data
        .items
        .iter()
        .find(|project| project.handle == "billing")
        .expect("billing listed");
    assert_eq!(billing_listed.topics.len(), 2, "the list carries them too");

    Ok(())
}

#[test]
fn the_misfiled_report_is_scoped_to_a_project_names_the_move_and_is_clean_after_it() -> Result<()> {
    use crate::queries::{
        require_registered_project, scan_misfiled_decisions, MisfiledScanRequest,
    };

    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alice", "beadline", None, None)?;
    commands.register_project("human:alice", "company", None, None)?;
    let recorder = DecisionRecorder::new(&commands)?;
    let stray = recorder.record("human:alice", "Filed nowhere in particular", None)?;
    let in_company = recorder.record("human:alice", "Filed under company", Some("company"))?;
    let in_beadline =
        recorder.record("human:alice", "Already where it belongs", Some("beadline"))?;

    let scan = |project: Option<&str>, move_to: Option<&str>, graph: &MemoryGraph| {
        scan_misfiled_decisions(
            graph,
            &MisfiledScanRequest {
                foreign_topic_keys: vec!["topic".to_owned()],
                project: project.map(ToOwned::to_owned),
                move_to: move_to.map(ToOwned::to_owned),
                limit: 100,
                cursor: None,
            },
        )
    };
    let ids = |rows: &[crate::queries::MisfiledDecisionCandidate]| {
        rows.iter()
            .map(|row| row.decision_id.clone())
            .collect::<Vec<_>>()
    };

    let graph = graph_of(&ledger)?;
    let everywhere = scan(None, None, &graph)?.data;
    assert_eq!(everywhere.len(), 3);
    let stray_row = everywhere
        .iter()
        .find(|row| row.decision_id == stray)
        .expect("stray flagged");
    assert_eq!(
        stray_row.project.as_deref(),
        Some("personal:human:alice"),
        "each row says where the decision is filed now"
    );
    assert_eq!(stray_row.proposed_move_to, None);

    let company_only = scan(Some("company"), None, &graph)?.data;
    assert_eq!(ids(&company_only), vec![in_company.clone()]);

    let to_beadline = scan(None, Some("beadline"), &graph)?.data;
    let mut expected = vec![stray.clone(), in_company.clone()];
    expected.sort();
    assert_eq!(
        ids(&to_beadline),
        expected,
        "a decision already in the destination is not flagged: that is not a move"
    );
    assert!(to_beadline
        .iter()
        .all(|row| row.proposed_move_to.as_deref() == Some("beadline")));
    assert!(!ids(&to_beadline).contains(&in_beadline));

    // The move is the existing verb, recorded and reversible; afterwards the personal
    // project's report is clean.
    commands.move_decision_to("human:alice", &stray, "beadline", Some("beadline work"))?;
    let graph = graph_of(&ledger)?;
    assert!(
        scan(Some("personal:human:alice"), Some("beadline"), &graph)?
            .data
            .is_empty()
    );
    assert_eq!(
        scan(Some("beadline"), None, &graph)?.data.len(),
        2,
        "the moved decision now reports under its new project"
    );

    // A project handle is checked, so a typo is a refusal and never an empty report.
    assert!(require_registered_project(&ledger, "beadline").is_ok());
    assert!(require_registered_project(&ledger, "personal:human:alice").is_ok());
    let refusal = require_registered_project(&ledger, "beadlin")
        .expect_err("an unregistered handle is refused")
        .to_string();
    assert!(
        refusal.contains("hivemind project register beadlin"),
        "{refusal}"
    );

    Ok(())
}
