//! Project registry reads: `list_projects` and `get_project`.
//!
//! Reads the ledger directly rather than the projected graph. `GraphView` has no
//! `remove_edge` (see `projector::project_event`'s `ProjectUnlinked` arm), so a
//! `project.unlinked` retraction is never reflected in `PART_OF`/`DEPENDS_ON` graph
//! edges -- trusting the graph here would silently show a retracted link as still
//! active (AGENTS.md §6, "no silent staleness"). Replaying the ledger nets adds
//! against removes the same way `commands::active_project_links` does for the write
//! layer, and gives every fact its own `event_origin` for free.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::commands::PERSONAL_PROJECT_HANDLE_PREFIX;
use crate::events::{
    EventType, ProjectAnchorPayload, ProjectLinkKind, ProjectLinkPayload, ProjectRegisteredPayload,
};
use crate::ledger::EventLedger;
use crate::Result;

use super::shared::{
    normalized_limit, normalized_query, parse_cursor, query_error, query_timer_start,
    DEFAULT_SEARCH_LIMIT,
};
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

#[derive(Clone, Debug, Default)]
struct ProjectRecord {
    display_name: Option<String>,
    purpose: Option<String>,
    registered_event_origin: i64,
    anchors: Vec<ProjectAnchorView>,
}

#[derive(Clone, Debug, Default)]
struct ProjectRegistry {
    projects: BTreeMap<String, ProjectRecord>,
    part_of: BTreeMap<String, ProjectLinkFact>,
    depends_on: BTreeMap<String, Vec<ProjectLinkFact>>,
}

/// Replays every `project.*` fact for the current tenant and nets adds against
/// removes, exactly like `commands::HiveMindCommands::active_project_links` does
/// for the write layer's own existence checks -- see the module doc comment for why
/// this can't be a graph read.
fn collect_project_registry(ledger: &impl EventLedger) -> Result<ProjectRegistry> {
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

#[cfg(test)]
mod tests;
