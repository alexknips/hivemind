#!/usr/bin/env bash
# check-classify-queue.sh — Session hook script for HiveMind classify-queue.
#
# Add to .claude/settings.json to be notified when unclassified batches are pending:
#
#   {
#     "hooks": {
#       "Stop": [{
#         "matcher": "",
#         "command": "hivemind classify-queue list --json --limit 1 2>/dev/null | jq -e '(.pending_total? // ((.batches? // .) | length)) > 0' >/dev/null && echo '[hivemind] classify-queue: batches pending — run /classify-queue to drain'"
#       }]
#     }
#   }
#
# Or use this script as the hook command:
#
#   "command": "/path/to/plugins/hivemind-capture/scripts/check-classify-queue.sh"

set -euo pipefail

HIVEMIND_DIR="${HIVEMIND_DIR:-./hivemind/}"

list() {
  hivemind --hivemind-dir "$HIVEMIND_DIR" classify-queue list --json --limit "$1" 2>/dev/null
}

# Builds after the v0.7.0 release reply {batches, pending_total, truncated, budget} on a local
# ledger and a served cell alike, and `pending_total` is the whole queue's depth whatever the
# limit, so --limit 1 keeps the check cheap. The v0.7.0 release prints a bare list of batches
# for a local ledger and {batches, budget} (no pending_total) from a server: with no
# pending_total the batches are counted instead, asked for up to 1000.
total=$(list 1 | command -p jq -r '.pending_total? // empty' 2>/dev/null || true)
if [ -z "$total" ]; then
  total=$(list 1000 | command -p jq -r '(.batches? // .) | length' 2>/dev/null || echo 0)
fi

if [ "${total:-0}" -gt 0 ]; then
  echo "[hivemind] classify-queue: $total batch(es) pending — run /classify-queue to drain"
fi
