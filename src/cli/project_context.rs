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
//! 2. the `.hivemind-project` markers of the folders the change
//!    touches (see below), else the nearest one walking up from the
//!    cwd                                                             -> `folder_marker`
//! 3. the registered project anchored to the rig in `GC_RIG`          -> `rig`
//! 4. the actor's current project (`hivemind project use`)            -> `current_project`
//! 5. none of the above: the personal project, with a reminder that
//!    the folder is not attached                                      -> `personal_fallback`
//!
//! A change that spans folders (hivemind-s15q.14). The files the change touches -- the same
//! working-tree diff plus staged set the situational query reads -- each name the marker
//! nearest above them; the distinct handles are the projects the change spans. One handle is
//! the project. Several are recorded for the nearest project they are all `part_of` (a
//! decision is made for the parent project, never for two projects), `folder_marker`, and the
//! reply says so. With no common parent the decision is saved to the actor's personal project
//! (`personal_fallback`) and the reply names the projects and how to move it: losing the
//! decision is worse than misfiling it, since moves are allowed. There is no refused or
//! ambiguous outcome, and the write path is only ever handed one project. A change whose
//! files sit under no marker falls back to the folder the session stands in.
//!
//! Standing constraint (Alex, choice 2a, provisional): sub-projects must stay possible.
//! Nothing here forecloses them. A marker nested inside an attached folder names a
//! sub-project and the nearest marker wins; `part_of` chains are the registry's business,
//! not this module's. This module is also deliberately small and self-contained -- a
//! narrow interface (`resolve_project_from_context`) over an injectable set of sources --
//! so the rule can move (into a plugin, or server-side) without untangling it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::commands::DeterminedProject;
use crate::error::CliError;
use crate::events::{ProjectAnchorKind, ProjectSource, TenantId};
use crate::ledger::{EventLedger, TenantScopedLedger};
use crate::queries::{
    get_project_ancestries, get_project_by_anchor, ProjectAncestry, ProjectOutcome,
};
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

/// Said next to a capture that fell back to the personal project because the change spans
/// projects with no parent in common. Follows the placement line, which already says
/// "saved to your personal project".
fn spans_unrelated_reminder(spanned: &[String]) -> String {
    format!(
        "this change spans {}, which share no parent. Move it with hivemind move ..., or register a parent.",
        join_handles(spanned)
    )
}

/// Said next to a capture recorded for the parent of the projects a change spans.
fn spans_under_parent_note(parent: &str, spanned: &[String]) -> String {
    format!(
        "recorded for {parent}: this change spans {}",
        join_handles(spanned)
    )
}

