// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::commands::{
    Commands, DecisionProposalInput, DeterminedProject, Grounding, SupersedeInput,
};
use crate::events::{Event, EventSource, EventType};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::queries::project_label::ProjectLabels;
use crate::Result;

use super::*;

// Every answer names its project (hivemind-s15q.5): the decision's address (`project`) and
// what a person calls it (`project_label`) on every decision the listed queries return.

struct Scenario {
    ledger: InMemoryEventLedger,
    graph: MemoryGraph,
    /// Filed under the registered, display-named project `billing`.
    billing: String,
    /// Filed under the registered project `auth`, which has no display name.
    auth: String,
    /// No project stated, recorded by a human.
    personal_human: String,
    /// No project stated, recorded by an agent session.
    personal_agent: String,
    /// Supersedes `billing` with an explicit override to `auth`.
    moved_to_auth: String,
}

fn propose(
    commands: &Commands<'_, InMemoryEventLedger>,
    actor: &str,
    title: &str,
    topic: &str,
    project: Option<&str>,
) -> Result<String> {
    let option_id = commands.record_option(actor, "Per seat", "Charge each seat")?;
    let decision_id = commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: actor,
        title,
        rationale: "Naming the project keeps every answer honest about where it was decided",
        topic_keys: &[topic.to_owned()],
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &["Per seat".to_owned()],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
        delegated_by: None,
        project: project.map(DeterminedProject::stated),
    })?;
    Ok(decision_id)
}

fn scenario() -> Result<Scenario> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alex", "billing", Some("Billing"), None)?;
    commands.register_project("human:alex", "auth", None, None)?;

    let billing = propose(
        &commands,
        "human:alex",
        "Price per seat",
        "pricing",
        Some("billing"),
    )?;
    let auth = propose(
        &commands,
        "human:alex",
        "Sign in with passkeys",
        "pricing",
        Some("auth"),
    )?;
    let personal_human = propose(
        &commands,
        "human:alex",
        "Keep a personal scratch decision",
        "pricing",
        None,
    )?;
    let personal_agent = propose(
        &commands,
        "agent:claude:session-7",
        "Agent picked a retry budget",
        "pricing",
        None,
    )?;
    let moved_to_auth = commands
        .supersede(SupersedeInput {
            actor_id: "human:alex",
            old_decision_id: &billing,
            new_title: "Price per seat, owned by auth",
            new_rationale: "Seat pricing now hangs off the auth project's account model",
            topic_keys: &["pricing".to_owned()],
            option_labels: &["Per seat".to_owned()],
            chosen_option_label: Some("Per seat"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project: Some(DeterminedProject::stated("auth")),
            grounding: None,
            expressed_confidence: None,
        })?
        .new_decision_id;

    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok(Scenario {
        ledger,
        graph,
        billing,
        auth,
        personal_human,
        personal_agent,
        moved_to_auth,
    })
}

fn assert_names(project: Option<&str>, label: &str, expected_project: &str, expected_label: &str) {
    assert_eq!(project, Some(expected_project));
    assert_eq!(label, expected_label);
}

fn json_of<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| query_error(error).into())
}

fn assert_json_names(value: &Value, project: &str, label: &str) {
    assert_eq!(
        value.get("project").and_then(Value::as_str),
        Some(project),
        "project on {value}"
    );
    assert_eq!(
        value.get("project_label").and_then(Value::as_str),
        Some(label),
        "project_label on {value}"
    );
}

