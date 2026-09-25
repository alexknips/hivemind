//! "Where am I" becomes a project: the client-side half of working out which project a
//! capture belongs to (product spec section 4; epic hivemind-s15q child D1).
//!
//! HiveMind validates the address a capture carries and never infers it (three-layer
//! rule), so the inference lives here, in the client, next to the other places the CLI
//! turns its own surroundings into a question (`cwd` and `diff` become situational paths).
//! Neither the write layer nor the query layer sees this module. It runs only when the
//! caller opts in -- `--project-from-context` on the capture verbs and on the stdio
//! `hivemind mcp` server -- so a bare `hivemind emit` stays deterministic.
//!
//! The order, first match wins, each rung naming how it was determined:
//!
//! 1. the project the caller stated (`--project`)                     -> as stated
//! 2. the nearest `.hivemind-project` marker walking up from the cwd  -> `folder_marker`
//! 3. the registered project anchored to the rig in `GC_RIG`          -> `rig`
//! 4. the actor's current project (`hivemind project use`)            -> `current_project`
//! 5. none of the above: the personal project, with a reminder that
//!    the folder is not attached                                      -> `personal_fallback`
//!
//! Standing constraint (Alex, choice 2a, provisional): sub-projects must stay possible.
//! Nothing here forecloses them. A marker nested inside an attached folder names a
//! sub-project and the nearest marker wins; `part_of` chains are the registry's business,
//! not this module's. This module is also deliberately small and self-contained -- a
//! narrow interface (`resolve_project_from_context`) over an injectable set of sources --
//! so the rule can move (into a plugin, or server-side) without untangling it.

use std::path::{Path, PathBuf};

use crate::commands::DeterminedProject;
use crate::error::CliError;
use crate::events::{ProjectAnchorKind, ProjectSource, TenantId};
use crate::ledger::{EventLedger, TenantScopedLedger};
use crate::queries::{get_project_by_anchor, ProjectOutcome};
use crate::Result;

use super::current_project::CurrentProjectStore;

/// The one-line, checked-in file that attaches a folder (and everything beneath it, until a
/// nearer one) to a project: its whole content is the project handle.
pub(crate) const PROJECT_MARKER_FILE_NAME: &str = ".hivemind-project";

/// Shown next to a capture that found no project to attach to, so the personal fallback is
/// never silent and the way out is in the same message.
pub(crate) const UNATTACHED_FOLDER_REMINDER: &str =
    "this folder is not attached to a project yet; run hivemind project anchor ... to attach it";

/// The environment variable every Gas City session carries its rig in.
const RIG_ENV_VAR: &str = "GC_RIG";

/// What the ladder settled on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedProject {
    /// A rung matched. The handle is still unvalidated here: the write layer refuses one
    /// that is not registered, with the register command.
    Determined {
        handle: String,
        source: ProjectSource,
    },
    /// No rung matched. The write layer records the personal fallback itself.
    PersonalFallback,
}

impl From<DeterminedProject<'_>> for ResolvedProject {
    fn from(project: DeterminedProject<'_>) -> Self {
        Self::Determined {
            handle: project.handle.to_owned(),
            source: project.source,
        }
    }
}

impl ResolvedProject {
    /// The write layer's input, `None` when the caller has nothing to pass (fallback). A
    /// verb that inherits a project when none is stated (`supersede`) inherits on `None`.
    pub(crate) fn determined(&self) -> Option<DeterminedProject<'_>> {
        match self {
            Self::Determined { handle, source } => Some(DeterminedProject {
                handle,
                source: *source,
            }),
            Self::PersonalFallback => None,
        }
    }

    /// The "not attached" reminder, present exactly when nothing matched.
    pub(crate) fn reminder(&self) -> Option<&'static str> {
        matches!(self, Self::PersonalFallback).then_some(UNATTACHED_FOLDER_REMINDER)
    }
}

/// Everything the ladder reads from outside itself. Injectable so each rung can be tested
/// alone, and so a rung the ladder never reaches is provably never read (the registry in
/// particular: it is consulted once, and only when the rig rung is reached).
pub(crate) trait ProjectContextSources {
    /// Where the walk for a folder marker starts: an absolute directory.
    fn start_dir(&self) -> Result<PathBuf>;

    /// The rig this session runs in, if any.
    fn rig(&self) -> Option<String>;

    /// Handle of the registered project anchored to `rig`. The one registry read.
    fn project_anchored_to_rig(&self, rig: &str) -> Result<Option<String>>;

    /// The acting person's current-project setting.
    fn current_project(&self) -> Result<Option<String>>;
}

