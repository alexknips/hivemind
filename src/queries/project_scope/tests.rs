// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use serde_json::Value;

use crate::commands::Commands;
use crate::events::ProjectLinkKind;
use crate::ledger::InMemoryEventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::queries::test_fixtures::{project_first_fixture, ProjectFirstFixture};
use crate::queries::{
    get_situational_decisions, DecisionStatus, DecisionView, QueryContext, QueryResponse,
    SituationalRequest, SituationalResults,
};
use crate::Result;

use super::*;

fn labels_of(fixture: &ProjectFirstFixture) -> Result<ProjectLabels> {
    let graph = MemoryGraph::default();
    rebuild_graph(&fixture.ledger, &graph)?;
    ProjectLabels::from_graph(&graph)
}

fn json_of<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| query_error(error).into())
}

fn refusal(ledger: &InMemoryEventLedger, project: &str) -> String {
    project_scope(ledger, project)
        .expect_err("the project is refused")
        .to_string()
}

// ---- the helper: which projects a question asked from one project draws on ----

#[test]
fn a_scope_is_the_project_its_parent_and_its_dependencies_and_says_what_it_did_not_follow(
) -> Result<()> {
    let f = project_first_fixture()?;
    let scope = project_scope(&f.ledger, "billing")?;

    assert_eq!(scope.own, "billing");
    assert_eq!(scope.parent.as_deref(), Some("platform"));
    assert_eq!(scope.dependencies, vec!["auth".to_owned()]);
    // City sits one level above the parent; Infra (Platform's dependency) and Crypto (Auth's)
    // are one link past the projects looked in.
    assert_eq!(scope.part_of_levels_not_followed, 1);
    assert_eq!(scope.linked_projects_not_followed, 2);
    Ok(())
}

#[test]
fn relation_of_places_a_decisions_project_in_the_scope_or_outside_it() -> Result<()> {
    let f = project_first_fixture()?;
    let scope = project_scope(&f.ledger, "billing")?;

    assert_eq!(scope.relation_of(Some("billing")), Some(ScopeRelation::Own));
    assert_eq!(
        scope.relation_of(Some("platform")),
        Some(ScopeRelation::Parent)
    );
    assert_eq!(
        scope.relation_of(Some("auth")),
        Some(ScopeRelation::Dependency)
    );
    for outside in [
        "city",
        "crypto",
        "infra",
        "marketing",
        "personal:human:alex",
    ] {
        assert_eq!(scope.relation_of(Some(outside)), None, "{outside}");
    }
    // A decision no request or proposal ever filed under a project belongs to no scope.
    assert_eq!(scope.relation_of(None), None);
    Ok(())
}

#[test]
fn the_note_names_the_projects_looked_in_and_where_the_walk_stopped() -> Result<()> {
    let f = project_first_fixture()?;
    let note = project_scope(&f.ledger, "billing")?.note(&labels_of(&f)?);

    assert_eq!(note.project, "billing");
    assert_eq!(note.project_label, "Billing");
    let followed: Vec<(&str, &str, ScopeRelation)> = note
        .followed
        .iter()
        .map(|p| (p.project.as_str(), p.project_label.as_str(), p.relation))
        .collect();
    assert_eq!(
        followed,
        vec![
            ("billing", "Billing", ScopeRelation::Own),
            ("platform", "Platform", ScopeRelation::Parent),
            ("auth", "Auth", ScopeRelation::Dependency),
        ]
    );
    assert_eq!(note.part_of_levels_not_followed, 1);
    assert_eq!(note.linked_projects_not_followed, 2);
    assert_eq!(
        note.note,
        "Looked in Billing (own project), Platform (Billing is part of it), Auth (Billing depends on it); not followed: 1 more level up the part_of chain, 2 more linked projects."
    );
    Ok(())
}

#[test]
fn a_project_linked_to_nothing_says_so() -> Result<()> {
    let f = project_first_fixture()?;
    let note = project_scope(&f.ledger, "marketing")?.note(&labels_of(&f)?);

    assert_eq!(note.followed.len(), 1);
    assert_eq!(
        note.note,
        "Looked in Marketing (own project); nothing further is linked."
    );
    Ok(())
}

