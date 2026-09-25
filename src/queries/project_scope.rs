//! Project-first scoping: which projects a question asked from one project may draw on.
//!
//! A question asked from project P looks in P itself, then in the project P is part of (the
//! parent's decisions are inherited constraints), then in the projects P depends on. Each link is
//! followed exactly one hop: the parent's own parent and the dependencies' own links are left
//! alone, and [`ScopeNote`] says how many of each were left, so "go deeper" stays an explicit
//! later step and a short answer never reads as a complete one (AGENTS.md §6). Children are not
//! visited: a parent cites a child decision as evidence, it does not see the child by default.
//!
//! The links are read from the ledger, not from the projected `PART_OF` / `DEPENDS_ON` edges:
//! `GraphView` has no `remove_edge`, so a retracted link would still look active in the graph
//! (see `projects.rs`). Only the project structure is resolved here; the caller filters and
//! orders decisions by [`ProjectScope::relation_of`]. No LLM, no ranking, no inference.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::commands::{personal_project_handle, PERSONAL_PROJECT_HANDLE_PREFIX};
use crate::ledger::EventLedger;
use crate::Result;

use super::project_label::ProjectLabels;
use super::projects::{collect_project_registry, ProjectRegistry};
use super::shared::query_error;

/// How a decision's project relates to the project a question was asked from. Declared in
/// answer order: own project first, then the parent's, then a dependency's.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeRelation {
    /// The decision belongs to the project the question was asked from.
    Own,
    /// The asked project is part of the decision's project: an inherited constraint.
    Parent,
    /// The asked project depends on the decision's project.
    Dependency,
}

/// The project structure a question is answered from. Built once per question by
/// [`project_scope`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectScope {
    own: String,
    parent: Option<String>,
    dependencies: Vec<String>,
    part_of_levels_not_followed: usize,
    linked_projects_not_followed: usize,
}

/// One project a question looked in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScopedProject {
    pub project: String,
    pub project_label: String,
    pub relation: ScopeRelation,
}

/// Where a scoped answer looked and where it stopped.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScopeNote {
    /// The project the question was asked from (address) and what a person calls it.
    pub project: String,
    pub project_label: String,
    /// Every project looked in, in answer order, whether or not it held a matching decision.
    pub followed: Vec<ScopedProject>,
    /// Projects further up the `part_of` chain than the parent, which were not followed.
    pub part_of_levels_not_followed: usize,
    /// Projects one link beyond the ones looked in (the parent's and the dependencies' own
    /// dependencies and parents, other than the `part_of` chain counted above), not followed.
    pub linked_projects_not_followed: usize,
    /// The same facts as one sentence.
    pub note: String,
}

/// How one decision in a scoped answer relates to the asked project, in words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MatchScope {
    pub relation: ScopeRelation,
    /// `own project`, `from Platform; Billing is part of it`, or
    /// `from Auth; Billing depends on it`.
    pub label: String,
}

/// Resolve the scope of a question asked from `project`: a registered handle, or a personal
/// address (which always resolves and has no links). An unregistered handle is refused with the
/// register hint the write layer gives, never answered as an empty scope.
///
/// `ledger` must already be scoped to the tenant being queried.
pub(crate) fn project_scope(ledger: &impl EventLedger, project: &str) -> Result<ProjectScope> {
    let project = project.trim();
    if project.is_empty() {
        return Err(query_error("project must not be empty").into());
    }

    if let Some(actor_part) = project.strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX) {
        let actor_part = actor_part.trim();
        if actor_part.is_empty() {
            return Err(query_error("personal project address must name an actor").into());
        }
        return Ok(ProjectScope {
            own: personal_project_handle(actor_part),
            parent: None,
            dependencies: Vec::new(),
            part_of_levels_not_followed: 0,
            linked_projects_not_followed: 0,
        });
    }

    let registry = collect_project_registry(ledger)?;
    if !registry.is_registered(project) {
        return Err(query_error(format!(
            "project not registered: {project} -- register it first with `hivemind project register {project}`"
        ))
        .into());
    }
    Ok(resolve_registered(&registry, project))
}