#[test]
fn get_decision_names_registered_and_personal_projects() -> Result<()> {
    let s = scenario()?;

    let billing = get_decision(&s.graph, &s.billing)?
        .data
        .ok_or_else(|| query_error("billing decision found"))?;
    assert_names(
        billing.project.as_deref(),
        &billing.project_label,
        "billing",
        "Billing",
    );

    // Registered without a display name: the label is the handle itself.
    let auth = get_decision(&s.graph, &s.auth)?
        .data
        .ok_or_else(|| query_error("auth decision found"))?;
    assert_names(auth.project.as_deref(), &auth.project_label, "auth", "auth");

    // No project stated: the recorder's personal project, labelled with the person ...
    let human = get_decision(&s.graph, &s.personal_human)?
        .data
        .ok_or_else(|| query_error("personal decision found"))?;
    assert_names(
        human.project.as_deref(),
        &human.project_label,
        "personal:human:alex",
        "alex's personal project",
    );

    // ... or the agent tool, never the session.
    let agent = get_decision(&s.graph, &s.personal_agent)?
        .data
        .ok_or_else(|| query_error("agent decision found"))?;
    assert_names(
        agent.project.as_deref(),
        &agent.project_label,
        "personal:agent:claude",
        "claude agents' personal project",
    );

    let json = json_of(&billing)?;
    assert_json_names(&json, "billing", "Billing");
    Ok(())
}

#[test]
fn a_superseding_decision_inherits_or_overrides_the_project() -> Result<()> {
    let s = scenario()?;
    let moved = get_decision(&s.graph, &s.moved_to_auth)?
        .data
        .ok_or_else(|| query_error("superseding decision found"))?;
    assert_names(
        moved.project.as_deref(),
        &moved.project_label,
        "auth",
        "auth",
    );

    // The superseded decision keeps naming the project it was filed under.
    let old = get_decision(&s.graph, &s.billing)?
        .data
        .ok_or_else(|| query_error("superseded decision found"))?;
    assert_names(
        old.project.as_deref(),
        &old.project_label,
        "billing",
        "Billing",
    );
    Ok(())
}

#[test]
fn the_brief_and_outcome_name_the_project() -> Result<()> {
    let s = scenario()?;

    let brief = get_decision_brief(&s.graph, &s.billing)?
        .data
        .ok_or_else(|| query_error("brief found"))?;
    assert_names(
        brief.project.as_deref(),
        &brief.project_label,
        "billing",
        "Billing",
    );
    assert_json_names(&json_of(&brief)?, "billing", "Billing");

    let outcome = get_decision_outcome(&s.graph, &s.personal_human)?
        .data
        .ok_or_else(|| query_error("outcome found"))?;
    assert_names(
        outcome.project.as_deref(),
        &outcome.project_label,
        "personal:human:alex",
        "alex's personal project",
    );
    assert_json_names(
        &json_of(&outcome)?,
        "personal:human:alex",
        "alex's personal project",
    );

    // The bulk quality read names the project on every outcome too.
    let bulk = get_decision_quality_candidates(
        &s.graph,
        &DecisionQualityCandidatesRequest {
            limit: 50,
            ..DecisionQualityCandidatesRequest::default()
        },
    )?;
    assert_eq!(bulk.data.len(), 5);
    for outcome in &bulk.data {
        let expected = get_decision(&s.graph, &outcome.decision_id)?
            .data
            .ok_or_else(|| query_error("bulk decision found"))?;
        assert_eq!(outcome.project, expected.project);
        assert_eq!(outcome.project_label, expected.project_label);
    }
    Ok(())
}