#[test]
fn a_personal_address_resolves_with_no_links_and_groups_every_session_of_an_agent_tool(
) -> Result<()> {
    let f = project_first_fixture()?;
    let scope = project_scope(&f.ledger, "personal:agent:claude:session-9")?;

    assert_eq!(scope.own, "personal:agent:claude");
    assert_eq!(scope.parent, None);
    assert!(scope.dependencies.is_empty());
    assert_eq!(
        scope.relation_of(Some("personal:agent:claude")),
        Some(ScopeRelation::Own)
    );
    assert_eq!(
        scope.note(&labels_of(&f)?).note,
        "Looked in claude agents' personal project (own project); nothing further is linked."
    );
    Ok(())
}

#[test]
fn a_retracted_link_is_not_followed() -> Result<()> {
    let f = project_first_fixture()?;
    Commands::new(&f.ledger).unlink_project(
        "human:alex",
        "billing",
        "auth",
        ProjectLinkKind::DependsOn,
    )?;

    let scope = project_scope(&f.ledger, "billing")?;
    assert!(scope.dependencies.is_empty());
    assert_eq!(scope.relation_of(Some("auth")), None);
    // Crypto was only reachable through the retracted link; Infra still is, through Platform.
    assert_eq!(scope.linked_projects_not_followed, 1);
    Ok(())
}

#[test]
fn a_project_that_is_both_parent_and_dependency_is_looked_in_once_as_the_parent() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alex", "app", None, None)?;
    commands.register_project("human:alex", "core", None, None)?;
    commands.link_project("human:alex", "app", "core", ProjectLinkKind::PartOf)?;
    commands.link_project("human:alex", "app", "core", ProjectLinkKind::DependsOn)?;

    let scope = project_scope(&ledger, "app")?;
    assert_eq!(scope.parent.as_deref(), Some("core"));
    assert!(scope.dependencies.is_empty());
    assert_eq!(scope.relation_of(Some("core")), Some(ScopeRelation::Parent));
    Ok(())
}

#[test]
fn a_part_of_cycle_ends_the_walk_instead_of_looping() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    for handle in ["ring-a", "ring-b", "ring-c"] {
        commands.register_project("human:alex", handle, None, None)?;
    }
    commands.link_project("human:alex", "ring-a", "ring-b", ProjectLinkKind::PartOf)?;
    commands.link_project("human:alex", "ring-b", "ring-c", ProjectLinkKind::PartOf)?;
    commands.link_project("human:alex", "ring-c", "ring-a", ProjectLinkKind::PartOf)?;

    let scope = project_scope(&ledger, "ring-a")?;
    assert_eq!(scope.parent.as_deref(), Some("ring-b"));
    // Ring-c is above the parent; ring-a, reached again, is the project itself.
    assert_eq!(scope.part_of_levels_not_followed, 1);
    assert_eq!(scope.linked_projects_not_followed, 0);
    Ok(())
}

#[test]
fn an_unregistered_project_is_refused_with_the_register_hint_never_scoped_to_nothing() -> Result<()>
{
    let f = project_first_fixture()?;

    let message = refusal(&f.ledger, "billng");
    assert!(
        message.contains("project not registered: billng"),
        "{message}"
    );
    assert!(
        message.contains("`hivemind project register billng`"),
        "{message}"
    );
    assert!(refusal(&f.ledger, "  ").contains("project must not be empty"));
    assert!(refusal(&f.ledger, "personal:").contains("must name an actor"));
    Ok(())
}

// ---- the situational answer built on it ----

fn scoped(f: &ProjectFirstFixture, project: &str, limit: usize) -> Result<SituationalResults> {
    let graph = MemoryGraph::default();
    rebuild_graph(&f.ledger, &graph)?;
    Ok(situational(&graph, f, Some(project), limit, None)?.data)
}

fn situational(
    graph: &MemoryGraph,
    f: &ProjectFirstFixture,
    project: Option<&str>,
    limit: usize,
    cursor: Option<&str>,
) -> Result<QueryResponse<SituationalResults>> {
    get_situational_decisions(
        &QueryContext::local(),
        graph,
        &f.ledger,
        &SituationalRequest {
            paths: vec!["pricing".to_owned()],
            limit,
            cursor: cursor.map(str::to_owned),
            project: project.map(str::to_owned),
            ..SituationalRequest::default()
        },
    )
}