fn resolve_registered(registry: &ProjectRegistry, own: &str) -> ProjectScope {
    let parent = registry.part_of_parent(own);
    // A project that is both the parent and a dependency is looked in once, as the parent.
    let dependencies: Vec<&str> = registry
        .depends_on(own)
        .filter(|dependency| Some(*dependency) != parent)
        .collect();

    let mut followed: BTreeSet<&str> = BTreeSet::from([own]);
    followed.extend(parent);
    followed.extend(dependencies.iter().copied());

    // Walk the part_of chain above the parent. `walked` makes a part_of cycle terminate: the
    // write layer allows one parent per project but does not forbid A part-of B part-of A.
    let mut ancestors: BTreeSet<&str> = BTreeSet::new();
    if let Some(parent) = parent {
        let mut walked: BTreeSet<&str> = BTreeSet::from([own, parent]);
        let mut current = parent;
        while let Some(next) = registry.part_of_parent(current) {
            if !walked.insert(next) {
                break;
            }
            if !followed.contains(next) {
                ancestors.insert(next);
            }
            current = next;
        }
    }

    // One link beyond the projects looked in. The asked project's own links are all followed,
    // so only the parent and the dependencies can reach anything new.
    let mut beyond: BTreeSet<&str> = BTreeSet::new();
    for project in followed.iter().copied().filter(|project| *project != own) {
        beyond.extend(registry.part_of_parent(project));
        beyond.extend(registry.depends_on(project));
    }
    let linked_projects_not_followed = beyond
        .iter()
        .filter(|project| !followed.contains(*project) && !ancestors.contains(*project))
        .count();

    ProjectScope {
        own: own.to_owned(),
        parent: parent.map(str::to_owned),
        dependencies: dependencies.into_iter().map(str::to_owned).collect(),
        part_of_levels_not_followed: ancestors.len(),
        linked_projects_not_followed,
    }
}

impl ProjectScope {
    /// How a decision filed under `project` relates to the asked project; `None` when it is
    /// outside the scope (or has no project at all).
    pub(crate) fn relation_of(&self, project: Option<&str>) -> Option<ScopeRelation> {
        let project = project?;
        if project == self.own {
            Some(ScopeRelation::Own)
        } else if self.parent.as_deref() == Some(project) {
            Some(ScopeRelation::Parent)
        } else if self
            .dependencies
            .iter()
            .any(|dependency| dependency == project)
        {
            Some(ScopeRelation::Dependency)
        } else {
            None
        }
    }

    /// The words a decision's row carries for how it reached the answer.
    pub(crate) fn match_scope(
        &self,
        relation: ScopeRelation,
        decision_project_label: &str,
        labels: &ProjectLabels,
    ) -> MatchScope {
        let label = match relation {
            ScopeRelation::Own => "own project".to_owned(),
            ScopeRelation::Parent => format!(
                "from {decision_project_label}; {} is part of it",
                labels.label(&self.own)
            ),
            ScopeRelation::Dependency => format!(
                "from {decision_project_label}; {} depends on it",
                labels.label(&self.own)
            ),
        };
        MatchScope { relation, label }
    }

    /// Where the answer looked and where it stopped.
    pub(crate) fn note(&self, labels: &ProjectLabels) -> ScopeNote {
        let own_label = labels.label(&self.own);
        let mut followed = vec![ScopedProject {
            project: self.own.clone(),
            project_label: own_label.clone(),
            relation: ScopeRelation::Own,
        }];
        if let Some(parent) = &self.parent {
            followed.push(scoped_project(parent, ScopeRelation::Parent, labels));
        }
        for dependency in &self.dependencies {
            followed.push(scoped_project(
                dependency,
                ScopeRelation::Dependency,
                labels,
            ));
        }

        let looked_in = followed
            .iter()
            .map(|project| match project.relation {
                ScopeRelation::Own => format!("{} (own project)", project.project_label),
                ScopeRelation::Parent => {
                    format!("{} ({own_label} is part of it)", project.project_label)
                }
                ScopeRelation::Dependency => {
                    format!("{} ({own_label} depends on it)", project.project_label)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let stopped =
            if self.part_of_levels_not_followed == 0 && self.linked_projects_not_followed == 0 {
                "nothing further is linked".to_owned()
            } else {
                format!(
                    "not followed: {}, {}",
                    counted(
                        self.part_of_levels_not_followed,
                        "more level up the part_of chain",
                        "more levels up the part_of chain",
                    ),
                    counted(
                        self.linked_projects_not_followed,
                        "more linked project",
                        "more linked projects",
                    ),
                )
            };

        ScopeNote {
            project: self.own.clone(),
            project_label: own_label,
            followed,
            part_of_levels_not_followed: self.part_of_levels_not_followed,
            linked_projects_not_followed: self.linked_projects_not_followed,
            note: format!("Looked in {looked_in}; {stopped}."),
        }
    }
}

fn scoped_project(project: &str, relation: ScopeRelation, labels: &ProjectLabels) -> ScopedProject {
    ScopedProject {
        project: project.to_owned(),
        project_label: labels.label(project),
        relation,
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

#[cfg(test)]
mod tests;
