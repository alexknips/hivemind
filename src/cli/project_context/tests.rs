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
    /// The files the change under way touches
    touched: Vec<PathBuf>,
    /// project handle -> handle of the project it is part of
    parents: BTreeMap<String, String>,
    registry_reads: Cell<usize>,
    current_project_reads: Cell<usize>,
    ancestry_reads: Cell<usize>,
}

impl FakeSources {
    fn at(start_dir: PathBuf) -> Self {
        Self {
            start_dir,
            rig: None,
            rig_anchors: BTreeMap::new(),
            current_project: None,
            touched: Vec::new(),
            parents: BTreeMap::new(),
            registry_reads: Cell::new(0),
            current_project_reads: Cell::new(0),
            ancestry_reads: Cell::new(0),
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

    /// The change touches these files.
    fn with_touched(mut self, paths: &[PathBuf]) -> Self {
        self.touched = paths.to_vec();
        self
    }

    /// `child` is registered as part of `parent`.
    fn with_part_of(mut self, child: &str, parent: &str) -> Self {
        self.parents.insert(child.to_owned(), parent.to_owned());
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

    fn touched_paths(&self, _dir: &Path) -> Vec<PathBuf> {
        self.touched.clone()
    }

    fn part_of_ancestries(&self, handles: &[String]) -> Result<Vec<ProjectAncestry>> {
        self.ancestry_reads.set(self.ancestry_reads.get() + 1);
        Ok(handles
            .iter()
            .map(|handle| {
                let mut ancestors: Vec<String> = Vec::new();
                let mut current = handle;
                while let Some(parent) = self.parents.get(current) {
                    if parent == handle || ancestors.contains(parent) {
                        break;
                    }
                    ancestors.push(parent.clone());
                    current = parent;
                }
                ProjectAncestry {
                    handle: handle.clone(),
                    ancestors,
                }
            })
            .collect())
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
        resolved.reminder(true).as_deref(),
        Some(
            "this folder is not attached to a project yet; run hivemind project anchor ... to attach it"
        )
    );
    assert_eq!(
        resolved.reminder(false),
        None,
        "a supersede that inherited a shared project has nothing to be reminded about"
    );
}

#[test]
fn a_determined_project_carries_no_reminder() {
    let resolved = determined("billing", ProjectSource::FolderMarker);

    assert_eq!(resolved.reminder(true), None);
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
// A change that spans several attached folders (hivemind-s15q.14)
// ---------------------------------------------------------------------------

/// A platform repo with two sub-project folders, each with its own marker, and a folder no
/// marker reaches:
///
/// ```text
/// repo/.hivemind-project                   platform
/// repo/services/billing/.hivemind-project  billing
/// repo/services/auth/.hivemind-project     auth
/// repo/services/ledger/.hivemind-project   ledger
/// unattached/
/// ```
fn spanning_tree() -> TempTree {
    let tree = TempTree::new();
    tree.marker("repo", "platform");
    tree.marker("repo/services/billing", "billing");
    tree.marker("repo/services/auth", "auth");
    tree.marker("repo/services/ledger", "ledger");
    tree.dir("unattached");
    tree
}

fn files(tree: &TempTree, relative: &[&str]) -> Vec<PathBuf> {
    relative.iter().map(|path| tree.root.join(path)).collect()
}

fn spanned(handles: &[&str]) -> Vec<String> {
    handles.iter().map(|handle| (*handle).to_owned()).collect()
}

#[test]
fn a_change_under_one_marker_is_that_project_without_a_registry_read() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(
            &tree,
            &[
                "repo/services/billing/queue.rs",
                "repo/services/billing/src/worker.rs",
                "repo/services/billing/README.md",
            ],
        ))
        .with_part_of("billing", "platform");

    assert_eq!(
        resolve(&sources),
        determined("billing", ProjectSource::FolderMarker)
    );
    assert_eq!(
        sources.ancestry_reads.get(),
        0,
        "one project needs no registry"
    );
}

#[test]
fn the_touched_files_decide_over_the_folder_the_session_stands_in() {
    let tree = spanning_tree();
    // Standing at the repo root (marker `platform`), but the change is all in auth.
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(&tree, &["repo/services/auth/tokens.rs"]));

    assert_eq!(
        resolve(&sources),
        determined("auth", ProjectSource::FolderMarker)
    );
}

#[test]
fn files_under_no_marker_add_nothing_to_the_projects_a_change_spans() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("unattached")).with_touched(&files(
        &tree,
        &["unattached/notes.md", "repo/services/billing/queue.rs"],
    ));

    assert_eq!(
        resolve(&sources),
        determined("billing", ProjectSource::FolderMarker)
    );
}

#[test]
fn a_change_touching_no_attached_folder_falls_back_to_the_folder_the_session_stands_in() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("repo/services/auth"))
        .with_touched(&files(&tree, &["unattached/notes.md"]));

    assert_eq!(
        resolve(&sources),
        determined("auth", ProjectSource::FolderMarker)
    );
}

