#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  query-decisions.sh ["free text query"] [--q "..."] [--topic t,...]
                     [--status s,...] [--actor-id actor,...]
                     [--source agent|human] [--since T] [--until T]
                     [--limit 10] [--cursor C] [--hivemind-dir DIR]

"What did we decide about X?" Free text first; forwards to the fluent verb:
  hivemind query recall

`--q "..."` is accepted for backward compatibility and is treated the same
as a bare positional query.

No actor or source filter is applied unless you pass --actor-id or
--source explicitly — this is a free-text search over every decision in
the ledger, not just the ones the calling session touched.

For deeper follow-up on a specific decision — why it was made, whether it
still holds, contesting or superseding it — install the hivemind-context
plugin and use its fluent verbs (recall/why/verify/chain/disagree/supersede);
none of them take a decision id as primary input either. See
docs/AGENT_DECISION_CONTEXT.md.
USAGE
}

worktree_root() {
  if [[ -n "${CLAUDE_PROJECT_DIR:-}" ]]; then
    printf '%s\n' "$CLAUDE_PROJECT_DIR"
  else
    git rev-parse --path-format=absolute --show-toplevel 2>/dev/null || pwd
  fi
}

project_root() {
  local common_dir
  common_dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
  if [[ -n "$common_dir" ]]; then
    dirname "$common_dir"
  else
    worktree_root
  fi
}

log_hivemind_resolution() {
  printf 'hivemind-dir resolved to %s (rig-root=%s worktree=%s)\n' \
    "$HIVEMIND_DIR" "$PROJECT_ROOT" "$WORKTREE_ROOT" >&2
}

install_hint() {
  cat >&2 <<'HINT'
HiveMind CLI was not found.

Install it first, then retry:
  cargo install --path /path/to/hivemind

Or set HIVEMIND_CAPTURE_BIN to a built hivemind binary.
HINT
}

WORKTREE_ROOT="$(worktree_root)"
PROJECT_ROOT="$(project_root)"
HIVEMIND_DIR="${HIVEMIND_DIR:-${CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR:-$PROJECT_ROOT/hivemind}}"
HAS_LIMIT=0
QUERY=""
FORWARDED=()

# The positional free-text query, if present, must be the first argument
# (mirrors QueryRecallArgs' positional `query` field). Every later bare
# token is a flag's value and must stay in place in FORWARDED, not be
# captured here — otherwise `--topic smoke` would lose its value to this
# check on the next loop iteration.
if [[ $# -gt 0 && "$1" != -* ]]; then
  QUERY="$1"
  shift
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --hivemind-dir)
      HIVEMIND_DIR="${2:-}"
      shift 2
      ;;
    --q)
      QUERY="${2:-}"
      shift 2
      ;;
    --actor)
      FORWARDED+=(--actor-id "${2:-}")
      shift 2
      ;;
    --actor-id)
      FORWARDED+=(--actor-id "${2:-}")
      shift 2
      ;;
    --source)
      FORWARDED+=(--source "${2:-}")
      shift 2
      ;;
    --limit)
      HAS_LIMIT=1
      FORWARDED+=(--limit "${2:-}")
      shift 2
      ;;
    --)
      shift
      FORWARDED+=("$@")
      break
      ;;
    *)
      FORWARDED+=("$1")
      shift
      ;;
  esac
done

if [[ "$HAS_LIMIT" -eq 0 ]]; then
  FORWARDED+=(--limit 10)
fi

log_hivemind_resolution

if [[ -n "${HIVEMIND_CAPTURE_BIN:-}" ]]; then
  BASE_CMD=("$HIVEMIND_CAPTURE_BIN")
elif command -v hivemind >/dev/null 2>&1; then
  BASE_CMD=("$(command -v hivemind)")
elif [[ -x "$PROJECT_ROOT/target/debug/hivemind" ]]; then
  BASE_CMD=("$PROJECT_ROOT/target/debug/hivemind")
elif [[ -f "$PROJECT_ROOT/Cargo.toml" && -f "$PROJECT_ROOT/src/main.rs" ]]; then
  BASE_CMD=(cargo run --quiet --manifest-path "$PROJECT_ROOT/Cargo.toml" --)
else
  install_hint
  exit 127
fi

QUERY_ARGS=()
if [[ -n "$QUERY" ]]; then
  QUERY_ARGS=("$QUERY")
fi

exec "${BASE_CMD[@]}" --hivemind-dir "$HIVEMIND_DIR" query recall \
  "${QUERY_ARGS[@]}" "${FORWARDED[@]}"
