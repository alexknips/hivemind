//! The actor's "current project": a one-time, per-machine setting for
//! captures with no repo or folder context (product spec §2, scenario 4;
//! epic hivemind-s15q child D2). Deliberately NOT a ledger fact — Alex,
//! choice 5a (2026-09-21): "local per machine, no fact in the record".
//! Setting or clearing it never appends an event and carries no
//! `event_origin`; it lives in a local JSON file under `--hivemind-dir`,
//! next to `connector-tokens.json` (see `crate::connector::GoogleTokenStore`,
//! the precedent this mirrors for "local file, not the ledger").
//!
//! Keyed by tenant then actor: one `--hivemind-dir` can serve several
//! tenants (see `--tenant`), and a setting made under one tenant must never
//! leak into another.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::Result;

const CURRENT_PROJECT_FILE_NAME: &str = "current-project.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CurrentProjectFile {
    #[serde(default)]
    tenants: BTreeMap<String, BTreeMap<String, String>>,
}

pub(crate) struct CurrentProjectStore {
    path: PathBuf,
}

impl CurrentProjectStore {
    pub(crate) fn new(hivemind_dir: &Path) -> Self {
        Self {
            path: hivemind_dir.join("current-project.json"),
        }
    }

    fn load(&self) -> Result<CurrentProjectFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|error| {
                CliError::InvalidInput(format!("{CURRENT_PROJECT_FILE_NAME} is malformed: {error}"))
                    .into()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(CurrentProjectFile::default())
            }
            Err(error) => Err(CliError::InvalidInput(format!(
                "failed to read {CURRENT_PROJECT_FILE_NAME}: {error}"
            ))
            .into()),
        }
    }

    fn save(&self, file: &CurrentProjectFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CliError::InvalidInput(format!(
                    "failed to create {CURRENT_PROJECT_FILE_NAME} directory: {error}"
                ))
            })?;
        }
        let json = serde_json::to_string_pretty(file).map_err(|error| {
            CliError::InvalidInput(format!(
                "failed to serialize {CURRENT_PROJECT_FILE_NAME}: {error}"
            ))
        })?;
        std::fs::write(&self.path, json).map_err(|error| {
            CliError::InvalidInput(format!(
                "failed to write {CURRENT_PROJECT_FILE_NAME}: {error}"
            ))
            .into()
        })
    }

    pub(crate) fn get(&self, tenant: &str, actor: &str) -> Result<Option<String>> {
        let file = self.load()?;
        Ok(file
            .tenants
            .get(tenant)
            .and_then(|actors| actors.get(actor))
            .cloned())
    }

    pub(crate) fn set(&self, tenant: &str, actor: &str, handle: &str) -> Result<()> {
        let mut file = self.load()?;
        file.tenants
            .entry(tenant.to_owned())
            .or_default()
            .insert(actor.to_owned(), handle.to_owned());
        self.save(&file)
    }

    pub(crate) fn clear(&self, tenant: &str, actor: &str) -> Result<()> {
        let mut file = self.load()?;
        if let Some(actors) = file.tenants.get_mut(tenant) {
            actors.remove(actor);
            if actors.is_empty() {
                file.tenants.remove(tenant);
            }
        }
        self.save(&file)
    }
}

#[cfg(test)]
mod tests;
