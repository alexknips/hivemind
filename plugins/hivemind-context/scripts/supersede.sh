#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  supersede.sh "description of the old decision" \
               --title "new decision title" --rationale "why the new direction" \
               [--topic-keys t,...] [--options o,...] [--chose o] \
               [--hypotheses h,...] [--evidence e,...] \
               (--rests-on-decision "..." | --rests-on-evidence "..." \
                --evidence-source "..." | --rests-on-assumption "..." | --bet ["..."]) \
               [--would-change-if "..."] [--check-by DATE] [--confidence low|medium|high] \
               [--old ID] [--pick N] [--topic T]

  supersede.sh '#1' --title "..." --rationale "..." --bet   # resolve candidate #1

The replacement must say what it rests on (a decision we already made,
something observed, something assumed, or a declared --bet); a supersede that
names nothing is refused and nothing is written. A '#N' given to
--rests-on-decision refers to the previous ambiguous candidate list.

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

hivemind_context_join_description "$@"
hivemind_context_exec --write supersede "${HC_ARGS[@]}"
