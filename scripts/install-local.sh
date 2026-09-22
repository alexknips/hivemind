#!/usr/bin/env sh
set -eu

# scripts/install-local.sh [ref]
#
# Builds the hivemind CLI from a given git ref and installs it to
# ~/.local/bin (override with HIVEMIND_INSTALL_DIR) — the same location
# scripts/install.sh uses for published releases, and where Gas Town rigs
# resolve `hivemind` on PATH ahead of ~/.cargo/bin.
#
# Unlike `make install` (which builds whatever is currently checked out),
# this takes an explicit ref and builds it in a disposable git worktree, so
# it never depends on — or disturbs — the state of the caller's checkout.
# Use it to install a specific commit (e.g. origin/master) between tagged
# releases; scripts/install.sh only installs published GitHub releases,
# which lag master by however long it's been since the last `v*` tag.
#
# The installed binary's `--version` embeds the exact commit built
# (<cargo semver>+<short sha>, via build.rs) so drift from what a ref
# actually contains is never silent (hivemind-zdsh.7).

ref="${1:-origin/master}"
install_dir="${HIVEMIND_INSTALL_DIR:-$HOME/.local/bin}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

case "$ref" in
  origin/*) git fetch origin --quiet ;;
esac

full_sha="$(git rev-parse "$ref")"
short_sha="$(git rev-parse --short=12 "$ref")"

worktree_dir="$(mktemp -d)"
cleanup() {
  git -C "$repo_root" worktree remove --force "$worktree_dir" >/dev/null 2>&1 || true
  rm -rf "$worktree_dir"
}
trap cleanup EXIT INT TERM

echo "Checking out ${ref} (${short_sha}) into a disposable worktree..."
git worktree add --detach --quiet "$worktree_dir" "$full_sha"

echo "Building (this can take a few minutes on a cold cache)..."
# Point at a target/ dir outside the disposable worktree so repeat runs —
# e.g. a nightly dog order re-running this against a new master commit —
# reuse cached dependency builds instead of recompiling from scratch every
# time. Cargo supports a target dir outside the source tree for exactly
# this reason.
build_target_dir="${HIVEMIND_BUILD_TARGET_DIR:-$repo_root/target}"
(cd "$worktree_dir" && \
  CARGO_TARGET_DIR="$build_target_dir" \
  HIVEMIND_BUILD_SHA="$full_sha" \
  cargo build --release --locked --bin hivemind)

mkdir -p "$install_dir"
install -m 0755 "${build_target_dir}/release/hivemind" "${install_dir}/hivemind"

echo "installed $("${install_dir}/hivemind" --version)"
echo "  -> ${install_dir}/hivemind"
