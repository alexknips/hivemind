#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  plugins/hivemind-capture/scripts/install-active-capture-skill.sh [--project-dir DIR] [--dest-dir DIR]

Copies the Claude active-capture skill into a checkout's .claude/skills/
directory. --dest-dir overrides the full destination skills directory.
USAGE
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SOURCE="$PLUGIN_ROOT/.claude-plugin/skills/active-capture.md"
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-}"
DEST_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --project-dir)
      PROJECT_DIR="${2:-}"
      shift 2
      ;;
    --dest-dir)
      DEST_DIR="${2:-}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ ! -f "$SOURCE" ]]; then
  echo "missing active capture skill source: $SOURCE" >&2
  exit 1
fi

if [[ -z "$DEST_DIR" ]]; then
  if [[ -z "$PROJECT_DIR" ]]; then
    PROJECT_DIR="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  fi
  DEST_DIR="$PROJECT_DIR/.claude/skills"
fi

mkdir -p "$DEST_DIR"
install -m 0644 "$SOURCE" "$DEST_DIR/active-capture.md"
printf 'installed active capture skill to %s\n' "$DEST_DIR/active-capture.md"