/// `a`, `a and b`, `a, b and c`.
fn join_handles(handles: &[String]) -> String {
    match handles {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// What the ladder settled on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedProject {
    /// A rung matched. The handle is still unvalidated here: the write layer refuses one
    /// that is not registered, with the register command.
    Determined {
        handle: String,
        source: ProjectSource,
    },
    /// The change touches folders attached to several projects that share a parent: it is
    /// recorded for `parent`, the nearest project they are all part of (`folder_marker`).
    /// `spanned` are the projects the change touches, sorted.
    SpansUnderParent {
        parent: String,
        spanned: Vec<String>,
    },
    /// No rung matched. The write layer records the personal fallback itself.
    PersonalFallback,
    /// The change touches folders attached to several projects that share no parent. The
    /// write layer records the personal fallback itself; the reply names `spanned` (sorted)
    /// so the decision can be moved or a parent registered.
    SpansUnrelated { spanned: Vec<String> },
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
            Self::SpansUnderParent { parent, .. } => Some(DeterminedProject {
                handle: parent,
                source: ProjectSource::FolderMarker,
            }),
            Self::PersonalFallback | Self::SpansUnrelated { .. } => None,
        }
    }

    /// What the reply tells the reader about how the project was worked out, if anything.
    /// `landed_in_personal` says whether the write layer really recorded the personal
    /// project: a fallback reminder is only true then (a superseding decision that inherited a
    /// shared project has nothing to be reminded about).
    pub(crate) fn reminder(&self, landed_in_personal: bool) -> Option<String> {
        match self {
            Self::Determined { .. } => None,
            Self::SpansUnderParent { parent, spanned } => {
                Some(spans_under_parent_note(parent, spanned))
            }
            Self::PersonalFallback => {
                landed_in_personal.then(|| UNATTACHED_FOLDER_REMINDER.to_owned())
            }
            Self::SpansUnrelated { spanned } => {
                landed_in_personal.then(|| spans_unrelated_reminder(spanned))
            }
        }
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

    /// The files the change under way in `dir` touches, as absolute paths: the working-tree
    /// diff plus the staged set, the same source the situational query reads. Empty when
    /// there is no change or git cannot say (not a repository, no `git` on PATH).
    fn touched_paths(&self, dir: &Path) -> Vec<PathBuf>;

    /// The `part_of` ancestry of each handle, from one registry read. Only asked when a
    /// change spans several projects.
    fn part_of_ancestries(&self, handles: &[String]) -> Result<Vec<ProjectAncestry>>;
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

    if let Some(project) = folder_marker_project(sources)? {
        return Ok(project);
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

/// The folder-marker rung. The projects the change's own files are attached to when it
/// touches any attached folder, otherwise the folder the session stands in.
fn folder_marker_project(sources: &impl ProjectContextSources) -> Result<Option<ResolvedProject>> {
    let start = sources.start_dir()?;
    let spanned: Vec<String> = touched_marker_handles(&sources.touched_paths(&start))?
        .into_iter()
        .collect();

    match spanned.as_slice() {
        [] => Ok(nearest_marker_handle(&start)?.map(folder_marker)),
        [only] => Ok(Some(folder_marker(only.clone()))),
        _ => spans_projects(spanned, sources).map(Some),
    }
}

fn folder_marker(handle: String) -> ResolvedProject {
    ResolvedProject::Determined {
        handle,
        source: ProjectSource::FolderMarker,
    }
}

/// A change that touches several projects: recorded for the nearest project they are all
/// part of, or -- with none in common -- left to the personal fallback, never refused.
fn spans_projects(
    spanned: Vec<String>,
    sources: &impl ProjectContextSources,
) -> Result<ResolvedProject> {
    let ancestries = sources.part_of_ancestries(&spanned)?;
    Ok(match nearest_common_ancestor(&ancestries) {
        Some(parent) => ResolvedProject::SpansUnderParent { parent, spanned },
        None => ResolvedProject::SpansUnrelated { spanned },
    })
}

/// The nearest project every one of `ancestries` is part of. A project is the start of its
/// own chain, so a change touching a project's own files and one of its sub-projects belongs
/// to that project. An unregistered handle has no chain beyond itself, so it shares a parent
/// with nothing: a typo in a marker never silently picks a parent.
fn nearest_common_ancestor(ancestries: &[ProjectAncestry]) -> Option<String> {
    let chains: Vec<Vec<&str>> = ancestries
        .iter()
        .map(|ancestry| {
            std::iter::once(ancestry.handle.as_str())
                .chain(ancestry.ancestors.iter().map(String::as_str))
                .collect()
        })
        .collect();
    let (first, rest) = chains.split_first()?;
    first
        .iter()
        .find(|candidate| rest.iter().all(|chain| chain.contains(candidate)))
        .map(|candidate| (*candidate).to_owned())
}

/// The distinct project handles the touched files' folders are attached to: the nearest
/// marker at or above each file's folder. A file under no marker adds nothing. A broken
/// marker is refused, as on the cwd walk.
fn touched_marker_handles(touched: &[PathBuf]) -> Result<BTreeSet<String>> {
    let folders: BTreeSet<&Path> = touched.iter().filter_map(|path| path.parent()).collect();
    let mut handles = BTreeSet::new();
    for folder in folders {
        handles.extend(nearest_marker_handle(folder)?);
    }
    Ok(handles)
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

    fn touched_paths(&self, dir: &Path) -> Vec<PathBuf> {
        git_touched_paths(dir)
    }

    fn part_of_ancestries(&self, handles: &[String]) -> Result<Vec<ProjectAncestry>> {
        Ok(get_project_ancestries(self.ledger, handles)?.data)
    }
}

/// The files the change under way in the repository around `dir` touches: the working-tree
/// diff plus the staged set, the same two reads the situational query defaults to (see
/// `git_default_diff_paths` in the runner), made absolute so each file can be walked up to its
/// marker. `git diff` prints repository-root-relative names, hence the top-level join.
///
/// Empty when `dir` is not in a repository or `git` is not there: no change is known, so the
/// caller falls back to the folder it stands in rather than refusing to capture.
fn git_touched_paths(dir: &Path) -> Vec<PathBuf> {
    let Some(toplevel) = git_stdout(dir, &["rev-parse", "--show-toplevel"]) else {
        return Vec::new();
    };
    let toplevel = PathBuf::from(String::from_utf8_lossy(&toplevel).trim_end_matches(['\n', '\r']));

    let mut touched = BTreeSet::new();
    for scope in [None, Some("--cached")] {
        // `diff.relative=false` because a repository may set it, which would print names
        // relative to `dir` instead. `-z` keeps a name with unusual characters unquoted.
        let mut args = vec!["-c", "diff.relative=false", "diff", "--name-only", "-z"];
        args.extend(scope);
        let Some(names) = git_stdout(dir, &args) else {
            continue;
        };
        touched.extend(
            String::from_utf8_lossy(&names)
                .split('\0')
                .filter(|name| !name.is_empty())
                .map(|name| toplevel.join(name)),
        );
    }
    touched.into_iter().collect()
}

/// `git -C dir args...`'s stdout, `None` when git could not be run or exited non-zero.
fn git_stdout(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
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