/// Work out the project for one capture. `stated` is what the caller passed outright; it
/// wins over everything and is not second-guessed.
pub(crate) fn resolve_project_from_context(
    stated: Option<DeterminedProject<'_>>,
    sources: &impl ProjectContextSources,
) -> Result<ResolvedProject> {
    if let Some(stated) = stated {
        return Ok(stated.into());
    }

    if let Some(handle) = nearest_marker_handle(&sources.start_dir()?)? {
        return Ok(ResolvedProject::Determined {
            handle,
            source: ProjectSource::FolderMarker,
        });
    }

    if let Some(rig) = sources.rig() {
        if let Some(handle) = sources.project_anchored_to_rig(&rig)? {
            return Ok(ResolvedProject::Determined {
                handle,
                source: ProjectSource::Rig,
            });
        }
    }

    if let Some(handle) = sources.current_project()? {
        return Ok(ResolvedProject::Determined {
            handle,
            source: ProjectSource::CurrentProject,
        });
    }

    Ok(ResolvedProject::PersonalFallback)
}

/// The handle in the nearest `.hivemind-project` at or above `start`. A marker nested inside
/// an attached folder is a sub-project and, being nearer, wins over the outer one.
///
/// A marker that cannot be read, or does not hold exactly one handle on one line, is refused
/// rather than skipped: silently walking past it would file the capture under some *other*
/// project than the folder says.
fn nearest_marker_handle(start: &Path) -> Result<Option<String>> {
    for dir in start.ancestors() {
        let marker = dir.join(PROJECT_MARKER_FILE_NAME); // ubs:ignore: constant file name, no untrusted segment
        if !marker.is_file() {
            continue;
        }
        let contents = std::fs::read_to_string(&marker).map_err(|error| {
            CliError::InvalidInput(format!("cannot read {}: {error}", marker.display()))
        })?;
        return parse_marker(&marker, &contents).map(Some);
    }
    Ok(None)
}

fn parse_marker(marker: &Path, contents: &str) -> Result<String> {
    let handle = contents.trim();
    if handle.is_empty() || handle.contains(char::is_whitespace) {
        return Err(CliError::InvalidInput(format!(
            "{} must contain exactly one project handle on a single line, for example `billing`",
            marker.display()
        ))
        .into());
    }
    Ok(handle.to_owned())
}

/// The process facts the ladder reads: where the session is, which rig it is in, and who is
/// acting. Held apart from the ledger so tests (and the stdio MCP server, which is started
/// once and then serves many calls) supply their own.
#[derive(Debug, Clone)]
pub(crate) struct ProjectContextEnv {
    /// `None` means the process working directory, read when the ladder needs it.
    start_dir: Option<PathBuf>,
    rig: Option<String>,
    /// Whose current-project setting the ladder consults: the person at the terminal, the
    /// same key `hivemind project use` writes under.
    person: String,
}

impl ProjectContextEnv {
    pub(crate) fn new(start_dir: Option<PathBuf>, rig: Option<&str>, person: &str) -> Self {
        Self {
            start_dir,
            rig: rig
                .map(str::trim)
                .filter(|rig| !rig.is_empty())
                .map(str::to_owned),
            person: person.trim().to_owned(),
        }
    }

    /// The real process: its working directory and its `GC_RIG`.
    pub(crate) fn from_process(person: &str) -> Self {
        Self::new(None, std::env::var(RIG_ENV_VAR).ok().as_deref(), person)
    }
}

/// [`ProjectContextSources`] over the invocation's own ledger and config directory.
struct LedgerProjectSources<'a, L: EventLedger> {
    env: &'a ProjectContextEnv,
    /// Already scoped to `tenant`.
    ledger: &'a L,
    tenant: &'a TenantId,
    hivemind_dir: &'a Path,
}

impl<L: EventLedger> ProjectContextSources for LedgerProjectSources<'_, L> {
    fn start_dir(&self) -> Result<PathBuf> {
        match &self.env.start_dir {
            Some(dir) => Ok(dir.clone()),
            None => std::env::current_dir().map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot work out the project from context: the working directory is unreadable: {error}"
                ))
                .into()
            }),
        }
    }

    fn rig(&self) -> Option<String> {
        self.env.rig.clone()
    }

    fn project_anchored_to_rig(&self, rig: &str) -> Result<Option<String>> {
        let response = get_project_by_anchor(self.ledger, ProjectAnchorKind::Rig, rig)?;
        Ok(match response.data {
            ProjectOutcome::Found { project } => Some(project.handle),
            ProjectOutcome::NotFound => None,
        })
    }

    fn current_project(&self) -> Result<Option<String>> {
        CurrentProjectStore::new(self.hivemind_dir).get(self.tenant.as_str(), &self.env.person)
    }
}

/// [`resolve_project_from_context`] against a tenant's ledger and the local config
/// directory (`--hivemind-dir`, where the current-project setting lives).
pub(crate) fn resolve_project_in_ledger<L: EventLedger + ?Sized>(
    stated: Option<DeterminedProject<'_>>,
    env: &ProjectContextEnv,
    ledger: &L,
    tenant: &TenantId,
    hivemind_dir: &Path,
) -> Result<ResolvedProject> {
    let scoped = TenantScopedLedger::new(ledger, tenant.clone());
    resolve_project_from_context(
        stated,
        &LedgerProjectSources {
            env,
            ledger: &scoped,
            tenant,
            hivemind_dir,
        },
    )
}

#[cfg(test)]
mod tests;