#[test]
fn search_relevant_and_situational_name_the_project_on_every_decision() -> Result<()> {
    let s = scenario()?;

    let search = search_decisions(
        &s.graph,
        &SearchDecisionRequest {
            topic_keys: vec!["pricing".to_owned()],
            limit: 50,
            ..SearchDecisionRequest::default()
        },
    )?;
    assert_eq!(search.data.items.len(), 5);
    let search_json = json_of(&search.data)?;
    let items = search_json["items"]
        .as_array()
        .ok_or_else(|| query_error("search items array"))?;
    for item in items {
        let decision = &item["decision"];
        assert!(decision["project"].is_string(), "project on {decision}");
        assert!(
            decision["project_label"].is_string(),
            "project_label on {decision}"
        );
    }
    let billing_hit = search
        .data
        .items
        .iter()
        .find(|item| item.decision.id == s.billing)
        .ok_or_else(|| query_error("billing decision searchable"))?;
    assert_names(
        billing_hit.decision.project.as_deref(),
        &billing_hit.decision.project_label,
        "billing",
        "Billing",
    );

    let relevant = get_relevant_decisions(&s.graph, "pricing", None)?;
    assert_eq!(relevant.data.len(), 5);
    let agent = relevant
        .data
        .iter()
        .find(|view| view.id == s.personal_agent)
        .ok_or_else(|| query_error("agent decision relevant"))?;
    assert_names(
        agent.project.as_deref(),
        &agent.project_label,
        "personal:agent:claude",
        "claude agents' personal project",
    );

    let situational = get_situational_decisions(
        &QueryContext::local(),
        &s.graph,
        &s.ledger,
        &SituationalRequest {
            paths: vec!["pricing".to_owned()],
            limit: 50,
            ..SituationalRequest::default()
        },
    )?;
    assert_eq!(situational.data.matches.len(), 5);
    let situational_json = json_of(&situational.data)?;
    for matched in situational_json["matches"]
        .as_array()
        .ok_or_else(|| query_error("situational matches array"))?
    {
        assert!(matched["decision"]["project"].is_string());
        assert!(matched["decision"]["project_label"].is_string());
        assert_eq!(
            matched["decision"]["project"],
            matched["outcome"]["project"]
        );
        assert_eq!(
            matched["decision"]["project_label"],
            matched["outcome"]["project_label"]
        );
    }
    Ok(())
}

#[test]
fn the_compact_view_names_the_project_of_the_decision_it_shows() -> Result<()> {
    let s = scenario()?;

    // Asked about the superseded decision, the compact view shows the terminal one: auth.
    let view = get_compact_view(&s.graph, &s.billing)?
        .data
        .ok_or_else(|| query_error("compact view found"))?;
    assert_eq!(view.decision.id, s.moved_to_auth);
    assert_names(
        view.decision.project.as_deref(),
        &view.decision.project_label,
        "auth",
        "auth",
    );
    assert_json_names(&json_of(&view)?["decision"], "auth", "auth");
    Ok(())
}

#[test]
fn the_neighborhood_names_the_project_of_the_root_and_of_each_decision_node() -> Result<()> {
    let s = scenario()?;

    let neighborhood =
        get_decision_neighborhood(&s.graph, &s.billing, &NeighborhoodRequest::all())?.data;
    let root = neighborhood
        .root
        .brief
        .as_ref()
        .ok_or_else(|| query_error("root carries its brief"))?;
    assert_names(
        root.project.as_deref(),
        &root.project_label,
        "billing",
        "Billing",
    );

    // The superseding decision lives in another project and says so.
    let superseding = neighborhood
        .nodes
        .iter()
        .find(|node| node.id == s.moved_to_auth)
        .ok_or_else(|| query_error("superseding decision is a neighbor"))?;
    assert_eq!(superseding.project.as_deref(), Some("auth"));
    assert_eq!(superseding.project_label.as_deref(), Some("auth"));

    // Only decisions are filed under a project; every other node kind stays bare.
    for node in neighborhood
        .nodes
        .iter()
        .filter(|node| node.kind != NodeKind::Decision)
    {
        assert!(node.project.is_none() && node.project_label.is_none());
    }
    let json = json_of(&neighborhood)?;
    assert_json_names(&json["root"], "billing", "Billing");
    let bare = json["nodes"]
        .as_array()
        .ok_or_else(|| query_error("nodes array"))?
        .iter()
        .find(|node| node["kind"] != "decision")
        .ok_or_else(|| query_error("a non-decision node"))?;
    assert!(bare.get("project").is_none());

    // A root that is not present has no project to name.
    let missing =
        get_decision_neighborhood(&s.graph, "decision-missing", &NeighborhoodRequest::all())?.data;
    assert!(!missing.root.present);
    assert!(missing.root.brief.is_none());
    let json = json_of(&missing)?;
    assert!(json["root"].get("project").is_none());
    assert!(json["root"].get("project_label").is_none());
    Ok(())
}

