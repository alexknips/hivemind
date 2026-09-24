use std::cell::Cell;
use std::collections::BTreeMap;

use super::*;

/// A throwaway directory tree, removed on drop.
struct TempTree {
    root: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("hivemind-project-context-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp tree root");
        Self { root }
    }

    /// Creates `relative` (and its parents) as a directory and returns it.
    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path).expect("create temp tree dir");
        path
    }

    /// Writes a `.hivemind-project` marker holding `contents` into `relative`.
    fn marker(&self, relative: &str, contents: &str) {
        let dir = self.dir(relative);
        std::fs::write(dir.join(PROJECT_MARKER_FILE_NAME), contents).expect("write marker");
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Sources with every rung switchable, counting the reads a rung the ladder should not
/// have reached would have made.
struct FakeSources {
    start_dir: PathBuf,
    rig: Option<String>,
    /// rig name -> handle of the project anchored to it
    rig_anchors: BTreeMap<String, String>,
    current_project: Option<String>,
    registry_reads: Cell<usize>,
    current_project_reads: Cell<usize>,
}

impl FakeSources {
    fn at(start_dir: PathBuf) -> Self {
        Self {
            start_dir,
            rig: None,
            rig_anchors: BTreeMap::new(),
            current_project: None,
            registry_reads: Cell::new(0),
            current_project_reads: Cell::new(0),
        }
    }

    fn with_rig(mut self, rig: &str, anchored_project: Option<&str>) -> Self {
        self.rig = Some(rig.to_owned());
        if let Some(handle) = anchored_project {
            self.rig_anchors.insert(rig.to_owned(), handle.to_owned());
        }
        self
    }

    fn with_current_project(mut self, handle: &str) -> Self {
        self.current_project = Some(handle.to_owned());
        self
    }
}

impl ProjectContextSources for FakeSources {
    fn start_dir(&self) -> Result<PathBuf> {
        Ok(self.start_dir.clone())
    }

    fn rig(&self) -> Option<String> {
        self.rig.clone()
    }

    fn project_anchored_to_rig(&self, rig: &str) -> Result<Option<String>> {
        self.registry_reads.set(self.registry_reads.get() + 1);
        Ok(self.rig_anchors.get(rig).cloned())
    }

    fn current_project(&self) -> Result<Option<String>> {
        self.current_project_reads
            .set(self.current_project_reads.get() + 1);
        Ok(self.current_project.clone())
    }
}

fn determined(handle: &str, source: ProjectSource) -> ResolvedProject {
    ResolvedProject::Determined {
        handle: handle.to_owned(),
        source,
    }
}

fn resolve(sources: &FakeSources) -> ResolvedProject {
    resolve_project_from_context(None, sources).expect("ladder resolves")
}

// ---------------------------------------------------------------------------
// Each rung in isolation
// ---------------------------------------------------------------------------

#[test]
fn stated_project_is_taken_as_stated_with_the_source_the_caller_gave() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("repo"));

    let resolved = resolve_project_from_context(
        Some(DeterminedProject {
            handle: "billing",
            source: ProjectSource::Job,
        }),
        &sources,
    )
    .expect("stated resolves");

    assert_eq!(resolved, determined("billing", ProjectSource::Job));
}

#[test]
fn a_marker_in_the_working_directory_names_the_project() {
    let tree = TempTree::new();
    tree.marker("repo", "billing\n");

    let resolved = resolve(&FakeSources::at(tree.dir("repo")));

    assert_eq!(resolved, determined("billing", ProjectSource::FolderMarker));
}

#[test]
fn the_marker_walk_climbs_from_a_nested_working_directory() {
    let tree = TempTree::new();
    tree.marker("repo", "billing");

    let resolved = resolve(&FakeSources::at(tree.dir("repo/src/deeply/nested")));

    assert_eq!(resolved, determined("billing", ProjectSource::FolderMarker));
}

