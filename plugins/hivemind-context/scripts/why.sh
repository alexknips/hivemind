#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  why.sh "description of the decision" [--id ID] [--pick N] [--topic T]
         [--depth 1] [--relations r,...] [--compact] [--summary]

  why.sh '#2'   # resolve candidate #2 from the previous ambiguous output

Rationale and neighborhood for a decision, resolved by description (never
guess an id). If the description is ambiguous, the CLI returns a numbered
candidate list instead of picking one — re-invoke with --pick N or #N. Runs:
  hivemind query why   (alias of get_decision_neighborhood)
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

hivemind_context_exec query why "$@"
