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


# Stable identity first: Gas City assigns every crew/polecat/refinery slot a
# fixed name (GC_AGENT, mirrored in GC_ALIAS) that survives process restarts.
# A raw session id (CLAUDE_SESSION_ID, CODEX_SESSION_ID, ...) does not -- it's
# freshly generated on every run, so checking it first makes the same physical
# agent appear as a different actor on every restart (hivemind-zdsh.9). Raw
# session ids stay as fallbacks for standalone use without gc.
detect_agent_session() {
  case "$1" in
    codex)
      first_nonempty \
        "${GC_AGENT:-}" \
        "${GC_ALIAS:-}" \
        "${CODEX_THREAD_ID:-}" \
        "${CODEX_SESSION_ID:-}" \
        "${CODEX_TASK_ID:-}" \
        "${GC_SESSION_ID:-}" \
        "${GC_SESSION_NAME:-}" \
        "manual-session"
      ;;
    claude)
      first_nonempty \
        "${GC_AGENT:-}" \
        "${GC_ALIAS:-}" \
        "${CLAUDE_SESSION_ID:-}" \
        "${CLAUDE_CODE_SESSION_ID:-}" \
        "${GC_SESSION_ID:-}" \
        "${GC_SESSION_NAME:-}" \
        "manual-session"
      ;;
    *)
      first_nonempty \
        "${GC_AGENT:-}" \
        "${GC_ALIAS:-}" \
        "${GC_SESSION_ID:-}" \
        "${GC_SESSION_NAME:-}" \
        "manual-session"
      ;;
  esac
}

# hivemind_context_join_description <args...>
# Sets HC_ARGS to a reconstructed argument list where a leading run of words
# that don't look like a flag (i.e. don't start with `-`) is joined with
# single spaces into ONE argument. The underlying CLI's free-text description
# positional is a single `Option<String>` (src/cli/args.rs) -- it accepts
# exactly one shell token, by design (hivemind-tenv.1). Every command's
# argument-hint documents the workaround: quote the description yourself,
# e.g. `"what did we decide about X" --topic t`. When a caller does that,
# the quoted text already arrives here as one word and this function is a
# no-op passthrough. But the equally natural, unprompted phrasing -- typing
# the free-text description as several bare shell words with no quotes at
# all -- used to reach the CLI as separate positionals and error on the
# second word (hivemind-tenv.3 reopen: an agent hit this on its first try).
# Joining here, in the plugin's own script layer, fixes that without
# touching the locked CLI contract or the command markdown's $ARGUMENTS
# substitution (which must stay unquoted -- wrapping the whole substituted
# blob in one more pair of quotes collides with a caller's own quotes around
# --flag values and breaks the documented quoted form instead of fixing the
# unquoted one; verified empirically before choosing this fix).
hivemind_context_join_description() {
  HC_ARGS=()
  local description="" collecting=1 word
  for word in "$@"; do
    if [[ "$collecting" == "1" && "$word" != -* ]]; then
      if [[ -z "$description" ]]; then
        description="$word"
      else
        description="$description $word"
      fi
    else
      collecting=0
      HC_ARGS+=("$word")
    fi
  done
  if [[ -n "$description" ]]; then
    # ${arr[@]+...}: macOS ships bash 3.2, where expanding an empty array under
    # `set -u` is an "unbound variable" error (a description with no flags
    # leaves HC_ARGS empty here).
    HC_ARGS=("$description" ${HC_ARGS[@]+"${HC_ARGS[@]}"})
  fi
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

# hivemind_context_exec [--write] [--project-from-context] <subcommand...>
# --write injects `--actor agent:<tool>:<name>` ahead of the subcommand so
# disagree/supersede record agent provenance in actor_id rather than falling
# back to the CLI's git-derived human default. Read verbs need no such
# override; they log the CLI's own default actor.
# --project-from-context, for the verb that records a new decision (supersede),
# hands the CLI the "work out the project from where this runs" switch. It is
# a flag of that subcommand, so it goes right after the subcommand name; the
# CLI owns the rule (nearest .hivemind-project, then the rig, then the current
# project), a --project the caller passes still wins, and a supersede that
# finds no project inherits the old decision's.
hivemind_context_exec() {
  hivemind_context_resolve
  log_hivemind_resolution

  local global_args=(--hivemind-dir "$HIVEMIND_DIR")
  local subcommand_args=()
  while [[ "${1:-}" == --write || "${1:-}" == --project-from-context ]]; do
    if [[ "$1" == "--write" ]]; then
      global_args+=(--actor "$AGENT_ACTOR")
    else
      subcommand_args+=(--project-from-context)
    fi
    shift
  done

  local subcommand="$1"
  shift
  exec "${BASE_CMD[@]}" "${global_args[@]}" "$subcommand" \
    ${subcommand_args[@]+"${subcommand_args[@]}"} "$@"
}
