#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  importance.sh [--limit 10] [--cursor C] [--include-not-in-force] [--summary]

Which decisions carry the most impact, most first, each with who decided it (a
person or an agent). A decision is ranked when other decisions follow from it;
the rest follow unranked and read `not_assessed`, never a score of zero.
Superseded and rejected decisions are left out (and counted) unless
--include-not-in-force. Runs:
  hivemind query rank_decisions_by_importance
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

# The marketplace serves this plugin from master while people install the latest release binary,
# so the verb can be newer than the CLI under it. Say so, instead of the CLI's bare usage error.
hivemind_context_resolve
if ! "${BASE_CMD[@]}" query rank_decisions_by_importance --help >/dev/null 2>&1; then
  printf 'the installed hivemind CLI (%s) has no `query rank_decisions_by_importance` command: this plugin is newer than that release. Update the CLI.\n' \
    "$("${BASE_CMD[@]}" --version 2>/dev/null || echo unknown)" >&2
  exit 2
fi

hivemind_context_exec query rank_decisions_by_importance "$@"
