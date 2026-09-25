#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  verify.sh "description of the decision" [--id ID] [--pick N] [--topic T] [--summary]

  verify.sh '#1'   # resolve candidate #1 from the previous ambiguous output

"Did that hold up?" Resolved by description (never guess an id). Leads with
the decision, rationale, rejected options, who decided, and whether it still
holds. If the description is ambiguous, the CLI returns a numbered candidate
list instead of picking one — re-invoke with --pick N or #N. Runs:
  hivemind query verify   (alias of get_decision_outcome)
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
hivemind_context_exec query verify ${HC_ARGS[@]+"${HC_ARGS[@]}"}
