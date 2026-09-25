//! Project reads: the registry (`list_projects`, `get_project`) and a project's
//! decisions (`decisions_in_project`).
//!
//! Reads the ledger directly rather than the projected graph. `GraphView` has no
//! `remove_edge` (see `projector::project_event`'s `ProjectUnlinked` arm), so a
//! `project.unlinked` retraction is never reflected in `PART_OF`/`DEPENDS_ON` graph
//! edges -- trusting the graph here would silently show a retracted link as still
//! active (AGENTS.md §6, "no silent staleness"). Replaying the ledger nets adds
//! against removes the same way `commands::active_project_links` does for the write
//! layer, and gives every fact its own `event_origin` for free.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::commands::{
    agent_actor_session, personal_project_handle, PERSONAL_PROJECT_HANDLE_PREFIX,
};
use crate::events::{
    EventType, ProjectAnchorKind, ProjectAnchorPayload, ProjectLinkKind, ProjectLinkPayload,
    ProjectRegisteredPayload,
};
use crate::ledger::EventLedger;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{
    neighbor_pairs, node_row, normalized_limit, normalized_query, optional_int, optional_string,
    parse_cursor, query_error, query_timer_start, required_string, Direction, DEFAULT_SEARCH_LIMIT,
};
use super::status::{derive_decision_status, DecisionStatus};
use super::QueryResponse;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ProjectAnchorView {
    pub kind: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectLinkFact {
    pub to: String,
    pub event_origin: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectView {
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// True for a derived personal-project address (`personal:<actor>`): never
    /// registered, always resolves, no anchors or links (see `get_project`).
    pub personal: bool,
    pub anchors: Vec<ProjectAnchorView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_of: Option<ProjectLinkFact>,
    pub depends_on: Vec<ProjectLinkFact>,
    /// Ledger offset of the `project.registered` fact. `None` for a personal
    /// project, which is derived from the actor and never registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_event_origin: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ProjectOutcome {
    Found {
        project: ProjectView,
    },
    /// A miss is data, not an error (Alex's rule, 2026-09-20): an unknown handle on
    /// `show` is a successful envelope, never a hard error.
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectListRequest {
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for ProjectListRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_SEARCH_LIMIT,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectListResults {
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub items: Vec<ProjectView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectDecisionsRequest {
    /// A registered shared handle, or a personal address (`personal:<actor>`).
    pub handle: String,
    pub limit: usize,
    pub cursor: Option<String>,
}

/// One decision in a project's list: enough to recognise it and to say where it came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectDecisionItem {
    pub decision_id: String,
    pub title: String,
    pub status: DecisionStatus,
    /// Who proposed it, as recorded (`agent:claude:session-1`, `human:alice`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    /// The session part of an agent's actor id. A personal address groups every session of
    /// one agent tool (`personal:agent:claude`), so this is what tells the sessions apart.
    /// Absent for a human, who has no session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// How the decision's project was determined (`stated`, `personal_fallback`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_origin: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectDecisionsPage {
    /// The address that was listed. For a personal project this is the canonical address
    /// (`personal:agent:claude`), even when the request named an actor with a session.
    pub handle: String,
    pub personal: bool,
    /// Whose personal project this is (`agent:claude`, `human:alice`); absent for a shared
    /// project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    /// Every decision in the project, not just this page.
    pub total_matches: usize,
    pub items: Vec<ProjectDecisionItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ProjectDecisionsOutcome {
    Found(ProjectDecisionsPage),
    /// A miss is data, like `ProjectOutcome::NotFound`: an unregistered handle is never an
    /// empty list, so a typo cannot read as "nothing recorded here".
    NotFound {
        handle: String,
    },
}

#[derive(Clone, Debug, Default)]
struct ProjectRecord {
    display_name: Option<String>,
    purpose: Option<String>,
    registered_event_origin: i64,
    anchors: Vec<ProjectAnchorView>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ProjectRegistry {
    projects: BTreeMap<String, ProjectRecord>,
    part_of: BTreeMap<String, ProjectLinkFact>,
    depends_on: BTreeMap<String, Vec<ProjectLinkFact>>,
}

impl ProjectRegistry {
    pub(super) fn is_registered(&self, handle: &str) -> bool {
        self.projects.contains_key(handle)
    }

    /// The one project `handle` is currently part of, if any.
    pub(super) fn part_of_parent(&self, handle: &str) -> Option<&str> {
        self.part_of.get(handle).map(|fact| fact.to.as_str())
    }

    /// The projects `handle` currently depends on, sorted by handle.
    pub(super) fn depends_on(&self, handle: &str) -> impl Iterator<Item = &str> {
        self.depends_on
            .get(handle)
            .into_iter()
            .flatten()
            .map(|fact| fact.to.as_str())
    }
}

/// Replays every `project.*` fact for the current tenant and nets adds against
/// removes, exactly like `commands::HiveMindCommands::active_project_links` does
/// for the write layer's own existence checks -- see the module doc comment for why
/// this can't be a graph read.
pub(super) fn collect_project_registry(ledger: &impl EventLedger) -> Result<ProjectRegistry> {
    let mut registry = ProjectRegistry::default();

    ledger.replay_from(0, &mut |event| {
        let Some(event_id) = event.event_id else {
            return Ok(());
        };
        let event_origin = i64::try_from(event_id).unwrap_or(i64::MAX);

        match event.event_type {
            EventType::ProjectRegistered => {
                if let Ok(payload) =
                    serde_json::from_value::<ProjectRegisteredPayload>(event.payload.clone())
                {
                    let record = registry.projects.entry(payload.handle).or_default();
                    record.display_name = payload.display_name;
                    record.purpose = payload.purpose;
                    record.registered_event_origin = event_origin;
                }
            }
            EventType::ProjectLinked => {
                if let Ok(payload) =
                    serde_json::from_value::<ProjectLinkPayload>(event.payload.clone())
                {
                    apply_project_linked(&mut registry, payload, event_origin);
                }
            }
            EventType::ProjectUnlinked => {
                if let Ok(payload) =
                    serde_json::from_value::<ProjectLinkPayload>(event.payload.clone())
                {
                    apply_project_unlinked(&mut registry, payload);
                }
            }
            EventType::ProjectAnchored => {
                if let Ok(payload) =
                    serde_json::from_value::<ProjectAnchorPayload>(event.payload.clone())
                {
                    let entry = anchor_view(&payload);
                    let record = registry.projects.entry(payload.handle).or_default();
                    if !record.anchors.contains(&entry) {
                        record.anchors.push(entry);
                    }
                }
            }
            EventType::ProjectUnanchored => {
                if let Ok(payload) =
                    serde_json::from_value::<ProjectAnchorPayload>(event.payload.clone())
                {
                    let entry = anchor_view(&payload);
                    if let Some(record) = registry.projects.get_mut(&payload.handle) {
                        record.anchors.retain(|anchor| *anchor != entry);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    })?;

    for record in registry.projects.values_mut() {
        record.anchors.sort();
    }
    for facts in registry.depends_on.values_mut() {
        facts.sort_by(|left, right| left.to.cmp(&right.to));
    }

    Ok(registry)
}

fn anchor_view(payload: &ProjectAnchorPayload) -> ProjectAnchorView {
    ProjectAnchorView {
        kind: payload.anchor_kind.as_str().to_owned(),
        value: payload.value.clone(),
    }
}

fn apply_project_linked(
    registry: &mut ProjectRegistry,
    payload: ProjectLinkPayload,
    event_origin: i64,
) {
    match payload.kind {
        ProjectLinkKind::PartOf => {
            registry.part_of.insert(
                payload.from,
                ProjectLinkFact {
                    to: payload.to,
                    event_origin,
                },
            );
        }
        ProjectLinkKind::DependsOn => {
            let facts = registry.depends_on.entry(payload.from).or_default();
            facts.retain(|fact| fact.to != payload.to);
            facts.push(ProjectLinkFact {
                to: payload.to,
                event_origin,
            });
        }
    }
}

fn apply_project_unlinked(registry: &mut ProjectRegistry, payload: ProjectLinkPayload) {
    match payload.kind {
        ProjectLinkKind::PartOf => {
            if registry
                .part_of
                .get(&payload.from)
                .is_some_and(|fact| fact.to == payload.to)
            {
                registry.part_of.remove(&payload.from);
            }
        }
        ProjectLinkKind::DependsOn => {
            if let Some(facts) = registry.depends_on.get_mut(&payload.from) {
                facts.retain(|fact| fact.to != payload.to);
            }
        }
    }
}

fn project_view(handle: &str, record: &ProjectRecord, registry: &ProjectRegistry) -> ProjectView {
    ProjectView {
        handle: handle.to_owned(),
        display_name: record.display_name.clone(),
        purpose: record.purpose.clone(),
        personal: false,
        anchors: record.anchors.clone(),
        part_of: registry.part_of.get(handle).cloned(),
        depends_on: registry.depends_on.get(handle).cloned().unwrap_or_default(),
        registered_event_origin: Some(record.registered_event_origin),
    }
}

/// Derived personal-project view: never stored, never `NotFound` -- a personal
/// address comes with the identity, so it "cannot be a typo" (product spec §1).
/// The actor component is carried verbatim; deriving one canonically from a live
/// actor id (dropping the session part) is a write-path concern for later slices
/// (A3/A4), not this read.
fn personal_project_view(handle: &str) -> ProjectView {
    ProjectView {
        handle: handle.to_owned(),
        display_name: None,
        purpose: None,
        personal: true,
        anchors: Vec::new(),
        part_of: None,
        depends_on: Vec::new(),
        registered_event_origin: None,
    }
}

/// List registered (shared) projects, paged. Personal projects never appear here --
/// they aren't registered and there is no bounded way to enumerate "every actor who
/// ever acted"; resolve one specifically via `get_project`.
pub fn list_projects(
    ledger: &impl EventLedger,
    request: &ProjectListRequest,
) -> Result<QueryResponse<ProjectListResults>> {
    let started = query_timer_start();
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let registry = collect_project_registry(ledger)?;
    let items: Vec<ProjectView> = registry
        .projects
        .iter()
        .map(|(handle, record)| project_view(handle, record, &registry))
        .collect();

    let total_matches = items.len();
    let page: Vec<ProjectView> = items.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(page.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: page.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: ProjectListResults {
            limit,
            cursor,
            next_cursor,
            total_matches,
            items: page,
        },
    })
}

/// Registered (shared) handles with their display names, for readers that need to name
/// projects without the full `ProjectView` (the decision log's per-project sections).
pub(super) fn registered_project_names(
    ledger: &impl EventLedger,
) -> Result<BTreeMap<String, Option<String>>> {
    Ok(collect_project_registry(ledger)?
        .projects
        .into_iter()
        .map(|(handle, record)| (handle, record.display_name))
        .collect())
}

/// The resolution rule `get_project` applies, for callers that already hold the registered
/// handles: a personal address resolves whenever it names an actor, any other handle must
/// be registered.
pub(super) fn is_known_project(
    registered: &BTreeMap<String, Option<String>>,
    handle: &str,
) -> bool {
    match handle.strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX) {
        Some(actor_part) => !actor_part.trim().is_empty(),
        None => registered.contains_key(handle),
    }
}

/// Resolve one project by handle. A `personal:<actor>` address always resolves to a
/// derived view (see `personal_project_view`); any other handle is looked up in the
/// registry and returns `ProjectOutcome::NotFound` -- not an error -- on a miss.
pub fn get_project(
    ledger: &impl EventLedger,
    handle: &str,
) -> Result<QueryResponse<ProjectOutcome>> {
    let started = query_timer_start();
    let handle = handle.trim();
    if handle.is_empty() {
        return Err(query_error("handle must not be empty").into());
    }

    if let Some(actor_part) = handle.strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX) {
        if actor_part.trim().is_empty() {
            return Err(query_error("personal project address must name an actor").into());
        }
        return Ok(QueryResponse {
            result_count: 1,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: ProjectOutcome::Found {
                project: personal_project_view(handle),
            },
        });
    }

    let registry = collect_project_registry(ledger)?;
    let outcome = match registry.projects.get(handle) {
        Some(record) => ProjectOutcome::Found {
            project: project_view(handle, record, &registry),
        },
        None => ProjectOutcome::NotFound,
    };

    Ok(QueryResponse {
        result_count: usize::from(matches!(outcome, ProjectOutcome::Found { .. })),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: outcome,
    })
}

/// The decisions in one project, oldest first, paged. Read from the projected graph, where
/// every decision node carries the project it belongs to (a decision recorded before
/// projects existed projects to its recorder's personal project), so a later move needs no
/// change here.
///
/// A personal address always resolves -- it comes with the identity and cannot be a typo --
/// and is canonicalised the way the write layer derives it, so every session of one agent
/// tool lists together and the session stays on each item. An unregistered shared handle is
/// `NotFound`, never an empty list.
///
/// Oldest first keeps a page boundary stable while new decisions are recorded; `total_matches`
/// counts the whole project, so a header can say how many are waiting.
pub fn decisions_in_project(
    graph: &impl GraphView,
    request: &ProjectDecisionsRequest,
) -> Result<QueryResponse<ProjectDecisionsOutcome>> {
    let started = query_timer_start();
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let requested = request.handle.trim();
    if requested.is_empty() {
        return Err(query_error("handle must not be empty").into());
    }

    let (handle, owner) =
        if let Some(actor_part) = requested.strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX) {
            let actor_part = actor_part.trim();
            if actor_part.is_empty() {
                return Err(query_error("personal project address must name an actor").into());
            }
            let handle = personal_project_handle(actor_part);
            let owner = handle
                .strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX)
                .unwrap_or(&handle)
                .to_owned();
            (handle, Some(owner))
        } else {
            if node_row(graph, NodeKind::Project, requested)?.is_none() {
                return Ok(QueryResponse {
                    result_count: 0,
                    truncated: false,
                    latency_ms: started.elapsed().as_millis(),
                    data: ProjectDecisionsOutcome::NotFound {
                        handle: requested.to_owned(),
                    },
                });
            }
            (requested.to_owned(), None)
        };

    let mut rows = project_decision_rows(graph, &handle)?;
    rows.sort_by(|left, right| {
        decision_origin(left)
            .cmp(&decision_origin(right))
            .then_with(|| row_id(left).cmp(row_id(right)))
    });

    let total_matches = rows.len();
    let mut items = Vec::new();
    for row in rows.into_iter().skip(offset).take(limit) {
        items.push(project_decision_item(graph, &row)?);
    }
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: ProjectDecisionsOutcome::Found(ProjectDecisionsPage {
            personal: owner.is_some(),
            handle,
            owner,
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        }),
    })
}

/// Every decision node whose project is `handle`. Filtered here rather than in the query:
/// the in-memory and Postgres graphs answer this shape by returning every decision, and a
/// decision node without a project (a stub named only by a request or blocker) belongs to
/// none.
fn project_decision_rows(graph: &impl GraphView, handle: &str) -> Result<Vec<GraphRow>> {
    let rows = graph.query(
        "MATCH (node:`Decision`) RETURN node.id AS id, node.title AS title, node.project AS project, node.project_source AS project_source, node.occurred_at AS occurred_at, node.event_origin AS event_origin ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    Ok(rows
        .into_iter()
        .filter(|row| optional_string(row, "project").as_deref() == Some(handle))
        .collect())
}

fn row_id(row: &GraphRow) -> &str {
    match row.get("id") {
        Some(GraphValue::String(id)) => id,
        _ => "",
    }
}

fn decision_origin(row: &GraphRow) -> i64 {
    optional_int(row, "event_origin").unwrap_or(i64::MAX)
}

fn project_decision_item(graph: &impl GraphView, row: &GraphRow) -> Result<ProjectDecisionItem> {
    let decision_id = required_string(row, "id")?;
    let status = derive_decision_status(graph, &decision_id)?;
    let proposed_by = neighbor_pairs(
        graph,
        NodeKind::Decision,
        &decision_id,
        RelationKind::ProposedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    )?
    .into_iter()
    .next()
    .map(|(actor_id, _origin)| actor_id);
    let session = proposed_by
        .as_deref()
        .and_then(agent_actor_session)
        .map(str::to_owned);

    Ok(ProjectDecisionItem {
        decision_id,
        title: optional_string(row, "title").unwrap_or_default(),
        status,
        proposed_by,
        session,
        project_source: optional_string(row, "project_source"),
        occurred_at: optional_string(row, "occurred_at"),
        event_origin: optional_int(row, "event_origin"),
    })
}

/// The registered project carrying an active anchor of `kind` with exactly `value`, one
/// registry replay. `NotFound` when none does -- a miss is data, as on `get_project`.
///
/// Only rig anchors are unique per tenant (`Commands::anchor_project` refuses a second
/// claimant), so for another kind two projects may carry the same value; the first handle in
/// sorted order answers, deterministically. Callers that need every claimant use
/// `list_projects`.
pub fn get_project_by_anchor(
    ledger: &impl EventLedger,
    kind: ProjectAnchorKind,
    value: &str,
) -> Result<QueryResponse<ProjectOutcome>> {
    let started = query_timer_start();
    let value = value.trim();
    if value.is_empty() {
        return Err(query_error("anchor value must not be empty").into());
    }

    let registry = collect_project_registry(ledger)?;
    let outcome = registry
        .projects
        .iter()
        .find(|(_, record)| {
            record
                .anchors
                .iter()
                .any(|anchor| anchor.kind == kind.as_str() && anchor.value == value)
        })
        .map_or(ProjectOutcome::NotFound, |(handle, record)| {
            ProjectOutcome::Found {
                project: project_view(handle, record, &registry),
            }
        });

    Ok(QueryResponse {
        result_count: usize::from(matches!(outcome, ProjectOutcome::Found { .. })),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: outcome,
    })
}

/// One project's place in the `part_of` tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectAncestry {
    pub handle: String,
    /// The projects above `handle`, nearest parent first. Empty for a top-level project and for
    /// a handle that is not registered (a miss is data, as on `get_project`). The walk stops
    /// before it would revisit a project: the write layer allows one parent per project but
    /// does not forbid a `part_of` cycle.
    pub ancestors: Vec<String>,
}

/// The `part_of` ancestry of each of `handles`, one registry replay however many are asked
/// about, in the order asked.
///
/// A read of the registry's shape and nothing more: which of these a change belongs to is the
/// caller's rule (the client works out "the nearest project they are all part of").
pub fn get_project_ancestries(
    ledger: &impl EventLedger,
    handles: &[String],
) -> Result<QueryResponse<Vec<ProjectAncestry>>> {
    let started = query_timer_start();
    let handles: Vec<&str> = handles.iter().map(|handle| handle.trim()).collect();
    if handles.iter().any(|handle| handle.is_empty()) {
        return Err(query_error("handle must not be empty").into());
    }

    let registry = collect_project_registry(ledger)?;
    let items: Vec<ProjectAncestry> = handles
        .into_iter()
        .map(|handle| ProjectAncestry {
            handle: handle.to_owned(),
            ancestors: part_of_ancestors(&registry, handle),
        })
        .collect();

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: items,
    })
}

fn part_of_ancestors(registry: &ProjectRegistry, handle: &str) -> Vec<String> {
    let mut walked: BTreeSet<&str> = BTreeSet::from([handle]);
    let mut ancestors: Vec<&str> = Vec::new();
    let mut current = handle;
    while let Some(parent) = registry.part_of_parent(current) {
        if !walked.insert(parent) {
            break;
        }
        ancestors.push(parent);
        current = parent;
    }
    ancestors.into_iter().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests;
