#!/usr/bin/env sh
# Install the HiveMind MCP gateway into the current project's .mcp.json.
#
# Usage:
#   HIVEMIND_URL=https://your-server \
#   HIVEMIND_API_KEY=hm_sk_live_... \
#   ./scripts/install-mcp.sh
#
# Optional env vars:
#   HIVEMIND_MCP_TARGET   Path to the .mcp.json to write (default: ./.mcp.json)
#   HIVEMIND_GATEWAY_DIR  Path to the clients/mcp-gateway directory (default: auto-detected)
set -eu

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
gateway_dir="${HIVEMIND_GATEWAY_DIR:-$repo_root/clients/mcp-gateway}"
target="${HIVEMIND_MCP_TARGET:-.mcp.json}"

# Validate required env vars
: "${HIVEMIND_URL:?HIVEMIND_URL is required (e.g. https://your-server)}"
: "${HIVEMIND_API_KEY:?HIVEMIND_API_KEY is required (e.g. hm_sk_live_...)}"

# Build the gateway if dist/index.js is absent
dist="$gateway_dir/dist/index.js"
if [ ! -f "$dist" ]; then
  echo "Building MCP gateway..."
  (cd "$gateway_dir" && npm install && npm run build)
fi

# Emit the config snippet to merge into .mcp.json
entry="$(cat <<JSON
{
  "command": "node",
  "args": ["$dist"],
  "env": {
    "HIVEMIND_URL": "$HIVEMIND_URL",
    "HIVEMIND_API_KEY": "$HIVEMIND_API_KEY"
  }
}
JSON
)"

# Upsert the "hivemind" key in .mcp.json using node (always available alongside npm)
node - <<NODEEOF
const fs = require('fs');
const path = require('path');

const target = '$target';
let config = {};
try {
  config = JSON.parse(fs.readFileSync(target, 'utf8'));
} catch (_) {}

config.mcpServers = config.mcpServers ?? {};
config.mcpServers.hivemind = $entry;

fs.writeFileSync(target, JSON.stringify(config, null, 2) + '\n');
console.log('Wrote hivemind MCP gateway entry to ' + path.resolve(target));
NODEEOF

echo ""
echo "Done. To use in Claude Code, reload or restart your session."
echo ""
echo "Server: $HIVEMIND_URL"
echo "Key: ${HIVEMIND_API_KEY:0:12}..."
