//! Embeds the commit this binary was built from into `HIVEMIND_BUILD_SHA`
//! (compile-time env var, consumed via `env!` in `src/lib.rs`), so
//! `hivemind --version` and `/v1/version` report what was actually built
//! instead of a bare, unbumped Cargo.toml semver (hivemind-zdsh.7).
//!
//! `HIVEMIND_BUILD_SHA` overrides the git lookup when set: the Docker build
//! context excludes `.git` (see `.dockerignore`), so the Dockerfile passes it
//! in via a `GIT_SHA` build-arg instead.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=HIVEMIND_BUILD_SHA");

    let sha = std::env::var("HIVEMIND_BUILD_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(git_head_sha)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=HIVEMIND_BUILD_SHA={}", short(&sha));

    // Watch the real HEAD/refs files git resolves for THIS checkout, not a
    // hardcoded ".git/HEAD" — inside a git worktree (every polecat/crew/
    // refinery checkout in this city) ".git" is a gitlink file, not a
    // directory, so that path doesn't exist and a hardcoded rerun-if-changed
    // would error out the build.
    if let Some(head_path) = run_git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    if let Some(common_dir) = run_git(&["rev-parse", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common_dir}/refs");
    }
}

fn short(sha: &str) -> String {
    sha.trim().chars().take(12).collect()
}

fn git_head_sha() -> Option<String> {
    run_git(&["rev-parse", "HEAD"])
}

fn run_git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}