#[test]
fn a_moved_decision_is_named_by_the_project_it_moved_to() -> Result<()> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.register_project("human:alex", "billing", Some("Billing"), None)?;
    commands.register_project("human:alex", "pricing", Some("Pricing"), None)?;
    let decision = propose(
        &commands,
        "human:alex",
        "Price per seat",
        "pricing",
        Some("billing"),
    )?;
    commands.move_decision("human:alex", &decision, "billing", "pricing", None)?;
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;

    // The graph read and the ledger read both name the destination, not the proposal's project.
    let view = get_decision(&graph, &decision)?
        .data
        .ok_or_else(|| query_error("moved decision found"))?;
    assert_names(
        view.project.as_deref(),
        &view.project_label,
        "pricing",
        "Pricing",
    );
    let recent = get_recent_decisions(
        &ledger,
        &RecentDecisionsRequest {
            since_timestamp: Utc
                .with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| query_error("since timestamp"))?,
            until_timestamp: None,
            filters: RecentDecisionFilterRequest::default(),
            limit: 50,
            cursor: None,
        },
    )?;
    let entry = recent
        .data
        .items
        .iter()
        .find(|item| item.decision_id == decision)
        .ok_or_else(|| query_error("moved decision listed as recent"))?;
    assert_names(
        Some(entry.project.as_str()),
        &entry.project_label,
        "pricing",
        "Pricing",
    );
    Ok(())
}

#[test]
fn recent_decisions_name_the_project_from_the_ledger() -> Result<()> {
    let s = scenario()?;

    let recent = get_recent_decisions(
        &s.ledger,
        &RecentDecisionsRequest {
            since_timestamp: Utc
                .with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| query_error("since timestamp"))?,
            until_timestamp: None,
            filters: RecentDecisionFilterRequest::default(),
            limit: 50,
            cursor: None,
        },
    )?;
    assert_eq!(recent.data.items.len(), 5);
    let by_id = |id: &str| {
        recent
            .data
            .items
            .iter()
            .find(|item| item.decision_id == id)
            .ok_or_else(|| query_error("decision listed as recent"))
    };
    let billing = by_id(&s.billing)?;
    assert_names(
        Some(billing.project.as_str()),
        &billing.project_label,
        "billing",
        "Billing",
    );
    let human = by_id(&s.personal_human)?;
    assert_names(
        Some(human.project.as_str()),
        &human.project_label,
        "personal:human:alex",
        "alex's personal project",
    );
    let agent = by_id(&s.personal_agent)?;
    assert_names(
        Some(agent.project.as_str()),
        &agent.project_label,
        "personal:agent:claude",
        "claude agents' personal project",
    );
    // A supersede that states a project keeps it on the proposal.
    let moved = by_id(&s.moved_to_auth)?;
    assert_names(
        Some(moved.project.as_str()),
        &moved.project_label,
        "auth",
        "auth",
    );

    // The ledger read and the graph read agree on every decision.
    for item in &recent.data.items {
        let view = get_decision(&s.graph, &item.decision_id)?
            .data
            .ok_or_else(|| query_error("recent decision found in graph"))?;
        assert_eq!(Some(item.project.as_str()), view.project.as_deref());
        assert_eq!(item.project_label, view.project_label);
    }
    assert_json_names(&json_of(billing)?, "billing", "Billing");
    Ok(())
}

#[test]
fn the_decision_log_names_the_project_in_front_matter_and_body() -> Result<()> {
    let s = scenario()?;

    let DecisionLogOutcome::Exported(export) =
        export_decision_log(&s.graph, &s.ledger, &DecisionLogRequest::default())?
    else {
        return Err(query_error("an unfiltered decision log export succeeds").into());
    };
    let (path, file) = export
        .files
        .iter()
        .find(|(path, content)| {
            path.contains("/decisions/") && content.contains("# Price per seat\n")
        })
        .ok_or_else(|| query_error("billing decision file exported"))?;
    // The file sits in the project directory its own front matter names.
    assert!(path.starts_with("projects/billing/decisions/"), "{path}");
    assert!(file.contains("\nproject: \"billing\"\n"), "{file}");
    assert!(file.contains("\nproject_label: \"Billing\"\n"), "{file}");
    assert!(file.contains("\nProject: Billing\n"), "{file}");

    let (agent_path, agent_file) = export
        .files
        .iter()
        .find(|(path, content)| {
            path.contains("/decisions/") && content.contains("# Agent picked a retry budget\n")
        })
        .ok_or_else(|| query_error("agent decision file exported"))?;
    assert!(
        agent_path.starts_with("projects/personal/agent-claude/decisions/"),
        "{agent_path}"
    );
    assert!(
        agent_file.contains("\nproject: \"personal:agent:claude\"\n"),
        "{agent_file}"
    );
    assert!(
        agent_file.contains("\nProject: claude agents' personal project\n"),
        "{agent_file}"
    );
    Ok(())
}

