#!/usr/bin/env sh
# Lint GitHub Actions workflow files with actionlint.
#
# actionlint integrates with shellcheck for `run:` blocks, but does not
# bundle it: it execs a `shellcheck` binary and, if none is found, silently
# disables the rule and still exits 0 (see hivemind-w5rw). So this script
# self-installs both actionlint and shellcheck into a local cache directory
# and passes the shellcheck path explicitly via `-shellcheck`, then verifies
# (via `-verbose`) that the shellcheck rule actually ran. If shellcheck
# integration is disabled for any reason, the gate fails loudly instead of
# reporting a silently partial lint.
#
# Usage:
#   ./scripts/lint-workflows.sh
set -eu

ACTIONLINT_VERSION="${ACTIONLINT_VERSION:-1.7.12}"
SHELLCHECK_VERSION="${SHELLCHECK_VERSION:-0.11.0}"
repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cache_dir="$repo_root/.cache/bin"

tmp_dirs=""
cleanup() {
  for d in $tmp_dirs; do rm -rf "$d"; done
}
trap cleanup EXIT

# Map `uname` output to the OS/arch tokens each project's release assets use.
os_arch_tokens() {
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$arch" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *)
      echo "lint-workflows.sh: unsupported architecture '$arch' for auto-install" >&2
      exit 1
      ;;
  esac
  case "$os" in
    linux | darwin) ;;
    *)
      echo "lint-workflows.sh: unsupported OS '$os' for auto-install" >&2
      exit 1
      ;;
  esac
}

actionlint_bin="$cache_dir/actionlint-$ACTIONLINT_VERSION"
if command -v actionlint >/dev/null 2>&1; then
  actionlint_bin="$(command -v actionlint)"
elif [ ! -x "$actionlint_bin" ]; then
  echo "actionlint not found on PATH; installing v$ACTIONLINT_VERSION into $cache_dir..."
  mkdir -p "$cache_dir"
  tmp_dir="$(mktemp -d)"
  tmp_dirs="$tmp_dirs $tmp_dir"
  curl -fsSL "https://raw.githubusercontent.com/rhysd/actionlint/v$ACTIONLINT_VERSION/scripts/download-actionlint.bash" \
    | bash -s -- "$ACTIONLINT_VERSION" "$tmp_dir"
  mv "$tmp_dir/actionlint" "$actionlint_bin"
fi

shellcheck_bin="$cache_dir/shellcheck-$SHELLCHECK_VERSION"
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck_bin="$(command -v shellcheck)"
elif [ ! -x "$shellcheck_bin" ]; then
  echo "shellcheck not found on PATH; installing v$SHELLCHECK_VERSION into $cache_dir..."
  mkdir -p "$cache_dir"
  os_arch_tokens
  tmp_dir="$(mktemp -d)"
  tmp_dirs="$tmp_dirs $tmp_dir"
  archive="shellcheck-v$SHELLCHECK_VERSION.$os.$arch.tar.gz"
  curl -fsSL "https://github.com/koalaman/shellcheck/releases/download/v$SHELLCHECK_VERSION/$archive" \
    -o "$tmp_dir/$archive"
  tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"
  mv "$tmp_dir/shellcheck-v$SHELLCHECK_VERSION/shellcheck" "$shellcheck_bin"
  chmod +x "$shellcheck_bin"
fi

echo "Running actionlint $ACTIONLINT_VERSION with shellcheck $SHELLCHECK_VERSION ($shellcheck_bin)..."
set +e
output="$("$actionlint_bin" -verbose -shellcheck "$shellcheck_bin" 2>&1)"
status=$?
set -e

if printf '%s\n' "$output" | grep -q 'Rule "shellcheck" was disabled'; then
  printf '%s\n' "$output" >&2
  echo "" >&2
  echo "ERROR: actionlint disabled its shellcheck rule instead of running it (see above)." >&2
  echo "Embedded shell in .github/workflows/**/run: blocks was NOT linted. Refusing to" >&2
  echo "report a passing gate for a partial lint — fix the shellcheck install and retry." >&2
  exit 1
fi

# Verbose diagnostic lines (project detection, per-file timing, disabled
# rules we don't rely on like pyflakes) are noise once we've confirmed
# shellcheck actually ran; keep real findings and drop the rest.
printf '%s\n' "$output" | grep -v '^verbose:' || true
exit "$status"
