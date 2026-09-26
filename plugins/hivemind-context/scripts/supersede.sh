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
               [--project HANDLE] [--declare-topic KEY,...] [--old ID] [--pick N] [--topic T]

  supersede.sh '#1' --title "..." --rationale "..." --bet   # resolve candidate #1

The replacement must say what it rests on (a decision we already made,
something observed, something assumed, or a declared --bet); a supersede that
names nothing is refused and nothing is written. A '#N' given to
--rests-on-decision refers to the previous ambiguous candidate list.

The replacement's project is worked out from where this runs (the nearest
.hivemind-project file walking up from the working directory, then the project
anchored to the Gas City rig, then your `hivemind project use` setting);
--project HANDLE names it outright and wins. When none applies, the
replacement stays in the project of the decision it replaces. Under a
registered project the replacement may use only that project's declared topic
keys; --declare-topic KEY adds a new one (it must be one of --topic-keys).

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

# These verbs are resolved by a description, so no argument at all is a usage
# error. Say so here rather than reaching the CLI with an empty argument list.
if [[ $# -eq 0 ]]; then
  usage
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

hivemind_context_join_description "$@"
# ${arr[@]+...}: macOS bash 3.2 treats expanding an empty array under `set -u`
# as an unbound variable, and HC_ARGS is empty for an empty description.
hivemind_context_exec --write --project-from-context supersede ${HC_ARGS[@]+"${HC_ARGS[@]}"}