#[test]
fn two_projects_under_one_parent_are_recorded_for_the_parent_and_the_reply_says_so() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(
            &tree,
            &[
                "repo/services/billing/queue.rs",
                "repo/services/auth/tokens.rs",
            ],
        ))
        .with_part_of("billing", "platform")
        .with_part_of("auth", "platform");

    let resolved = resolve(&sources);

    assert_eq!(
        resolved,
        ResolvedProject::SpansUnderParent {
            parent: "platform".to_owned(),
            spanned: spanned(&["auth", "billing"]),
        }
    );
    assert_eq!(
        resolved.determined(),
        Some(DeterminedProject {
            handle: "platform",
            source: ProjectSource::FolderMarker,
        }),
        "the write path is handed the one parent, never the two"
    );
    assert_eq!(
        resolved.reminder(false).as_deref(),
        Some("recorded for platform: this change spans auth and billing"),
        "said whether or not anything landed in the personal project"
    );
    assert_eq!(sources.ancestry_reads.get(), 1, "the registry is read once");
    assert_eq!(
        sources.registry_reads.get(),
        0,
        "and the rig is never asked"
    );
}

#[test]
fn two_projects_with_no_common_parent_land_in_the_personal_project_and_the_reminder_names_both() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("repo"))
        // Even offered by the rest of the ladder, the change is not misattributed to it.
        .with_rig("some-rig", Some("from-rig"))
        .with_current_project("from-current")
        .with_touched(&files(
            &tree,
            &[
                "repo/services/billing/queue.rs",
                "repo/services/auth/tokens.rs",
            ],
        ));

    let resolved = resolve(&sources);

    assert_eq!(
        resolved,
        ResolvedProject::SpansUnrelated {
            spanned: spanned(&["auth", "billing"]),
        }
    );
    assert_eq!(
        resolved.determined(),
        None,
        "the write layer falls back to the personal project itself, and says so"
    );
    let reminder = resolved.reminder(true).expect("the fallback is announced");
    assert_eq!(
        reminder,
        "this change spans auth and billing, which share no parent. Move it with hivemind move ..., or register a parent."
    );
    assert_eq!(
        resolved.reminder(false),
        None,
        "a supersede that inherited a shared project was not saved to the personal project"
    );
    assert_eq!(
        sources.registry_reads.get(),
        0,
        "the rig rung is not consulted"
    );
    assert_eq!(sources.current_project_reads.get(), 0);
}

#[test]
fn three_projects_at_mixed_depths_resolve_to_their_nearest_common_ancestor() {
    let tree = spanning_tree();
    // billing and auth are part of platform, platform of city; ledger is part of city directly.
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(
            &tree,
            &[
                "repo/services/billing/queue.rs",
                "repo/services/auth/tokens.rs",
                "repo/services/ledger/books.rs",
            ],
        ))
        .with_part_of("billing", "platform")
        .with_part_of("auth", "platform")
        .with_part_of("platform", "city")
        .with_part_of("ledger", "city");

    let resolved = resolve(&sources);

    assert_eq!(
        resolved,
        ResolvedProject::SpansUnderParent {
            parent: "city".to_owned(),
            spanned: spanned(&["auth", "billing", "ledger"]),
        }
    );
    assert_eq!(
        resolved.reminder(true).as_deref(),
        Some("recorded for city: this change spans auth, billing and ledger")
    );
}

#[test]
fn two_projects_that_share_a_parent_plus_a_third_that_does_not_go_personal_naming_all_three() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(
            &tree,
            &[
                "repo/services/billing/queue.rs",
                "repo/services/auth/tokens.rs",
                "repo/services/ledger/books.rs",
            ],
        ))
        .with_part_of("billing", "platform")
        .with_part_of("auth", "platform");

    let resolved = resolve(&sources);

    assert_eq!(
        resolved,
        ResolvedProject::SpansUnrelated {
            spanned: spanned(&["auth", "billing", "ledger"]),
        }
    );
    assert!(resolved
        .reminder(true)
        .expect("announced")
        .starts_with("this change spans auth, billing and ledger, which share no parent."));
}

#[test]
fn a_project_and_its_own_sub_project_are_recorded_for_the_outer_one() {
    let tree = spanning_tree();
    // A file at the repo root (marker platform) and one in billing (marker billing, part of
    // platform): a project is the start of its own chain.
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(
            &tree,
            &["repo/Cargo.toml", "repo/services/billing/queue.rs"],
        ))
        .with_part_of("billing", "platform");

    assert_eq!(
        resolve(&sources),
        ResolvedProject::SpansUnderParent {
            parent: "platform".to_owned(),
            spanned: spanned(&["billing", "platform"]),
        }
    );
}

