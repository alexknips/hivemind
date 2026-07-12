# HiveMind Self-Hosting Guide

This guide covers deploying a self-hosted HiveMind cell — the same server
binary and Postgres backend used by the hosted service — on infrastructure
you control. One `docker compose up` command brings up the full stack.

---

## Prerequisites

- Docker 24+ and Docker Compose v2 (`docker compose version`)
- Port 8080 available on the host (or set `HIVEMIND_PORT`)

No other dependencies: Postgres runs as a companion container; the HiveMind
image bundles the SPA.

---

## Quick start

```bash
git clone https://github.com/alexknips/hivemind
cd hivemind

# 1. Create your environment file
cp .env.example .env

# 2. Set a strong admin key and Postgres password
#    (these are the only required secrets)
sed -i "s/change-me-before-production/$(openssl rand -hex 32)/g" .env

# 3. Start the cell (builds the image on first run, ~5 minutes)
docker compose up --build -d

# 4. Wait for healthy status
docker compose ps
```

The server is ready when `hivemind` shows `healthy`:

```
NAME        STATUS                   PORTS
hivemind    Up (healthy)             0.0.0.0:8080->8080/tcp
postgres    Up (healthy)
```

Verify:

```bash
curl http://localhost:8080/v1/health
# {"status":"ok"}
```

---

## Auth story (no WorkOS required)

Self-hosted cells do not need WorkOS. The server supports two auth modes
depending on whether `HIVEMIND_DATABASE_URL` is set:

### Postgres mode (default in this compose)

When `HIVEMIND_DATABASE_URL` is set, the server uses per-tenant bearer tokens
issued by the provisioning endpoint. There is no single shared API key.

**Provision your first tenant:**

```bash
# Replace <ADMIN_KEY> with the value of HIVEMIND_ADMIN_KEY in your .env
curl -s -X POST http://localhost:8080/v1/tenants \
  -H "Authorization: Bearer <ADMIN_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id": "acme", "display_name": "Acme Corp"}' | tee /tmp/tenant.json
```

Response:

```json
{
  "tenant_id": "acme",
  "token_id": "tok_...",
  "token_secret": "hm_sk_live_..."
}
```

Save `token_secret` — it is shown only once. Use it as the bearer token for
all subsequent requests from this tenant. The server never stores the raw
secret, only its hash.

**Use the token:**

```bash
export HM_TOKEN="hm_sk_live_..."   # token_secret from above

curl http://localhost:8080/v1/decisions/search?q=architecture \
  -H "Authorization: Bearer $HM_TOKEN"
```

### SQLite mode (no Postgres, single user)

If you remove the `HIVEMIND_DATABASE_URL` line from `docker-compose.yml` the
server falls back to SQLite stored in the `hivemind-data` volume. Auth becomes
a single static bearer token:

```bash
# In .env:
HIVEMIND_API_KEY=your-secret-key
```

When both `HIVEMIND_API_KEY` and `HIVEMIND_DATABASE_URL` are unset the server
starts in **development mode** — all requests are accepted without a token.
Only use this on a trusted private network.

---

## Configuration reference

All variables are read at startup. Compose reads them from `.env` in the
project root, or from the shell environment.

| Variable | Default | Description |
|---|---|---|
| `HIVEMIND_DATABASE_URL` | *(unset)* | Postgres connection string. When set enables the multi-tenant Postgres backend. Unset = SQLite at `HIVEMIND_DIR`. |
| `HIVEMIND_DIR` | `/data` | Directory for the SQLite ledger (SQLite mode only). Mount a volume here. |
| `HIVEMIND_PORT` | `8080` | Port the HTTP API listens on inside the container. |
| `HIVEMIND_ADMIN_KEY` | *(unset)* | Bearer token for `POST /v1/tenants` (Postgres mode). Required before provisioning tenants. |
| `HIVEMIND_API_KEY` | *(unset)* | Static bearer token (SQLite mode only). Omit for development/trusted-network mode. |
| `ANTHROPIC_API_KEY` | *(unset)* | Enables the Layer-3 ingest classifier (Claude Haiku). Optional. |
| `HIVEMIND_CORS_ORIGINS` | *(unset)* | Comma-separated origins allowed for browser cross-origin requests. |
| `POSTGRES_PASSWORD` | `hivemind` | Password for the bundled Postgres service. Change before production. |
| `HIVEMIND_TENANT` | `local` | Default tenant for CLI usage (not used in Postgres mode). |

