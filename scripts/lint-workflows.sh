#!/usr/bin/env sh
# Lint GitHub Actions workflow files with actionlint.
#
# actionlint bundles shellcheck integration for `run:` blocks, so this one
# tool covers both YAML-shape and embedded-shell mistakes in
# .github/workflows/** (see hivemind-w5rw). Self-installing so it works the
# same way on a bare CI runner or the refinery's build host: use actionlint
# from PATH if present, otherwise download the pinned release into a local
# cache directory.
#
# Usage:
#   ./scripts/lint-workflows.sh
set -eu

ACTIONLINT_VERSION="${ACTIONLINT_VERSION:-1.7.12}"
repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cache_dir="$repo_root/.cache/bin"
actionlint_bin="$cache_dir/actionlint-$ACTIONLINT_VERSION"

if command -v actionlint >/dev/null 2>&1; then
  actionlint_bin="$(command -v actionlint)"
elif [ ! -x "$actionlint_bin" ]; then
  echo "actionlint not found on PATH; installing v$ACTIONLINT_VERSION into $cache_dir..."
  mkdir -p "$cache_dir"
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
  curl -fsSL "https://raw.githubusercontent.com/rhysd/actionlint/v$ACTIONLINT_VERSION/scripts/download-actionlint.bash" \
    | bash -s -- "$ACTIONLINT_VERSION" "$tmp_dir"
  mv "$tmp_dir/actionlint" "$actionlint_bin"
fi

echo "Running actionlint $ACTIONLINT_VERSION (includes shellcheck for run: blocks)..."
"$actionlint_bin" -color