fn ids(results: &SituationalResults) -> Vec<&str> {
    results
        .matches
        .iter()
        .map(|m| m.decision.id.as_str())
        .collect()
}

#[test]
fn a_question_from_billing_lists_billing_then_its_parent_then_its_dependency() -> Result<()> {
    let f = project_first_fixture()?;
    let results = scoped(&f, "billing", 50)?;

    // Newest first within a group: the superseding Auth decision was proposed after the old one.
    assert_eq!(
        ids(&results),
        vec![
            f.billing.as_str(),
            f.platform.as_str(),
            f.auth_new.as_str(),
            f.auth_old.as_str(),
        ]
    );
    let relations: Vec<ScopeRelation> = results
        .matches
        .iter()
        .filter_map(|m| m.scope.as_ref().map(|scope| scope.relation))
        .collect();
    assert_eq!(
        relations,
        vec![
            ScopeRelation::Own,
            ScopeRelation::Parent,
            ScopeRelation::Dependency,
            ScopeRelation::Dependency,
        ]
    );
    assert_eq!(results.total_matches, 4);

    // Left out, and not silently: City is a level too high, Crypto and Infra a link too far,
    // Marketing is unrelated, and the personal decision has no project in this scope.
    for outside in [&f.city, &f.crypto, &f.infra, &f.marketing, &f.personal] {
        assert!(!ids(&results).contains(&outside.as_str()), "{outside}");
    }
    Ok(())
}

#[test]
fn each_decision_says_how_it_reached_the_answer() -> Result<()> {
    let f = project_first_fixture()?;
    let results = scoped(&f, "billing", 50)?;

    let labels: Vec<&str> = results
        .matches
        .iter()
        .filter_map(|m| m.scope.as_ref().map(|scope| scope.label.as_str()))
        .collect();
    assert_eq!(
        labels,
        vec![
            "own project",
            "from Platform; Billing is part of it",
            "from Auth; Billing depends on it",
            "from Auth; Billing depends on it",
        ]
    );
    let project_labels: Vec<&str> = results
        .matches
        .iter()
        .map(|m| m.decision.project_label.as_str())
        .collect();
    assert_eq!(project_labels, vec!["Billing", "Platform", "Auth", "Auth"]);
    Ok(())
}

#[test]
fn the_answer_says_where_it_stopped() -> Result<()> {
    let f = project_first_fixture()?;
    let results = scoped(&f, "billing", 50)?;

    let scope = results
        .scope
        .as_ref()
        .expect("a scoped answer has a scope note");
    assert_eq!(scope.project, "billing");
    assert_eq!(
        scope
            .followed
            .iter()
            .map(|p| p.project.as_str())
            .collect::<Vec<_>>(),
        vec!["billing", "platform", "auth"]
    );
    assert_eq!(scope.part_of_levels_not_followed, 1);
    assert_eq!(scope.linked_projects_not_followed, 2);
    assert!(scope
        .note
        .contains("not followed: 1 more level up the part_of chain, 2 more linked projects"));
    Ok(())
}

#[test]
fn staleness_shows_across_the_hop_with_the_same_labels() -> Result<()> {
    let f = project_first_fixture()?;
    let results = scoped(&f, "billing", 50)?;

    // Auth's decision was superseded; asked from Billing, it still surfaces, marked stale, and
    // says it came from Auth.
    let stale = results
        .matches
        .iter()
        .find(|m| m.decision.id == f.auth_old)
        .expect("the superseded Auth decision is in Billing's answer");
    assert_eq!(stale.decision.status, DecisionStatus::Superseded);
    assert!(stale.outcome.superseded);
    assert!(!stale.outcome.held_up);
    assert_eq!(
        stale.outcome.superseded_by.as_deref(),
        Some(f.auth_new.as_str())
    );
    assert_eq!(
        stale.scope.as_ref().map(|scope| scope.label.as_str()),
        Some("from Auth; Billing depends on it")
    );

    let json = json_of(stale)?;
    assert_eq!(json["scope"]["relation"], "dependency");
    let reasons = json["outcome"]["reasons"]
        .as_array()
        .expect("outcome reasons are a list");
    assert!(reasons
        .iter()
        .any(|reason| reason["kind"] == "superseded_by"));
    Ok(())
}

