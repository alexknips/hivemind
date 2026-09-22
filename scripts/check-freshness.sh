#!/usr/bin/env sh
set -eu

# scripts/check-freshness.sh [cell-base-url]
#
# One-line freshness check: is what's actually running (the local
# `hivemind` on PATH, and optionally a cell's /v1/version) the same commit
# as origin/master? Prints a single machine-parseable line and exits 0 when
# everything checked is fresh, 1 otherwise — so a scheduled dog order (or a
# human) can act on the exit code alone, or read the line for which side is
# stale (hivemind-zdsh.7).
#
# The cell URL is optional: pass it as $1, or set HIVEMIND_CELL_URL. With
# neither, only the local binary is checked.

cell_url="${1:-${HIVEMIND_CELL_URL:-}}"

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
master_sha="$(git -C "$repo_root" rev-parse --short=12 origin/master 2>/dev/null || echo "unknown")"

if command -v hivemind >/dev/null 2>&1; then
  local_sha="$(hivemind --version 2>/dev/null | sed -n 's/.*+\([0-9a-f]\{1,\}\)$/\1/p')"
  [ -n "$local_sha" ] || local_sha="unknown"
else
  local_sha="missing"
fi

if [ -n "$cell_url" ]; then
  cell_sha="$(curl -fsS "${cell_url%/}/v1/version" 2>/dev/null | sed -n 's/.*"sha":"\([0-9a-f]*\)".*/\1/p')"
  [ -n "$cell_sha" ] || cell_sha="unreachable"
else
  cell_sha="skipped"
fi

fresh=true
case "$master_sha" in
  unknown) fresh=false ;;
esac
[ "$local_sha" = "$master_sha" ] || fresh=false
if [ "$cell_sha" != "skipped" ]; then
  [ "$cell_sha" = "$master_sha" ] || fresh=false
fi

echo "MASTER=${master_sha} LOCAL=${local_sha} CELL=${cell_sha} FRESH=${fresh}"

[ "$fresh" = "true" ]
