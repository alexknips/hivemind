#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  recall.sh "what did we decide about bearer auth on postgres" [--topic t,...]
            [--status s,...] [--actor-id a,...] [--source agent|human]
            [--since T] [--until T] [--limit 5] [--cursor C] [--summary]

"What did we decide about X?" Free text, never an id. Runs:
  hivemind query recall
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

hivemind_context_exec query recall "$@"
