# HiveMind Deployment Guide

This guide covers running HiveMind as a multi-tenant HTTP service using the
provided Docker image.

## Prerequisites

- Docker 24+ and Docker Compose v2
- (Optional) A Postgres 15+ instance if using the shared-backend-postgres feature

## Quick start: `docker compose up`

```bash
# Clone the repository
git clone https://github.com/alexknips/hivemind
cd hivemind

# (Optional) Set an API key — omit for development mode (no auth)
export HIVEMIND_API_KEY=your-secret-key

# Start HiveMind + Postgres
docker compose up --build
```

The HTTP API is available at `http://localhost:8080`.

Verify it is healthy:

```bash
curl http://localhost:8080/v1/health
# {"status":"ok"}
```

Data is persisted in a Docker named volume (`hivemind-data`). The Postgres
service is included for the `shared-backend-postgres` feature; HiveMind
defaults to SQLite stored in the same volume.

## Configuration reference

All configuration is passed through environment variables. Docker Compose
reads these from the shell or from an `.env` file in the project root.

| Variable | Default | Description |
|---|---|---|
| `HIVEMIND_DATABASE_URL` | *(unset)* | Postgres connection string. Also accepted as `--database-url` on the CLI and `hivemind mcp` (flag beats the env var), not only by `serve`. When set, the process connects directly to Postgres instead of the local SQLite ledger. |
| `HIVEMIND_DIR` | `/data` | Directory where the SQLite ledger is stored. Mount a volume here for persistence. |
| `HIVEMIND_PORT` | `8080` | Port the HTTP API listens on. Also accepted as `--port` / `-p` on the CLI. |
| `HIVEMIND_API_KEY` | *(unset)* | Static bearer token (SQLite mode only). Omit for development/trusted-network mode. |
| `HIVEMIND_ADMIN_KEY` | *(unset)* | Bearer token for `POST /v1/tenants` (Postgres mode). Required before provisioning tenants. |
| `HIVEMIND_TENANT` | `local` | Tenant to open. For the CLI and `hivemind mcp` this selects the Postgres tenant scope when `HIVEMIND_DATABASE_URL`/`--database-url` is also set (see below for what happens on an unrecognized value). Not used by `serve`, which resolves the tenant per request from the caller's auth instead. |
| `HIVEMIND_CORS_ORIGINS` | *(unset)* | Comma-separated origins for browser cross-origin requests. |
| `ANTHROPIC_API_KEY` | *(unset)* | Enables the Layer-3 ingest classifier (Claude Haiku). Optional. |
| `POSTGRES_PASSWORD` | `hivemind` | Password for the bundled Postgres service (compose only). |

### CLI and MCP direct-Postgres mode

`HIVEMIND_DATABASE_URL` (or `--database-url`) and `HIVEMIND_TENANT` (or
`--tenant`) are read by every `hivemind` subcommand, not only `serve` — this
includes `hivemind mcp`, so a stdio MCP server can talk to the shared Postgres
backend directly instead of the local SQLite file.

- The CLI and `hivemind mcp` never provision tenants in the `hm_tenants`
  table that `POST /v1/tenants` manages (see
  [SELF_HOSTING.md](SELF_HOSTING.md)) — but they also don't check against
  it: tenant-id validation only rejects an empty string, so an
  unrecognized or typo'd `--tenant` on a Postgres-backed CLI/mcp invocation
  is not rejected. It silently opens, and on first write silently
  populates, a brand-new empty tenant scope in the `events` table (there is
  no foreign key from `events.tenant_id` to `hm_tenants`). Whether this
  path should instead hard-error on an unknown tenant is an open design
  question, not yet decided.
- A binary built without the `shared-backend-postgres` Cargo feature fails
  with an explicit error if a database URL is configured — it never falls
  back to SQLite silently. The Docker image is built with this feature;
  `cargo install` / `make install` (the path most local dev setups use) is
  not, so a locally installed `hivemind` cannot open a Postgres ledger
  without rebuilding with `--features shared-backend-postgres`.

