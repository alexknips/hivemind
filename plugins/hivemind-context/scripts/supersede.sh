#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  supersede.sh "description of the old decision" \
               --title "new decision title" --rationale "why the new direction" \
               [--topic-keys t,...] [--options o,...] [--chose o] \
               [--hypotheses h,...] [--evidence e,...] \
               [--old ID] [--pick N] [--topic T]

  supersede.sh '#1' --title "..." --rationale "..."   # resolve candidate #1

Replace an existing decision by description, never by guessing an id. This
is a WRITE verb: the ambiguity gate is strict here. If the description does
not resolve to exactly one decision, the CLI returns the candidate list and
performs NO write — re-invoke with --pick N (or --old <id>) once you have
picked the right one; never guess. Runs:
  hivemind supersede
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

hivemind_context_exec --write supersede "$@"
