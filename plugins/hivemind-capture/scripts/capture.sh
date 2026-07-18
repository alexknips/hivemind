#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  capture.sh "<text>" --kind decision --title "..." --rationale "..." \
    --topic-keys topic[,topic] --options option[,option] [--chose option]
  capture.sh "<text>" --kind evidence
  capture.sh "<text>" --kind hypothesis
  capture.sh "<text>"

Options:
  --kind KIND              decision, evidence, hypothesis, blocker,
                           decision-request, or notification. When omitted,
                           the configured classifier chooses a kind.
  --source human|agent     Provenance source. Defaults to agent.
  --actor-id ID            Override actor id.
  --source-ref REF         Override source_ref. Defaults to actor id.
  --agent-tool TOOL        Agent tool name. Defaults from session context.
  --agent-session SESSION  Agent session id. Defaults from session context.
  --hivemind-dir DIR       Ledger directory. Defaults to plugin config,
                           $HIVEMIND_DIR, or <project>/hivemind.

Decision captures forward decision.capture flags such as --title, --rationale,
--topic-keys, --options, --chose, --evidence, and --hypotheses.
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

human_actor_id() {
  local raw
  raw="$(git config user.email 2>/dev/null || true)"
  if [[ -z "$raw" ]]; then
    raw="$(git config user.name 2>/dev/null || true)"
  fi
  if [[ -z "$raw" ]]; then
    raw="$(id -un)"
  fi
  raw="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9_.@-' '-')"
  raw="${raw#-}"
  raw="${raw%-}"
  if [[ -z "$raw" ]]; then
    raw="local-user"
  fi
  printf 'human:%s' "$raw"
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

install_hint() {
  cat >&2 <<'HINT'
HiveMind CLI was not found.

Install it first, then retry:
  cargo install --path /path/to/hivemind

Or set HIVEMIND_CAPTURE_BIN to a built hivemind binary.
HINT
}

append_text() {
  local next="$1"
  if [[ -z "$TEXT" ]]; then
    TEXT="$next"
  else
    TEXT="$TEXT $next"
  fi
}

provenance_args() {
  printf '%s\0' --source "$SOURCE"
  if [[ "$SOURCE" == "human" ]]; then
    printf '%s\0' --actor-id "$ACTOR_ID" --source-ref "$SOURCE_REF"
  else
    printf '%s\0' --agent-tool "$AGENT_TOOL" --agent-session "$AGENT_SESSION"
    printf '%s\0' --actor-id "$ACTOR_ID" --source-ref "$SOURCE_REF"
  fi
}

classifier_schema() {
  cat <<'JSON'
{"type":"object","properties":{"kind":{"type":"string","enum":["decision","evidence","hypothesis","blocker","decision-request","notification","none"]}},"required":["kind"],"additionalProperties":true}
JSON
}

run_classifier() {
  local input="$1"

  if [[ -n "${HIVEMIND_CAPTURE_CLASSIFIER_JSON:-}" ]]; then
    printf '%s\n' "$HIVEMIND_CAPTURE_CLASSIFIER_JSON"
    return 0
  fi

  if [[ -n "${HIVEMIND_CAPTURE_CLASSIFIER_CMD:-}" ]]; then
    printf '%s\n' "$input" | sh -c "$HIVEMIND_CAPTURE_CLASSIFIER_CMD"
    return $?
  fi

  if command -v claude >/dev/null 2>&1; then
    local prompt
    prompt="$(printf 'Classify this HiveMind capture text. Return JSON only, with kind set to one of decision, evidence, hypothesis, blocker, decision-request, notification, or none.\n\nText:\n%s\n' "$input")"
    local claude_cmd=(claude --print --no-session-persistence --model "${HIVEMIND_CLASSIFIER_MODEL:-haiku}" --json-schema "$(classifier_schema)")
    if [[ -n "${CLAUDE_PLUGIN_ROOT:-}" ]]; then
      claude_cmd+=(--plugin-dir "$CLAUDE_PLUGIN_ROOT")
    fi
    "${claude_cmd[@]}" "$prompt"
    return $?
  fi

  return 127
}

classifier_kind() {
  local output="$1"
  printf '%s' "$output" | python3 -c '
import json
import sys

data = json.load(sys.stdin)
if isinstance(data, dict) and isinstance(data.get("captures"), list):
    captures = data["captures"]
    data = captures[0] if captures else {"kind": "none"}
if not isinstance(data, dict):
    raise SystemExit("classifier output must be a JSON object")
kind = data.get("kind")
if not isinstance(kind, str):
    raise SystemExit("classifier output missing string kind")
print(kind)
'
}

resolve_kind() {
  if [[ -n "$KIND" ]]; then
    return 0
  fi

  local classifier_output
  if ! classifier_output="$(run_classifier "$TEXT")"; then
    printf 'No HiveMind capture emitted: --kind was omitted and the classifier is unavailable.\n' >&2
    exit 0
  fi

  if ! KIND="$(classifier_kind "$classifier_output")"; then
    printf 'No HiveMind capture emitted: classifier returned non-JSON output.\n' >&2
    exit 0
  fi

  if [[ "$KIND" == "none" ]]; then
    printf 'No HiveMind capture emitted: classifier returned kind=none.\n'
    exit 0
  fi
}

emit_decision() {
  local result
  if [[ "${#FORWARDED[@]}" -eq 0 ]]; then
    cat >&2 <<'ERROR'
decision captures require structured decision.capture flags:
  --title, --rationale, --topic-keys, and --options
ERROR
    exit 2
  fi
  result="$("${BASE_CMD[@]}" --hivemind-dir "$HIVEMIND_DIR" emit decision.capture \
    "${PROVENANCE[@]}" "${FORWARDED[@]}")"
  printf 'Captured HiveMind decision %s in %s.\n' "$result" "$HIVEMIND_DIR"
}

emit_evidence() {
  local result
  if [[ -z "$TEXT" ]]; then
    echo "evidence captures require non-empty text" >&2
    exit 2
  fi
  result="$("${BASE_CMD[@]}" --hivemind-dir "$HIVEMIND_DIR" emit evidence.recorded \
    "${PROVENANCE[@]}" --content "$TEXT")"
  printf 'Captured HiveMind evidence %s in %s.\n' "$result" "$HIVEMIND_DIR"
}

emit_hypothesis() {
  local result
  if [[ -z "$TEXT" ]]; then
    echo "hypothesis captures require non-empty text" >&2
    exit 2
  fi
  result="$("${BASE_CMD[@]}" --hivemind-dir "$HIVEMIND_DIR" emit hypothesis.recorded \
    "${PROVENANCE[@]}" --statement "$TEXT")"
  printf 'Captured HiveMind hypothesis %s in %s.\n' "$result" "$HIVEMIND_DIR"
}

WORKTREE_ROOT="$(worktree_root)"
PROJECT_ROOT="$(project_root)"
HIVEMIND_DIR="${HIVEMIND_DIR:-${CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR:-$PROJECT_ROOT/hivemind}}"
SOURCE="agent"
KIND=""
TEXT=""
ACTOR_ID=""
SOURCE_REF=""
AGENT_TOOL=""
AGENT_SESSION=""
FORWARDED=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --kind)
      KIND="${2:-}"
      shift 2
      ;;
    --source)
      SOURCE="${2:-}"
      shift 2
      ;;
    --actor-id|--actor)
      ACTOR_ID="${2:-}"
      shift 2
      ;;
    --source-ref)
      SOURCE_REF="${2:-}"
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
    --hivemind-dir)
      HIVEMIND_DIR="${2:-}"
      shift 2
      ;;
    --title|--rationale|--topic-keys|--options|--chose|--hypotheses|--evidence)
      FORWARDED+=("$1" "${2:-}")
      shift 2
      ;;
    --)
      shift
      while [[ $# -gt 0 ]]; do
        append_text "$1"
        shift
      done
      ;;
    -*)
      FORWARDED+=("$1")
      shift
      ;;
    *)
      append_text "$1"
      shift
      ;;
  esac