For a self-hosted cell (server + Postgres, one command), see [SELF_HOSTING.md](SELF_HOSTING.md).

### Example `.env` file

```dotenv
HIVEMIND_ADMIN_KEY=change-me-before-production
HIVEMIND_PORT=8080
HIVEMIND_TENANT=acme-corp
POSTGRES_PASSWORD=s3cr3t
```

## Healthcheck endpoint

`GET /v1/health` returns HTTP 200 when the database is reachable:

```json
{"status": "ok"}
```

Returns HTTP 503 when the database cannot be opened:

```json
{"status": "error", "message": "..."}
```

The Docker image configures this as the container HEALTHCHECK. Docker Compose
waits for it before considering the service ready.

## Graceful shutdown

The server handles `SIGINT` and `SIGTERM`. In-flight requests complete before
the process exits. Docker sends `SIGTERM` on `docker compose stop` or
`docker stop`, so running containers drain cleanly.

## Making API requests

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full endpoint list. A quick
example:

```bash
# Capture a decision
curl -s -X POST http://localhost:8080/v1/decisions \
  -H "Authorization: Bearer $HIVEMIND_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Use Postgres for shared-backend storage",
    "rationale": "SQLite WAL mode does not scale across multiple writer processes on shared storage.",
    "topic_keys": ["infrastructure", "storage"],
    "options": [
      {"label": "postgres", "description": "Postgres with r2d2 connection pool"},
      {"label": "sqlite",   "description": "SQLite with WAL mode"}
    ],
    "chosen_option_label": "postgres"
  }'

# Query decisions
curl -s "http://localhost:8080/v1/decisions/search?q=storage" \
  -H "Authorization: Bearer $HIVEMIND_API_KEY"
```

When `HIVEMIND_API_KEY` is unset, omit the `Authorization` header entirely.

### Fluent (no-id) read routes

Four GET routes resolve a decision without an id, the HTTP equivalent of the
CLI's `why`/`verify`/`recall`/`situational` verbs
(`docs/AGENT_FLUENT_QUERYING.md`):

| Route | Query params | Equivalent |
|---|---|---|
| `GET /v1/decisions/situational` | `paths` (required, comma-separated), `since_offset`, `since_ts`, `limit`, `cursor` | `hivemind query situational` |
| `GET /v1/decisions/recall` | `q`, `topic`, `status`, `actor_id`, `source`, `since`, `until`, `limit`, `cursor` | `hivemind query recall` |
| `GET /v1/decisions/why` | `id` or `description` (+ optional `topic`) | `hivemind query why` |
| `GET /v1/decisions/verify` | `id` or `description` (+ optional `topic`) | `hivemind query verify` |

```bash
curl -s "http://localhost:8080/v1/decisions/why?description=storage" \
  -H "Authorization: Bearer $HIVEMIND_API_KEY"
```

`why`/`verify` resolve `description` the same deterministic way `search`
does (term match + topic + recency, no LLM — `docs/AGENT_FLUENT_QUERYING.md`
§1). A description matching more than one decision at the best rank tier, or
matching none, is **not an error** — it comes back as HTTP 200 with
`data.outcome` set to `"ambiguous"` (with a `candidates` list) or
`"not_found"`. There is no `--pick`/`#N` continuation over HTTP (the API is
stateless): re-call with `id=<decision_id>` from the candidate list.

`disagree`/`supersede` are **not** fluent over HTTP today — `POST
/v1/decisions/{id}/disagreements` and `/v1/decisions/{id}/supersessions`
still take `decision_id` only as a path parameter, unlike their MCP
equivalents (`docs/AGENT_FLUENT_QUERYING.md` §3.4).

## Multi-tenant usage

Each request carries a tenant identifier via the `X-HiveMind-Tenant` header.
Tenant ledgers are isolated — decisions written for `tenant-a` are invisible
to `tenant-b` queries.

