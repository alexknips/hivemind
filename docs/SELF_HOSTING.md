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

> **Search note:** The default Postgres compose uses `GET /v1/decisions/relevant?topic=<topic>`
> for querying. Full-text search (`GET /v1/decisions/search`) is only available in SQLite
> mode. Switch to `HIVEMIND_DATABASE_URL=` (empty/unset) for a single-user local setup
> with full-text search.

---

## Auth story (no WorkOS required)

Self-hosted cells do not need WorkOS. A single cell can serve multiple users —
each user gets their own bearer token, and every captured decision is attributed
to that user's identity. The admin provisions users via the API; users connect
their agents with their personal token.

The server supports two auth modes depending on whether `HIVEMIND_DATABASE_URL`
is set:

### Postgres mode (default in this compose) — multi-user quickstart

When `HIVEMIND_DATABASE_URL` is set, the server uses per-user bearer tokens.

**Step 1 — provision your first tenant (one-time):**

```bash
# Replace <ADMIN_KEY> with the value of HIVEMIND_ADMIN_KEY in your .env
curl -s -X POST http://localhost:8080/v1/tenants \
  -H "Authorization: Bearer <ADMIN_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id": "myorg", "display_name": "My Org"}' | tee /tmp/tenant.json
```

**Step 2 — add users:**

Each team member gets their own token. The `actor_id` recorded in every
decision they capture is derived from their email at token-creation time and
cannot be overridden by the caller.

```bash
# Add Alice (admin)
curl -s -X POST http://localhost:8080/v1/users \
  -H "Authorization: Bearer <ADMIN_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"email": "alice@example.com", "display_name": "Alice", "role": "admin"}' \
  | tee /tmp/alice.json

# Add Bob (member)
curl -s -X POST http://localhost:8080/v1/users \
  -H "Authorization: Bearer <ADMIN_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"email": "bob@example.com", "display_name": "Bob", "role": "member"}' \
  | tee /tmp/bob.json
```

Response for each user:

```json
{
  "user_id":      "3fa85f64-...",
  "email":        "alice@example.com",
  "display_name": "Alice",
  "role":         "admin",
  "token_id":     "8d3f...",
  "token_secret": "hm_tk_<64-hex>"
}
```

`token_secret` is shown **once only** — save it immediately. The server stores
only its hash.

**Step 3 — each user connects their agent:**

```json
{
  "mcpServers": {
    "hivemind": {
      "url": "http://localhost:8080/mcp",
      "headers": {
        "Authorization": "Bearer hm_tk_<Alice's token>"
      }
    }
  }
}
```

Decisions captured by Alice are attributed to `human:alice@example.com`; Bob's
to `human:bob@example.com`. Actor identity is locked to the token — the server
ignores any `X-HiveMind-Actor` header.

**Managing tokens:**

```bash
# List all users
curl -s http://localhost:8080/v1/users \
  -H "Authorization: Bearer <ADMIN_KEY>"

# Mint an additional token for an existing user (e.g. new device)
curl -s -X POST "http://localhost:8080/v1/users/<USER_ID>/tokens?label=laptop" \
  -H "Authorization: Bearer <ADMIN_KEY>"

# Revoke a token (e.g. lost device)
curl -s -X DELETE "http://localhost:8080/v1/users/<USER_ID>/tokens/<TOKEN_ID>" \
  -H "Authorization: Bearer <ADMIN_KEY>"
```

### SQLite mode (no Postgres)

If you remove the `HIVEMIND_DATABASE_URL` line from `docker-compose.yml` the
server falls back to SQLite stored in the `hivemind-data` volume. With
`HIVEMIND_ADMIN_KEY` set, the same `POST /v1/users` endpoint works for
per-user SQLite tokens (default tenant `local`; omit `tenant_id` in body).

For a single static key instead:

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
| `HIVEMIND_ADMIN_KEY` | *(unset)* | Bearer token for `POST /v1/tenants`, `POST /v1/users`, `GET /v1/users`, and token revocation. Required before provisioning tenants or users. |
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
export HM_TOKEN="hm_tk_..."   # a user token from POST /v1/users

# 1. Health
curl -s $HM_URL/v1/health
# → {"status":"ok"}

# 2. Capture a decision (actor_id is resolved from your token — no need to supply it)
curl -s -X POST $HM_URL/v1/decisions \
  -H "Authorization: Bearer $HM_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Use Postgres for shared-backend storage",
    "rationale": "WAL mode does not scale across writers.",
    "topic_keys": ["infrastructure"],
    "chosen_option_label": "postgres",
    "options": [
      {"label": "postgres", "description": "r2d2 connection pool"},
      {"label": "sqlite",   "description": "WAL mode, single writer"}
    ]
  }' | tee /tmp/decision.json
# → {"decision_id":"decision-...","option_ids":[...],...}

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
        "Authorization": "Bearer hm_tk_<your-token-secret>"
      }
    }
  }
}
```

Use the `token_secret` value returned by `POST /v1/users` as the bearer token.
The token format is `hm_tk_<64-hex>` (shown once at creation time).

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
