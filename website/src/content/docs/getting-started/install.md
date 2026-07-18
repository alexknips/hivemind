---
title: Self-host
description: Run your own HiveMind cell — three commands, your data, your infrastructure.
---

Your instance, your data, nothing phones home. A self-hosted cell takes about five minutes on any machine with Docker.

---

## Start in three commands

**Prerequisites:** Docker 24+ with Compose v2 (`docker compose version`). Port 8080 must be available.

```bash
# 1. Fetch the compose cell
git clone https://github.com/alexknips/hivemind && cd hivemind

# 2. Create your config and set strong secrets
cp .env.example .env
# Linux:
sed -i "s/change-me-before-production/$(openssl rand -hex 32)/g" .env
# macOS: sed -i '' "s/change-me-before-production/$(openssl rand -hex 32)/g" .env

# 3. Start (builds the image on first run, ~5 minutes)
docker compose up --build -d
```

Confirm the cell is healthy:

```bash
docker compose ps
```

```
NAME        STATUS                   PORTS
hivemind    Up (healthy)             0.0.0.0:8080->8080/tcp
postgres    Up (healthy)
```

---

## Connect your agent

Provision a tenant to get your bearer token (shown once — save it):

```bash
ADMIN_KEY=$(grep HIVEMIND_ADMIN_KEY .env | cut -d= -f2)
curl -s -X POST http://localhost:8080/v1/tenants \
  -H "Authorization: Bearer $ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id": "me", "display_name": "My Team"}'
```

The response includes `token_secret` — a value starting with `hm_tk_...`. Copy it now.

Add HiveMind to Claude Code by pasting this into `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "hivemind": {
      "url": "http://localhost:8080/mcp",
      "headers": {
        "Authorization": "Bearer hm_tk_..."
      }
    }
  }
}
```

Reload Claude Code. The HiveMind tools appear in your agent's tool list. Your instance is running.

:::note
**Your data stays on your machine.** The self-hosted cell never communicates with
the managed hosted service. Separate deployments, separate data.
:::

---

## Local CLI / development

For single-user local use or development without Docker.

**Installer script** (places `hivemind` in `~/.local/bin`):

```bash
curl -fsSL https://raw.githubusercontent.com/alexknips/hivemind/master/scripts/install.sh | sh
```

**Build from source:**

```bash
cargo install --git https://github.com/alexknips/hivemind --locked hivemind
```

Run a local MCP server over stdio — no HTTP, no auth required:

```bash
hivemind --hivemind-dir ./hivemind/ mcp
```

Add to Claude Code via `.mcp.json`:

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "hivemind",
      "args": ["mcp"],
      "env": { "HIVEMIND_DIR": "./hivemind/" }
    }
  }
}
```

Run the HTTP API server directly:

```bash
HIVEMIND_DIR=./hivemind hivemind serve --port 8080
```

Set `HIVEMIND_API_KEY` to require bearer-token authentication.

---

## Going deeper

The [full self-hosting runbook](https://github.com/alexknips/hivemind/blob/master/docs/SELF_HOSTING.md)
covers production configuration, TLS setup, E2E verification, upgrading, and troubleshooting.

- [MCP Setup](/guides/mcp-setup/) — agent configuration for all MCP clients
- [Quickstart](/getting-started/quickstart/) — capture your first decision in one command