#[test]
fn the_nearest_marker_wins_so_a_nested_marker_is_a_sub_project() {
    let tree = TempTree::new();
    tree.marker("repo", "platform");
    tree.marker("repo/services/billing", "billing");

    let inside = resolve(&FakeSources::at(tree.dir("repo/services/billing/src")));
    let beside = resolve(&FakeSources::at(tree.dir("repo/services/auth")));

    assert_eq!(inside, determined("billing", ProjectSource::FolderMarker));
    assert_eq!(beside, determined("platform", ProjectSource::FolderMarker));
}

#[test]
fn marker_whitespace_and_line_endings_around_the_handle_are_ignored() {
    let tree = TempTree::new();
    tree.marker("crlf", "billing\r\n");
    tree.marker("padded", "  billing  \n\n");

    for dir in ["crlf", "padded"] {
        let resolved = resolve(&FakeSources::at(tree.dir(dir)));
        assert_eq!(
            resolved,
            determined("billing", ProjectSource::FolderMarker),
            "{dir}"
        );
    }
}

#[test]
fn a_marker_that_is_not_exactly_one_handle_is_refused_not_skipped() {
    let tree = TempTree::new();
    // An outer marker exists: skipping the broken inner one would silently file the capture
    // under a project the inner folder did not choose.
    tree.marker("repo", "platform");

    for (dir, contents) in [
        ("repo/empty", ""),
        ("repo/blank", "  \n\n"),
        ("repo/two-lines", "billing\nauth\n"),
        ("repo/two-words", "billing auth"),
    ] {
        tree.marker(dir, contents);
        let error = resolve_project_from_context(None, &FakeSources::at(tree.dir(dir)))
            .expect_err("a malformed marker is refused");
        let message = error.to_string();
        assert!(
            message.contains(PROJECT_MARKER_FILE_NAME)
                && message.contains("exactly one project handle"),
            "{dir}: {message}"
        );
    }
}

#[test]
fn a_directory_that_happens_to_be_named_like_the_marker_is_not_a_marker() {
    let tree = TempTree::new();
    tree.marker("repo", "platform");
    tree.dir(&format!("repo/sub/{PROJECT_MARKER_FILE_NAME}"));

    let resolved = resolve(&FakeSources::at(tree.dir("repo/sub")));

    assert_eq!(
        resolved,
        determined("platform", ProjectSource::FolderMarker)
    );
}

#[test]
fn the_rig_names_the_project_anchored_to_it() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("no-marker")).with_rig("hivemind-ui", Some("ui"));

    assert_eq!(resolve(&sources), determined("ui", ProjectSource::Rig));
    assert_eq!(sources.registry_reads.get(), 1, "one registry read");
}

#[test]
fn a_rig_with_no_anchored_project_falls_through() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("no-marker"))
        .with_rig("scratch", None)
        .with_current_project("billing");

    assert_eq!(
        resolve(&sources),
        determined("billing", ProjectSource::CurrentProject)
    );
    assert_eq!(sources.registry_reads.get(), 1, "the registry is read once");
}

#[test]
fn no_rig_means_no_registry_read() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("no-marker")).with_current_project("billing");

    assert_eq!(
        resolve(&sources),
        determined("billing", ProjectSource::CurrentProject)
    );
    assert_eq!(sources.registry_reads.get(), 0);
}

#[test]
fn the_current_project_names_the_project_when_nothing_nearer_does() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("no-marker")).with_current_project("billing");

    assert_eq!(
        resolve(&sources),
        determined("billing", ProjectSource::CurrentProject)
    );
}

#[test]
fn with_nothing_to_go_on_the_personal_fallback_carries_the_reminder() {
    let tree = TempTree::new();
    let resolved = resolve(&FakeSources::at(tree.dir("no-marker")));

    assert_eq!(resolved, ResolvedProject::PersonalFallback);
    assert_eq!(resolved.determined(), None);
    assert_eq!(
        resolved.reminder(),
        Some(
            "this folder is not attached to a project yet; run hivemind project anchor ... to attach it"
        )
    );
}

