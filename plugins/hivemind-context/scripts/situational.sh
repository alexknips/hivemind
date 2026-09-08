#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  situational.sh [--paths a,b] [--diff] [--branch] [--cwd]
                  [--since-offset N | --since-ts T | --since-branch-point [--base REF]]
                  [--limit 25] [--cursor C] [--summary]

"What should I know before I touch this?" Defaults to the current git diff /
staged set when no --paths/--diff/--branch/--cwd is given. Runs:
  hivemind query situational
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

hivemind_context_exec query situational "$@"
