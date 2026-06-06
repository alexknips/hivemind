#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  query-decisions.sh [--q "..."] [--actor-id actor] [--source agent|human]
                     [--limit 10] [--hivemind-dir DIR]

Options are forwarded to:
  hivemind query search_decisions

If no actor is supplied, the query defaults to the current agent session actor:
agent:<tool>:<session>.
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

first_nonempty() {
  local value
  for value in "$@"; do
    if [[ -n "$value" ]]; then
      printf '%s\n' "$value"
      return 0
    fi
  done
  return 1
}

detect_agent_tool() {
  if [[ -n "${CLAUDE_SESSION_ID:-}${CLAUDE_CODE_SESSION_ID:-}${CLAUDE_PROJECT_DIR:-}${CLAUDE_PLUGIN_ROOT:-}" ]]; then
    printf 'claude\n'
  elif [[ -n "${CODEX_THREAD_ID:-}${CODEX_SESSION_ID:-}${CODEX_TASK_ID:-}" ]]; then
    printf 'codex\n'
  else
    printf 'claude\n'
  fi
}

detect_agent_session() {
  case "$1" in
    codex)
      first_nonempty \
        "${CODEX_THREAD_ID:-}" \
        "${CODEX_SESSION_ID:-}" \
        "${CODEX_TASK_ID:-}" \
        "${GC_SESSION_ID:-}" \
        "${GC_SESSION_NAME:-}" \
        "manual-session"
      ;;
    claude)
      first_nonempty \
        "${CLAUDE_SESSION_ID:-}" \
        "${CLAUDE_CODE_SESSION_ID:-}" \
        "${GC_SESSION_ID:-}" \
        "${GC_SESSION_NAME:-}" \
        "manual-session"
      ;;
    *)
      first_nonempty \
        "${GC_SESSION_ID:-}" \
        "${GC_SESSION_NAME:-}" \
        "manual-session"
      ;;
  esac
}

WORKTREE_ROOT="$(worktree_root)"
PROJECT_ROOT="$(project_root)"
HIVEMIND_DIR="${HIVEMIND_DIR:-${CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR:-$PROJECT_ROOT/hivemind}}"
AGENT_TOOL=""
AGENT_SESSION=""
HAS_ACTOR=0
HAS_SOURCE=0
HAS_LIMIT=0
FORWARDED=()

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
    --actor)
      HAS_ACTOR=1
      FORWARDED+=(--actor-id "${2:-}")
      shift 2
      ;;
    --actor-id)
      HAS_ACTOR=1
      FORWARDED+=(--actor-id "${2:-}")
      shift 2
      ;;
    --source)
      HAS_SOURCE=1
      FORWARDED+=(--source "${2:-}")
      shift 2
      ;;
    --limit)
      HAS_LIMIT=1
      FORWARDED+=(--limit "${2:-}")
      shift 2
      ;;
    --agent-tool)
      AGENT_TOOL="${2:-}"
      shift 2
      ;;
    --agent-session)
      AGENT_SESSION="${2:-}"
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

if [[ -z "$AGENT_TOOL" ]]; then
  AGENT_TOOL="$(detect_agent_tool)"
fi
if [[ -z "$AGENT_SESSION" ]]; then
  AGENT_SESSION="$(detect_agent_session "$AGENT_TOOL")"
fi

if [[ "$HAS_ACTOR" -eq 0 ]]; then
  FORWARDED+=(--actor-id "agent:$AGENT_TOOL:$AGENT_SESSION")
fi
if [[ "$HAS_SOURCE" -eq 0 ]]; then
  FORWARDED+=(--source agent)
fi
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

exec "${BASE_CMD[@]}" --hivemind-dir "$HIVEMIND_DIR" query search_decisions \
  "${FORWARDED[@]}"
