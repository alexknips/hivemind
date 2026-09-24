//! The words each answer prints beside a decision's project address.
//!
//! A decision's `project` is an address: a registered handle (`billing`) or a derived personal
//! address (`personal:human:alex`, see `commands::personal_project_handle`). Every answer also
//! carries `project_label`, the name a person would use for it: the display name the project was
//! registered with (else its handle), or for a personal address the person or agent tool it
//! belongs to. Labels are read from the registered `Project` facts, never inferred (AGENTS.md §3).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::commands::PERSONAL_PROJECT_HANDLE_PREFIX;
use crate::events::{Event, EventType, ProjectRegisteredPayload};
use crate::projector::{GraphView, NodeKind};
use crate::Result;

use super::shared::{node_rows, optional_string};

/// What the label says for a decision node that has no project: one that was referenced (by a
/// decision request, a blocker, a supersession) before any proposal recorded it. Its `project`
/// is `null`; the label never stays blank, so an unassigned decision is visible, not silent.
pub(crate) const NO_PROJECT_LABEL: &str = "no project recorded";

/// Display names of the registered projects, loaded once per query so labelling a page of
/// decisions costs one project read rather than one per row.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProjectLabels {
    display_names: BTreeMap<String, String>,
}

impl ProjectLabels {
    /// Read the display names off the projected `Project` nodes.
    pub(crate) fn from_graph(graph: &impl GraphView) -> Result<Self> {
        let mut display_names = BTreeMap::new();
        for (handle, row) in node_rows(graph, NodeKind::Project)? {
            if let Some(name) = optional_string(&row, "display_name") {
                insert_display_name(&mut display_names, handle, name);
            }
        }
        Ok(Self { display_names })
    }

    /// Read the display names off the `project.registered` facts, for the queries that read
    /// the ledger rather than the graph. A later registration of the same handle wins, as it
    /// does in the projected graph.
    pub(crate) fn from_events(events: &[Event]) -> Self {
        let mut display_names = BTreeMap::new();
        for event in events {
            if event.event_type != EventType::ProjectRegistered {
                continue;
            }
            let Ok(payload) = ProjectRegisteredPayload::deserialize(&event.payload) else {
                continue;
            };
            if let Some(name) = payload.display_name {
                insert_display_name(&mut display_names, payload.handle, name);
            }
        }
        Self { display_names }
    }

    /// `label` for a decision that may have no project at all.
    pub(crate) fn label_of(&self, address: Option<&str>) -> String {
        match address {
            Some(address) => self.label(address),
            None => NO_PROJECT_LABEL.to_owned(),
        }
    }

    /// The label for a project address: the person or agent tool for a personal address,
    /// else the registered display name, else the handle itself.
    pub(crate) fn label(&self, address: &str) -> String {
        if let Some(personal) = personal_project_label(address) {
            return personal;
        }
        match self.display_names.get(address) {
            Some(name) => name.to_owned(),
            None => address.to_owned(),
        }
    }
}

fn insert_display_name(names: &mut BTreeMap<String, String>, handle: String, name: String) {
    if name.trim().is_empty() {
        return;
    }
    names.insert(handle, name);
}

/// `Some(label)` when `address` is a derived personal address. The owner is named exactly as the
/// actor id names them: `human:alex` is "alex's personal project"; every session of one agent
/// tool shares one personal project, so `agent:claude` is "claude agents' personal project".
pub(crate) fn personal_project_label(address: &str) -> Option<String> {
    let owner = address.strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX)?;
    if let Some(tool) = owner.strip_prefix("agent:") {
        return Some(format!("{tool} agents' personal project"));
    }
    let person = match owner.strip_prefix("human:") {
        Some(person) => person,
        None => owner,
    };
    Some(format!("{person}'s personal project"))
}

#[cfg(test)]
mod tests;