#[test]
fn a_determined_project_carries_no_reminder() {
    let resolved = determined("billing", ProjectSource::FolderMarker);

    assert_eq!(resolved.reminder(), None);
    assert_eq!(
        resolved.determined(),
        Some(DeterminedProject {
            handle: "billing",
            source: ProjectSource::FolderMarker,
        })
    );
}

// ---------------------------------------------------------------------------
// Precedence: stated > marker > rig > current project > personal
// ---------------------------------------------------------------------------

/// Every rung offers a different project, so the winner names the rung.
fn every_rung_offers_a_project(tree: &TempTree) -> FakeSources {
    tree.marker("repo", "from-marker");
    FakeSources::at(tree.dir("repo/src"))
        .with_rig("some-rig", Some("from-rig"))
        .with_current_project("from-current")
}

#[test]
fn stated_beats_marker_rig_and_current_project() {
    let tree = TempTree::new();
    let sources = every_rung_offers_a_project(&tree);

    let resolved =
        resolve_project_from_context(Some(DeterminedProject::stated("from-flag")), &sources)
            .expect("stated resolves");

    assert_eq!(resolved, determined("from-flag", ProjectSource::Stated));
    assert_eq!(sources.registry_reads.get(), 0, "stated needs no registry");
    assert_eq!(sources.current_project_reads.get(), 0);
}

#[test]
fn marker_beats_rig_and_current_project() {
    let tree = TempTree::new();
    let sources = every_rung_offers_a_project(&tree);

    assert_eq!(
        resolve(&sources),
        determined("from-marker", ProjectSource::FolderMarker)
    );
    assert_eq!(
        sources.registry_reads.get(),
        0,
        "a marker needs no registry"
    );
    assert_eq!(sources.current_project_reads.get(), 0);
}

#[test]
fn rig_beats_current_project() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("no-marker"))
        .with_rig("some-rig", Some("from-rig"))
        .with_current_project("from-current");

    assert_eq!(
        resolve(&sources),
        determined("from-rig", ProjectSource::Rig)
    );
    assert_eq!(sources.current_project_reads.get(), 0);
}

#[test]
fn current_project_beats_the_personal_fallback() {
    let tree = TempTree::new();
    let sources = FakeSources::at(tree.dir("no-marker")).with_current_project("from-current");

    assert_eq!(
        resolve(&sources),
        determined("from-current", ProjectSource::CurrentProject)
    );
}

#[test]
fn the_ladder_steps_down_one_rung_at_a_time() {
    let tree = TempTree::new();
    let mut sources = every_rung_offers_a_project(&tree);
    assert_eq!(
        resolve(&sources),
        determined("from-marker", ProjectSource::FolderMarker)
    );

    // Detach the folder, and the rig answers.
    sources.start_dir = tree.dir("elsewhere");
    assert_eq!(
        resolve(&sources),
        determined("from-rig", ProjectSource::Rig)
    );

    // Unanchor the rig, and the current project answers.
    sources.rig_anchors.clear();
    assert_eq!(
        resolve(&sources),
        determined("from-current", ProjectSource::CurrentProject)
    );

    // Clear the current project, and it is the personal fallback.
    sources.current_project = None;
    assert_eq!(resolve(&sources), ResolvedProject::PersonalFallback);
}

// ---------------------------------------------------------------------------
// The process environment
// ---------------------------------------------------------------------------

#[test]
fn the_env_trims_the_rig_and_the_person_and_drops_an_empty_rig() {
    let env = ProjectContextEnv::new(None, Some("  hivemind  "), " human:alice ");
    assert_eq!(env.rig.as_deref(), Some("hivemind"));
    assert_eq!(env.person, "human:alice");

    assert_eq!(
        ProjectContextEnv::new(None, Some("  "), "human:alice").rig,
        None
    );
    assert_eq!(ProjectContextEnv::new(None, None, "human:alice").rig, None);
}
