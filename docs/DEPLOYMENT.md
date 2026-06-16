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
| `HIVEMIND_DIR` | `/data` | Directory where the SQLite ledger is stored. Mount a volume here for persistence. |
| `HIVEMIND_PORT` | `8080` | Port the HTTP API listens on. Also accepted as `--port` / `-p` on the CLI. |
| `HIVEMIND_API_KEY` | *(unset)* | Bearer token for API auth. When unset the server starts in **development mode** — all requests are accepted without auth. Set this in production. |
| `HIVEMIND_TENANT` | `local` | Default tenant identifier written into events emitted via CLI. The HTTP API reads tenant scope from the `X-HiveMind-Tenant` request header (default: `local`). |
| `POSTGRES_PASSWORD` | `hivemind` | Password for the bundled Postgres service (compose only). |

### Example `.env` file

```dotenv
HIVEMIND_API_KEY=change-me-before-production
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