#[test]
fn an_unregistered_marker_handle_shares_a_parent_with_nothing_and_is_never_dropped() {
    let tree = spanning_tree();
    tree.marker("repo/services/auth", "authn");
    let sources = FakeSources::at(tree.dir("repo"))
        .with_touched(&files(
            &tree,
            &[
                "repo/services/billing/queue.rs",
                "repo/services/auth/tokens.rs",
            ],
        ))
        .with_part_of("billing", "platform");

    let resolved = resolve(&sources);

    assert_eq!(
        resolved,
        ResolvedProject::SpansUnrelated {
            spanned: spanned(&["authn", "billing"]),
        },
        "the typo is named in the reminder; the decision is saved, not lost"
    );
}

#[test]
fn a_broken_marker_in_a_touched_folder_is_refused_not_skipped() {
    let tree = spanning_tree();
    tree.marker("repo/services/auth", "auth billing");
    let sources = FakeSources::at(tree.dir("repo")).with_touched(&files(
        &tree,
        &[
            "repo/services/billing/queue.rs",
            "repo/services/auth/tokens.rs",
        ],
    ));

    let error = resolve_project_from_context(None, &sources)
        .expect_err("a malformed marker is refused, as on the cwd walk");

    assert!(
        error.to_string().contains("exactly one project handle"),
        "{error}"
    );
}

#[test]
fn the_same_project_named_by_two_folders_is_one_project() {
    let tree = spanning_tree();
    tree.marker("repo/services/auth", "billing");
    let sources = FakeSources::at(tree.dir("repo")).with_touched(&files(
        &tree,
        &[
            "repo/services/billing/queue.rs",
            "repo/services/auth/tokens.rs",
        ],
    ));

    assert_eq!(
        resolve(&sources),
        determined("billing", ProjectSource::FolderMarker)
    );
    assert_eq!(sources.ancestry_reads.get(), 0);
}

#[test]
fn a_stated_project_is_never_second_guessed_by_the_files_touched() {
    let tree = spanning_tree();
    let sources = FakeSources::at(tree.dir("repo")).with_touched(&files(
        &tree,
        &[
            "repo/services/billing/queue.rs",
            "repo/services/auth/tokens.rs",
        ],
    ));

    let resolved = resolve_project_from_context(Some(DeterminedProject::stated("ops")), &sources)
        .expect("stated resolves");

    assert_eq!(resolved, determined("ops", ProjectSource::Stated));
    assert_eq!(sources.ancestry_reads.get(), 0);
}

#[test]
fn nearest_common_ancestor_is_the_nearest_of_the_shared_chain() {
    let chain = |handle: &str, ancestors: &[&str]| ProjectAncestry {
        handle: handle.to_owned(),
        ancestors: spanned(ancestors),
    };

    assert_eq!(
        nearest_common_ancestor(&[chain("a", &["p", "root"]), chain("b", &["q", "p", "root"]),])
            .as_deref(),
        Some("p"),
        "p, not root: the nearest they share"
    );
    assert_eq!(
        nearest_common_ancestor(&[chain("a", &[]), chain("b", &[])]),
        None
    );
    assert_eq!(nearest_common_ancestor(&[]), None);
}

// ---------------------------------------------------------------------------
// Reading the change from git
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=test@example.com", "-c", "user.name=Test"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_touched_paths_are_absolute_and_cover_the_working_tree_and_the_staged_set() {
    let tree = TempTree::new();
    let repo = tree.dir("repo");
    let repo = std::fs::canonicalize(&repo).expect("canonical repo path");
    tree.dir("repo/services/billing");
    tree.dir("repo/services/auth");
    tree.dir("repo/docs");
    for file in [
        "services/billing/queue.rs",
        "services/auth/tokens.rs",
        "docs/spare.md",
    ] {
        std::fs::write(repo.join(file), "one\n").expect("write file");
    }
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);

    // An unstaged edit, a staged edit, and an untouched file.
    std::fs::write(repo.join("services/billing/queue.rs"), "two\n").expect("edit file");
    std::fs::write(repo.join("services/auth/tokens.rs"), "two\n").expect("edit file");
    git(&repo, &["add", "services/auth/tokens.rs"]);

    // From a subfolder: `git diff` prints repository-root-relative names, made absolute here.
    let touched = git_touched_paths(&repo.join("services/billing"));

    assert_eq!(
        touched,
        vec![
            repo.join("services/auth/tokens.rs"),
            repo.join("services/billing/queue.rs"),
        ]
    );
}

#[test]
fn git_touched_paths_are_empty_outside_a_repository_and_for_a_clean_tree() {
    let tree = TempTree::new();
    let plain = tree.dir("plain");
    assert_eq!(git_touched_paths(&plain), Vec::<PathBuf>::new());
    assert_eq!(
        git_touched_paths(&tree.root.join("does-not-exist")),
        Vec::<PathBuf>::new()
    );

    let repo = tree.dir("repo");
    std::fs::write(repo.join("a.txt"), "one\n").expect("write file");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    assert_eq!(git_touched_paths(&repo), Vec::<PathBuf>::new());
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
