#!/usr/bin/env bash
# check-classify-queue.sh — Session hook script for HiveMind classify-queue.
#
# Add to .claude/settings.json to be notified when unclassified batches are pending:
#
#   {
#     "hooks": {
#       "Stop": [{
#         "matcher": "",
#         "command": "hivemind classify-queue list --json 2>/dev/null | jq -e 'length > 0' >/dev/null && echo '[hivemind] classify-queue: batches pending — run /classify-queue to drain'"
#       }]
#     }
#   }
#
# Or use this script as the hook command:
#
#   "command": "/path/to/plugins/hivemind-capture/scripts/check-classify-queue.sh"

set -euo pipefail

HIVEMIND_DIR="${HIVEMIND_DIR:-./hivemind/}"

count=$(hivemind --hivemind-dir "$HIVEMIND_DIR" classify-queue list --json 2>/dev/null \
  | command -p jq 'length' 2>/dev/null || echo 0)

if [ "${count:-0}" -gt 0 ]; then
  echo "[hivemind] classify-queue: $count batch(es) pending — run /classify-queue to drain"
fi