done

case "$SOURCE" in
  human|agent) ;;
  *)
    echo "unsupported --source '$SOURCE' (expected human or agent)" >&2
    exit 2
    ;;
esac

log_hivemind_resolution
mkdir -p "$HIVEMIND_DIR"

if [[ -z "$AGENT_TOOL" ]]; then
  AGENT_TOOL="$(detect_agent_tool)"
fi
if [[ -z "$AGENT_SESSION" ]]; then
  AGENT_SESSION="$(detect_agent_session "$AGENT_TOOL")"
fi

if [[ "$SOURCE" == "human" ]]; then
  ACTOR_ID="${ACTOR_ID:-$(human_actor_id)}"
  SOURCE_REF="${SOURCE_REF:-$ACTOR_ID}"
else
  ACTOR_ID="${ACTOR_ID:-agent:$AGENT_TOOL:$AGENT_SESSION}"
  SOURCE_REF="${SOURCE_REF:-$ACTOR_ID}"
fi

PROVENANCE=()
while IFS= read -r -d '' value; do
  PROVENANCE+=("$value")
done < <(provenance_args)

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

resolve_kind

case "$KIND" in
  decision)
    emit_decision
    ;;
  evidence)
    emit_evidence
    ;;
  hypothesis)
    emit_hypothesis
    ;;
  blocker|decision-request|notification)
    printf 'kind %s is known but does not yet have a canonical CLI capture path.\n' "$KIND" >&2
    exit 2
    ;;
  *)
    printf 'unsupported --kind %s (expected decision, evidence, hypothesis, blocker, decision-request, or notification)\n' "$KIND" >&2
    exit 2
    ;;
esac

printf 'Query it with: /hivemind-capture:query-decisions --actor-id %s --source %s\n' \
  "$ACTOR_ID" "$SOURCE"
