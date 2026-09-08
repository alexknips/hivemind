# Shared helpers for hivemind-context's thin CLI wrapper scripts.
# Source this file; do not execute it directly.
#
# Resolution matches plugins/hivemind-capture/scripts/query-decisions.sh exactly
# (same HIVEMIND_CAPTURE_BIN override, same hivemind_dir userConfig, same local/
# shared ledger fallback) so both plugins point at one binary-override knob.

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

# Sets WORKTREE_ROOT, PROJECT_ROOT, HIVEMIND_DIR, AGENT_TOOL, AGENT_SESSION,
# AGENT_ACTOR, and BASE_CMD (array) for the calling script.
hivemind_context_resolve() {
  WORKTREE_ROOT="$(worktree_root)"
  PROJECT_ROOT="$(project_root)"
  HIVEMIND_DIR="${HIVEMIND_DIR:-${CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR:-$PROJECT_ROOT/hivemind}}"

  AGENT_TOOL="$(detect_agent_tool)"
  AGENT_SESSION="$(detect_agent_session "$AGENT_TOOL")"
  AGENT_ACTOR="agent:$AGENT_TOOL:$AGENT_SESSION"

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
}

# hivemind_context_exec [--write] <subcommand...>
# --write injects `--actor agent:<tool>:<session>` ahead of the subcommand so
# disagree/supersede record agent provenance in actor_id rather than falling
# back to the CLI's git-derived human default. Read verbs need no such
# override; they log the CLI's own default actor.
hivemind_context_exec() {
  hivemind_context_resolve
  log_hivemind_resolution

  local global_args=(--hivemind-dir "$HIVEMIND_DIR")
  if [[ "${1:-}" == "--write" ]]; then
    global_args+=(--actor "$AGENT_ACTOR")
    shift
  fi

  exec "${BASE_CMD[@]}" "${global_args[@]}" "$@"
}
