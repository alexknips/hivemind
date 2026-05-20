#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  .claude/scripts/capture-decision.sh --title "..." --rationale "..." \
    --topic-keys topic[,topic] --options option[,option] [--chose option]

Options:
  --source human|agent       Provenance source. Defaults to human.
  --actor-id ID              Override actor id. Required by direct human CLI calls.
  --source-ref REF           Override source_ref. Defaults to actor id.
  --agent-session SESSION    Claude session id for --source agent.
  --hivemind-dir DIR         Ledger directory. Defaults to $HIVEMIND_DIR or ./hivemind.

Additional decision.capture flags such as --evidence and --hypotheses are
forwarded to the HiveMind CLI.
USAGE
}

repo_root() {
  git rev-parse --show-toplevel 2>/dev/null || pwd
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

PROJECT_ROOT="$(repo_root)"
HIVEMIND_DIR="${HIVEMIND_DIR:-$PROJECT_ROOT/hivemind}"
SOURCE="human"
ACTOR_ID=""
SOURCE_REF=""
AGENT_TOOL="claude"
AGENT_SESSION="${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-manual-session}}"
FORWARDED=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --source)
      SOURCE="${2:-}"
      shift 2
      ;;
    --actor-id)
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

case "$SOURCE" in
  human|agent) ;;
  *)
    echo "unsupported --source '$SOURCE' (expected human or agent)" >&2
    exit 2
    ;;
esac

mkdir -p "$HIVEMIND_DIR"

PROVENANCE=(--source "$SOURCE")
if [[ "$SOURCE" == "human" ]]; then
  ACTOR_ID="${ACTOR_ID:-$(human_actor_id)}"
  SOURCE_REF="${SOURCE_REF:-$ACTOR_ID}"
  PROVENANCE+=(--actor-id "$ACTOR_ID" --source-ref "$SOURCE_REF")
else
  PROVENANCE+=(--agent-tool "$AGENT_TOOL" --agent-session "$AGENT_SESSION")
  if [[ -n "$ACTOR_ID" ]]; then
    PROVENANCE+=(--actor-id "$ACTOR_ID")
  fi
  if [[ -n "$SOURCE_REF" ]]; then
    PROVENANCE+=(--source-ref "$SOURCE_REF")
  fi
fi

if [[ -n "${HIVEMIND_CAPTURE_BIN:-}" ]]; then
  BASE_CMD=("$HIVEMIND_CAPTURE_BIN")
elif [[ -x "$PROJECT_ROOT/target/debug/hivemind" ]]; then
  BASE_CMD=("$PROJECT_ROOT/target/debug/hivemind")
elif [[ -f "$PROJECT_ROOT/Cargo.toml" ]]; then
  BASE_CMD=(cargo run --quiet --manifest-path "$PROJECT_ROOT/Cargo.toml" --)
elif command -v hivemind >/dev/null 2>&1; then
  BASE_CMD=("$(command -v hivemind)")
else
  echo "could not find hivemind binary or Cargo.toml in $PROJECT_ROOT" >&2
  exit 127
fi

exec "${BASE_CMD[@]}" --hivemind-dir "$HIVEMIND_DIR" emit decision.capture \
  "${PROVENANCE[@]}" "${FORWARDED[@]}"
