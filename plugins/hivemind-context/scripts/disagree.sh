#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  disagree.sh "description of the decision" --reason "why you disagree"
              [--decision ID] [--pick N] [--topic T]

  disagree.sh '#2' --reason "..."   # resolve candidate #2 from a prior ambiguous output

Push back on a decision by description, never by guessing an id. This is a
WRITE verb: the ambiguity gate is strict here. If the description does not
resolve to exactly one decision, the CLI returns the candidate list and
performs NO write — re-invoke with --pick N (or --decision <id>) once you
have picked the right one; never guess. Runs:
  hivemind disagree
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

hivemind_context_exec --write disagree "$@"
