#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  ground.sh "description of the decision" \
            (--rests-on-decision "..." | --rests-on-evidence "..." \
             --evidence-source "..." | --rests-on-assumption "..." | --bet ["..."]) \
            [--would-change-if "..."] [--check-by DATE] \
            [--evidence e,...] [--hypotheses h,...] \
            [--id ID] [--pick N] [--topic T]

  ground.sh '#1' --rests-on-decision "..."   # resolve candidate #1 as the decision

"This decision rests on nothing declared? Add it." Say what an existing
decision rests on, after the fact: a decision it follows from, something
observed (and where), something assumed, or a declared bet. The grounding is
append-only and attributed to you, not to whoever captured the decision.

Resolved by description, never by guessing an id. This is a WRITE verb: the
ambiguity gate is strict. If the description does not resolve to exactly one
decision, or a --rests-on-decision matches more than one, the CLI returns the
candidate list and performs NO write — re-invoke with --pick N (or --id <id>,
or '#N' for a premise) once you have picked the right one; never guess. A
premise that would close a loop (it already rests on this decision) is refused.
--confidence is not accepted: it is the decider's own words at capture. Runs:
  hivemind ground
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
# ${arr[@]+...}: no arguments at all leaves HC_ARGS empty, and macOS bash 3.2
# treats expanding an empty array under `set -u` as an unbound variable, which
# would hide the CLI's own "nothing to ground" refusal.
hivemind_context_exec --write ground ${HC_ARGS[@]+"${HC_ARGS[@]}"}