#[test]
fn an_unscoped_question_is_unchanged_and_carries_no_scope() -> Result<()> {
    let f = project_first_fixture()?;
    let graph = MemoryGraph::default();
    rebuild_graph(&f.ledger, &graph)?;
    let results = situational(&graph, &f, None, 50, None)?.data;

    // Every decision in the tenant: nothing was left out for want of a project.
    assert_eq!(results.total_matches, 9);
    assert!(results.scope.is_none());
    assert!(results.matches.iter().all(|m| m.scope.is_none()));
    let json = json_of(&results)?;
    assert!(json.get("scope").is_none());
    assert!(json["matches"][0].get("scope").is_none());
    Ok(())
}

#[test]
fn a_scoped_answer_pages_in_the_same_order_and_says_it_is_truncated() -> Result<()> {
    let f = project_first_fixture()?;
    let graph = MemoryGraph::default();
    rebuild_graph(&f.ledger, &graph)?;

    let first = situational(&graph, &f, Some("billing"), 3, None)?;
    assert!(first.truncated);
    assert_eq!(first.data.total_matches, 4);
    assert_eq!(
        ids(&first.data),
        vec![f.billing.as_str(), f.platform.as_str(), f.auth_new.as_str()]
    );
    let cursor = first.data.next_cursor.clone().expect("a next page");

    let second = situational(&graph, &f, Some("billing"), 3, Some(&cursor))?;
    assert!(!second.truncated);
    assert_eq!(ids(&second.data), vec![f.auth_old.as_str()]);
    // The note rides on every page, so a later page still says where the walk stopped.
    assert!(second.data.scope.is_some());
    Ok(())
}

#[test]
fn a_question_from_a_project_with_no_matching_decision_is_empty_and_still_names_its_scope(
) -> Result<()> {
    let f = project_first_fixture()?;
    let graph = MemoryGraph::default();
    rebuild_graph(&f.ledger, &graph)?;

    // Crypto's only decision is on `pricing`; ask about a term nothing in scope carries.
    let results = get_situational_decisions(
        &QueryContext::local(),
        &graph,
        &f.ledger,
        &SituationalRequest {
            paths: vec!["unrelatedterm".to_owned()],
            limit: 50,
            project: Some("billing".to_owned()),
            ..SituationalRequest::default()
        },
    )?
    .data;
    assert!(results.matches.is_empty());
    assert_eq!(
        results.scope.map(|scope| scope.followed.len()),
        Some(3),
        "an empty answer says which projects it looked in"
    );
    Ok(())
}

#[test]
fn a_personal_project_question_sees_only_that_persons_decisions() -> Result<()> {
    let f = project_first_fixture()?;
    let results = scoped(&f, "personal:human:alex", 50)?;

    assert_eq!(ids(&results), vec![f.personal.as_str()]);
    assert_eq!(
        results.matches[0].scope.as_ref().map(|s| s.relation),
        Some(ScopeRelation::Own)
    );
    Ok(())
}

#[test]
fn an_unknown_project_is_refused_not_answered_empty() -> Result<()> {
    let f = project_first_fixture()?;
    let graph = MemoryGraph::default();
    rebuild_graph(&f.ledger, &graph)?;

    let error = situational(&graph, &f, Some("billng"), 50, None)
        .expect_err("an unregistered project is refused");
    assert!(
        error.to_string().contains("project not registered: billng"),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_decision_moved_into_the_project_is_answered_from_its_new_project() -> Result<()> {
    let f = project_first_fixture()?;
    // Marketing's decision moves to Billing: the scope follows the decision's current project.
    Commands::new(&f.ledger).move_decision_to("human:alex", &f.marketing, "billing", None)?;
    let graph = MemoryGraph::default();
    rebuild_graph(&f.ledger, &graph)?;

    let results = situational(&graph, &f, Some("billing"), 50, None)?.data;
    let moved: &DecisionView = &results
        .matches
        .iter()
        .find(|m| m.decision.id == f.marketing)
        .expect("the moved decision is now Billing's own")
        .decision;
    assert_eq!(moved.project.as_deref(), Some("billing"));
    Ok(())
}