```bash
curl -s http://localhost:8080/v1/decisions/search \
  -H "Authorization: Bearer $HIVEMIND_API_KEY" \
  -H "X-HiveMind-Tenant: acme-corp" \
  -H "X-HiveMind-Actor: alice@acme.com"
```

See [MULTI_TENANCY.md](MULTI_TENANCY.md) for the full multi-tenant design.

## Production checklist

- [ ] Set `HIVEMIND_API_KEY` to a strong random value
- [ ] Mount a durable volume at `HIVEMIND_DIR` (or back up the named volume)
- [ ] Place a TLS-terminating reverse proxy (nginx, Caddy, Traefik) in front
      of the container — the server speaks plain HTTP
- [ ] Set `restart: always` in compose (or equivalent in your orchestrator)
- [ ] Monitor `/v1/health` with an external uptime checker

---

## Hosted deployment: Fly.io

The cheapest production path is a scale-to-zero Fly.io machine backed by a
tiny Fly Postgres cluster. The app machine costs ~$0 while idle (Fly stops it
automatically); the Postgres cluster stays running at ~$2/mo.

### Prerequisites

Install flyctl: <https://fly.io/docs/flyctl/install/>

```bash
fly auth login
```

### One-time setup

```bash
# 1. Create the Fly app (choose a name and the region nearest to you)
fly launch --name hivemind-api --region sea --no-deploy

# 2. Provision the smallest Fly Postgres cluster (~$2/mo, stays running)
fly postgres create \
  --name hivemind-pg \
  --region sea \
  --vm-size shared-cpu-1x \
  --volume-size 1

# Attach Postgres — injects HIVEMIND_DATABASE_URL as a Fly secret automatically
fly postgres attach hivemind-pg --app hivemind-api

# 3. Set the API key secret (all clients must send this as a Bearer token)
fly secrets set HIVEMIND_API_KEY="$(openssl rand -hex 32)" --app hivemind-api

# Optional: enable the Layer-3 ingest classifier
# fly secrets set ANTHROPIC_API_KEY=sk-ant-... --app hivemind-api

# 4. Deploy
fly deploy --app hivemind-api
```

### Verify

```bash
fly status --app hivemind-api
curl https://hivemind-api.fly.dev/v1/health
# {"status":"ok"}
```

### Cost estimate

| Resource | Config | Monthly cost |
|---|---|---|
| App machine | shared-cpu-1x, 256 MB, scale-to-zero | ~$0 idle / ~$1.94 if running 24/7 |
| Fly Postgres | shared-cpu-1x, 256 MB, 1 GB volume | ~$2.09 (always running) |
| Bandwidth | First 100 GB | $0 |
| **Total** | | **~$2–4/mo** |

### Secrets reference

Set these via `fly secrets set --app <your-app-name>`. Never commit them.

| Secret | Required | Description |
|---|---|---|
| `HIVEMIND_DATABASE_URL` | Yes (Postgres mode) | Set automatically by `fly postgres attach`. Enables multi-tenant Postgres backend. |
| `HIVEMIND_API_KEY` | Yes (production) | Bearer token all clients must send. Omit only in dev mode. |
| `HIVEMIND_ADMIN_KEY` | Recommended | Bearer token for the `POST /v1/tenants` provisioning endpoint. |
| `ANTHROPIC_API_KEY` | Optional | Enables the Layer-3 decision classifier on ingest. |

### Continuous deployment

The repository ships a GitHub Actions workflow at
`.github/workflows/deploy-hosted.yml` that deploys automatically on every
push to `master` that touches `src/**`, `Dockerfile`, `fly.toml`, or the
`Cargo.*` lock files.

Add your Fly API token as a repository secret:

```
GitHub → Settings → Secrets → Actions → New repository secret
Name:  FLY_API_TOKEN
Value: (output of `fly auth token`)
```

Manual deploys are also supported via the **Actions → Deploy to Fly.io →
Run workflow** button.