#[test]
fn labels_read_from_the_graph_and_from_the_ledger_agree() -> Result<()> {
    let s = scenario()?;
    let from_graph = ProjectLabels::from_graph(&s.graph)?;
    let from_events = ProjectLabels::from_events(&s.ledger.read(0, 1000)?);
    for address in [
        "billing",
        "auth",
        "unregistered",
        "personal:human:alex",
        "personal:agent:claude",
    ] {
        assert_eq!(
            from_graph.label(address),
            from_events.label(address),
            "{address}"
        );
    }
    assert_eq!(from_graph.label("billing"), "Billing");
    assert_eq!(from_graph.label("auth"), "auth");
    assert_eq!(from_graph.label("unregistered"), "unregistered");
    Ok(())
}

#[test]
fn a_decision_only_named_by_a_request_says_it_has_no_project() -> Result<()> {
    let s = scenario()?;
    // A decision request can name a decision id before any proposal records it: the graph then
    // holds a bare node for it, with no project. It must read as unassigned, not break every
    // listing and not be filed under somebody's personal project by guesswork.
    s.ledger.append(Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::from_u128(9_001),
        correlation_id: Some("project-naming-test".to_owned()),
        causation_event_id: None,
        event_type: EventType::DecisionRequested,
        actor_id: "human:alex".to_owned(),
        source: EventSource::Cli,
        source_ref: Some("project-naming-test".to_owned()),
        payload: json!({
            "topic_keys": ["pricing"],
            "decision_id": "decision-not-yet-proposed",
            "reason": "Need a call on seat pricing",
            "priority": "P1",
            "required_owner_id": null,
            "authority_class": "product",
            "requested_by": "human:alex",
            "client_request_id": "request-1"
        }),
        ts: Some(Utc::now()),
    })?;
    let graph = MemoryGraph::default();
    rebuild_graph(&s.ledger, &graph)?;

    let view = get_decision(&graph, "decision-not-yet-proposed")?
        .data
        .ok_or_else(|| query_error("bare decision node found"))?;
    assert_eq!(view.project, None);
    assert_eq!(view.project_label, "no project recorded");
    let json = json_of(&view)?;
    assert!(json.get("project").is_some_and(Value::is_null), "{json}");
    assert_eq!(json["project_label"], "no project recorded");

    let outcome = get_decision_outcome(&graph, "decision-not-yet-proposed")?
        .data
        .ok_or_else(|| query_error("bare decision outcome found"))?;
    assert_eq!(outcome.project, None);
    assert_eq!(outcome.project_label, "no project recorded");

    // Every listing still works, the bare node included, and the proposed decisions keep their names.
    let search = search_decisions(&graph, &SearchDecisionRequest::default())?;
    assert_eq!(search.data.total_matches, 6);
    let neighborhood = get_decision_neighborhood(
        &graph,
        "decision-not-yet-proposed",
        &NeighborhoodRequest::all(),
    )?
    .data;
    assert!(neighborhood.root.present);
    let root = neighborhood
        .root
        .brief
        .as_ref()
        .ok_or_else(|| query_error("bare root carries its brief"))?;
    assert_eq!(root.project, None);
    assert_eq!(root.project_label, "no project recorded");
    let billing = get_decision(&graph, &s.billing)?
        .data
        .ok_or_else(|| query_error("billing decision found"))?;
    assert_names(
        billing.project.as_deref(),
        &billing.project_label,
        "billing",
        "Billing",
    );
    Ok(())
}