**WorkOS variables** (`WORKOS_DOMAIN`, `WORKOS_JWKS_URL`, `WORKOS_AUDIENCE`) enable
OIDC browser login. Self-hosted cells typically omit these; the token-based auth
above is the supported self-host path. If you do need OIDC, leave connector
config (`CONNECTOR_*`) as a placeholder — that surface is not yet finalised
(see hivemind-ld68).

---

## E2E verification

Run these checks after provisioning your first tenant to confirm all layers work.

```bash
export HM_URL=http://localhost:8080
export HM_TOKEN="hm_sk_live_..."   # your tenant token

# 1. Health
curl -s $HM_URL/v1/health
# → {"status":"ok"}

# 2. Capture a decision
curl -s -X POST $HM_URL/v1/decisions \
  -H "Authorization: Bearer $HM_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Use Postgres for shared-backend storage",
    "rationale": "WAL mode does not scale across writers.",
    "actor_id": "human:alice",
    "chosen_option_label": "postgres",
    "options": [
      {"label": "postgres", "description": "r2d2 connection pool"},
      {"label": "sqlite",   "description": "WAL mode, single writer"}
    ]
  }' | tee /tmp/decision.json
# → {"id":"dec_...","status":"proposed",...}

# 3. Query it back
# Note: full-text search (GET /v1/decisions/search) is SQLite mode only.
# In Postgres mode, query by topic:
curl -s "$HM_URL/v1/decisions/relevant?topic=infrastructure" \
  -H "Authorization: Bearer $HM_TOKEN" | python3 -m json.tool | head -20

# 4. SPA reachable
curl -s -o /dev/null -w "%{http_code}" $HM_URL/
# → 200

# 5. MCP-over-HTTP endpoint reachable
curl -s -o /dev/null -w "%{http_code}" -X POST $HM_URL/mcp \
  -H "Authorization: Bearer $HM_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}'
# → 200
```

**Layer-3 classifier** (if `ANTHROPIC_API_KEY` is set): after step 2, wait a
few seconds then re-fetch the decision — `topic_keys` should be populated
automatically.

---

## Configuring an agent to use MCP-over-HTTP

Point your Claude Code (or any MCP-compatible agent) at the `/mcp` endpoint:

```json
{
  "mcpServers": {
    "hivemind": {
      "url": "http://localhost:8080/mcp",
      "headers": {
        "Authorization": "Bearer hm_sk_live_..."
      }
    }
  }
}
```

Or using the `hivemind mcp` CLI against the remote server:

```bash
hivemind mcp \
  --remote http://localhost:8080 \
  --token-env HM_TOKEN \
  --tenant acme \
  --agent-tool claude \
  --session-id my-session
```

---

## Production checklist

- [ ] Set `HIVEMIND_ADMIN_KEY` to a strong random value (`openssl rand -hex 32`)
- [ ] Set `POSTGRES_PASSWORD` to a strong random value
- [ ] Place a TLS-terminating reverse proxy (Caddy, nginx, Traefik) in front
      of port 8080 — the server speaks plain HTTP
- [ ] Back up the `postgres-data` Docker volume regularly
- [ ] Monitor `GET /v1/health` with an external uptime checker
- [ ] Set `restart: always` in compose for unattended recovery

---

## Upgrading

```bash
git pull
docker compose build --no-cache
docker compose up -d
```

Postgres schema migrations run automatically at startup.

---

## Troubleshooting

**`hivemind` container exits immediately**: check logs with
`docker compose logs hivemind`. Common cause: Postgres not yet healthy when
the server starts (the `depends_on: condition: service_healthy` guard handles
this; if it races, restart with `docker compose restart hivemind`).

**`POST /v1/tenants` returns 500 "HIVEMIND_ADMIN_KEY not configured"**: set
`HIVEMIND_ADMIN_KEY` in your `.env` and restart the stack.

**Auth errors on every request in Postgres mode**: you must provision a tenant
first and use its `token_secret` as the bearer. The static `HIVEMIND_API_KEY`
variable is ignored in Postgres mode.
