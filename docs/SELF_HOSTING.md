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
curl http://localhost:8080/v1/version
# {"version":"0.6.0+d2d203d1a2b3","cargo_version":"0.6.0","sha":"d2d203d1a2b3"}
```

`/v1/health` and `/v1/version` are both unauthenticated liveness/build probes.
Like `hivemind --version` locally, `/v1/version`'s `sha` is what the running
container was actually built from, not what a tag or `:latest` claims.

> **Search note:** `GET /v1/decisions/search` and `GET /v1/decisions/recall` work on both
> backends — SQLite goes through FTS5, Postgres goes through a portable in-memory term
> matcher with the same ranking order (`docs/AGENT_FLUENT_QUERYING.md` §1.5). `GET
> /v1/decisions/relevant?topic=<topic>` remains available on both backends too, for exact
> topic-key filtering rather than free-text search.

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
| `HIVEMIND_DATABASE_URL` | *(unset)* | Postgres connection string. When set enables the multi-tenant Postgres backend. Also accepted as `--database-url` on the CLI and `hivemind mcp` (flag beats the env var), not only by `serve` — see below. Unset = SQLite at `HIVEMIND_DIR`. |
| `HIVEMIND_DIR` | `/data` | Directory for the SQLite ledger (SQLite mode only). Mount a volume here. |
| `HIVEMIND_PORT` | `8080` | Port the HTTP API listens on inside the container. |
| `HIVEMIND_ADMIN_KEY` | *(unset)* | Bearer token for `POST /v1/tenants`, `POST /v1/users`, `GET /v1/users`, and token revocation. Required before provisioning tenants or users. |
| `HIVEMIND_API_KEY` | *(unset)* | Static bearer token (SQLite mode only). Omit for development/trusted-network mode. |
| `ANTHROPIC_API_KEY` | *(unset)* | Enables the Layer-3 ingest classifier (Claude Haiku). Optional. |
| `HIVEMIND_CORS_ORIGINS` | *(unset)* | Comma-separated origins allowed for browser cross-origin requests. |
| `POSTGRES_PASSWORD` | `hivemind` | Password for the bundled Postgres service. Change before production. |
| `HIVEMIND_TENANT` | `local` | Default tenant. For the CLI and `hivemind mcp` this selects the Postgres tenant scope when `HIVEMIND_DATABASE_URL`/`--database-url` is also set. Not used by `serve`, which resolves the tenant per request from the caller's auth token instead. |

### CLI and MCP direct-Postgres mode

`HIVEMIND_DATABASE_URL`/`--database-url` and `HIVEMIND_TENANT`/`--tenant` are
read by every `hivemind` subcommand, not only `serve` — including
`hivemind mcp`, so a stdio MCP server can talk to this cell's Postgres
directly instead of a local SQLite file. Two facts worth knowing before
using this:

- The CLI and `hivemind mcp` never provision tenants in the `hm_tenants`
  table that `POST /v1/tenants` manages (see above) — you must provision a
  tenant through the API first, then point `--tenant` at it.
- A binary built without the `shared-backend-postgres` Cargo feature fails
  with an explicit error if a database URL is configured — it never falls
  back to SQLite silently. The Docker image is built with this feature;
  `cargo install` / `make install` is not, so a locally installed `hivemind`
  needs `--features shared-backend-postgres` to open a Postgres ledger.

See the "Using the CLI / MCP from local agents" section below for how to
reach this cell's Postgres from outside the compose network.

**WorkOS variables** (`WORKOS_DOMAIN`, `WORKOS_JWKS_URL`, `WORKOS_AUDIENCE`) enable
OIDC browser login. Self-hosted cells typically omit these; the token-based auth
above is the supported self-host path. If you do need OIDC, leave connector
config (`CONNECTOR_*`) as a placeholder — that surface is not yet finalised
(see hivemind-ld68).

---

## Keyless capture — no ANTHROPIC_API_KEY required

`ANTHROPIC_API_KEY` in the table above is **optional**. Without it the server
starts and operates correctly — the only thing that doesn't run is the
server-side background classifier (Worker B).

**If you omit `ANTHROPIC_API_KEY`**, classification still happens via
**Worker A**: the `hivemind-capture` plugin ships a
`/hivemind-capture:classify-queue` command that drains the same pending queue
using your agent's subscription seat. No key, no extra configuration.

### Keyless walkthrough

1. **Start the cell** without an API key (skip that line in `.env`).

2. **Install the capture plugin** from Claude Code:

   ```text
   /plugin marketplace add alexknips/hivemind
   /plugin install hivemind-capture@hivemind
   /reload-plugins
   ```

3. **Capture a decision** in-session:

   ```text
   /hivemind-capture:capture "Use Postgres for the shared backend" \
     --kind decision \
     --rationale "SQLite WAL mode does not scale across concurrent writers" \
     --topic-keys infrastructure,storage \
     --options sqlite,postgres \
     --chose postgres
   ```

4. **Drain the classification queue** after your session:

   ```text
   /hivemind-capture:classify-queue
   ```

   The command classifies pending batches using your subscription model and
   writes `IngestBatchClassified` events. Pass `--limit N` to cap the run.

5. **Query back**:

   ```bash
   hivemind query recent_decisions --since 1h --limit 5
   ```

See [`docs/KEYLESS_CAPTURE.md`](KEYLESS_CAPTURE.md) for the full walkthrough
and a comparison of Worker A vs Worker B.

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

# 3. Query it back — full-text search/recall work on both backends (see the
# Search note above); topic filtering also works on both:
curl -s "$HM_URL/v1/decisions/relevant?topic=infrastructure" \
  -H "Authorization: Bearer $HM_TOKEN" | python3 -m json.tool | head -20

# 3b. Fluent (no-id) follow-up — "why was this decided?", by description:
curl -s "$HM_URL/v1/decisions/why?description=Postgres%20for%20shared-backend" \
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

**Fluent (no-id) routes:** step 3b above is one of four GET routes that
resolve a decision without an id — `situational`, `recall`, `why`, `verify`.
See `docs/DEPLOYMENT.md`'s "Fluent (no-id) read routes" table for the full
param list; all four work identically on both backends. `disagree`/
`supersede` remain `decision_id`-path-only over HTTP (not fluent).

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

**Actor identity differs from the REST endpoints above.** `POST /v1/decisions`
and friends always attribute writes to the bearer token's own bound identity
and ignore any client-supplied actor — that is what "Actor identity is locked
to the token" means in the auth story above. The `/mcp` tool-call surface is
more permissive: `capture_decision`, `capture_evidence`, `capture_hypothesis`,
`disagree_decision`, and `supersede_decision` all accept an optional
`actor_id` argument that, when present, overrides the token's bound identity
for that one call (`src/api/mcp_http.rs::mcp_resolve_actor`; verified against
a running cell — a `tools/call capture_decision` with
`"actor_id": "agent:claude:some-session"` produces a `PROPOSED_BY` edge to
that actor, not to the token's own identity). This lets one shared per-role
token serve many distinct callers with correct per-caller attribution — e.g.
a fleet of agent sessions sharing one token, each passing its own
`agent:<tool>:<session>` as `actor_id` — without minting a token per session.

This is a convenience, not a security boundary: `actor_id` here is
**caller-asserted, not verified**. Any holder of the token can claim any
actor string, including another human's or agent's identity — the same trust
level as the CLI's free-text `--actor` flag. Only share one token across
callers you already trust not to misattribute their own writes; mint a
personal per-user token (Step 2 above) whenever that trust does not hold.

---

## Using the CLI / MCP from local agents

The compose stack above publishes only port 8080 (`docker-compose.yml`);
Postgres is reachable only on the compose network. An agent that wants to
run the `hivemind` CLI or `hivemind mcp` with `--database-url` /
`HIVEMIND_DATABASE_URL` against the cell's own Postgres — instead of going
through the HTTP API — needs one of the following.

### Option A — publish Postgres on loopback

Bring the stack up with the local-agents override, which adds a
loopback-only port mapping for the `postgres` service
(`docker-compose.local-agents.yml`; not applied by default):

```bash
docker compose -f docker-compose.yml -f docker-compose.local-agents.yml up -d
```

Then point the CLI or `hivemind mcp` at it directly:

```bash
HIVEMIND_DATABASE_URL=postgres://hivemind:<POSTGRES_PASSWORD>@127.0.0.1:5432/hivemind \
HIVEMIND_TENANT=<tenant> \
hivemind query situational --diff
```

Set `POSTGRES_PORT` in `.env` if `5432` collides with a Postgres instance
already running on the host.

**This puts the database credential on the agent's machine.** The
`POSTGRES_PASSWORD` from `.env` is a full read/write credential to every
tenant's data on the cell, not scoped to one tenant. Only use this option on
a host you trust with that credential. A CLI that talks to the server's HTTP
API instead of the database directly would avoid this, but that is a
separate, larger change — file it only if this credential story doesn't
work for your setup.

### Option B — run the CLI inside the container

No port publishing needed. Exec into the running `hivemind` container, which
already has `HIVEMIND_DATABASE_URL` pointed at the compose network address:

```bash
docker compose exec hivemind hivemind --tenant <tenant> query situational --diff
```

This is the safer default — no database credential ever leaves the
container.

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
./scripts/cell-update.sh [ref]   # default ref: origin/master
```

Builds the server image from the given ref (default `origin/master`), tags
it by commit sha — never `:latest`, which only moves when a `v*` tag is
pushed (see `.github/workflows/release.yml`) and can otherwise sit behind
master for weeks — restarts the `hivemind` service with it, and waits for
`/v1/health` before declaring success. On failure the previous container is
left running; nothing is torn down until the replacement is confirmed
healthy. Confirm what's actually running with `curl
http://localhost:8080/v1/version`.

Equivalent by hand, without the health check or sha tagging:

```bash
git pull
docker compose build --no-cache
docker compose up -d
```

Note: with `image:` and `build:` both set in `docker-compose.yml`, a bare
`docker compose up -d` reuses whatever image already carries that tag
locally rather than pulling — so `docker compose build` must run first, or
this silently keeps serving the old container.

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
