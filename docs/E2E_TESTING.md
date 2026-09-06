# HiveMind End-to-End Testing

This document describes the E2E test suite that exercises the full server stack
as deployed: Docker Compose cell up → capture (CLI + MCP + HTTP) → ledger →
projection → queries/search → review (disagree / supersede) → quality-scan.

Unlike the unit and integration tests in `tests/` (which run in-process or
against an in-memory ledger), the E2E suite talks to a real running HTTP server
over TCP.

---

## Slices

| Slice | File | Backends | LLM key? | Status |
|-------|------|----------|-----------|--------|
| 1a | `scripts/e2e_smoke.sh` | SQLite | No (key-gated) | **Implemented** |
| 1b | `scripts/e2e_smoke.sh --skip-map` + Postgres compose profile | SQLite + Postgres | No | **Implemented** |
| 2 | LLM-gated extensions in `e2e_smoke.sh` | Both | Yes | Planned (hivemind-fabq.3, after key lands) |
| 3 | WorkOS-auth compose profile | Postgres + WorkOS JWT | Test creds | Planned (hivemind-fabq.4) |

---

## Running locally

### Prerequisites

- Docker and Docker Compose v2
- `curl` and `jq`
- A built `hivemind` binary (for CLI + MCP legs) — see below

```bash
# 1. Build the image and the binary
docker compose build
cargo build --locked --bin hivemind

# 2. Start the server (SQLite, no auth)
docker compose up -d hivemind

# 3. Wait for health
until curl -sf http://localhost:8080/v1/health | grep -q '"ok"'; do sleep 1; done

# 4. Run the smoke suite
HIVEMIND_BIN=./target/debug/hivemind \
  bash scripts/e2e_smoke.sh

# 5. Tear down
docker compose down -v
```

### LLM-gated assertions

Set `ANTHROPIC_API_KEY` before running the script to enable the classifier and
summarizer assertions (cheap model: `claude-haiku-4-5-20251001`):

```bash
ANTHROPIC_API_KEY=sk-ant-... \
HIVEMIND_BIN=./target/debug/hivemind \
  bash scripts/e2e_smoke.sh
```

When the key is absent, those assertions are marked `SKIP` and the suite still
exits `0` — keyless CI remains green.

### Postgres backend

Run against the local Postgres container using the provided compose override:

```bash
# 1. Build the image and the binary
docker compose build
cargo build --locked --bin hivemind

# 2. Start postgres + hivemind (Postgres backend)
docker compose \
  -f docker-compose.yml \
  -f docker-compose.e2e-postgres.yml \
  up -d

# 3. Wait for health
until curl -sf http://localhost:8080/v1/health | grep -q '"ok"'; do sleep 1; done

# 4. Run the smoke suite (spectral map skipped — SQLite only per hivemind-270r)
HIVEMIND_BIN=./target/debug/hivemind \
  bash scripts/e2e_smoke.sh --skip-map

# 5. Tear down
docker compose \
  -f docker-compose.yml \
  -f docker-compose.e2e-postgres.yml \
  down -v
```

`--skip-map` is required because `GET /v1/decisions/map` is SQLite-only
(tracked in hivemind-270r).

---

## Script overview: `scripts/e2e_smoke.sh`

The script exercises these paths in order:

| Section | Endpoints / transports |
|---------|------------------------|
| Health | `GET /v1/health` |
| Capture — HTTP | `POST /v1/decisions`, `/v1/evidence`, `/v1/hypotheses` |
| Capture — CLI | `hivemind emit decision.proposed` (local dir) |
| Capture — MCP stdio | MCP `capture_decision` tool (local dir) |
| Query / projection | `GET /v1/decisions/{id}`, `/{id}/compact-view`, `/v1/graph` |
| Search | `GET /v1/decisions/search`, `/v1/decisions/relevant` |
| Spectral map | `GET /v1/decisions/map` (skipped on Postgres) |
| Review | `POST /v1/decisions/{id}/disagreements`, `/{id}/supersessions` |
| Supersession chain | `GET /v1/decisions/{id}/supersession-chain` |
| Multi-tenant isolation | Two-tenant capture + graph cross-check |
| Quality scan | MCP `scan_decision_quality` tool |
| LLM-gated | Capture with classifier enabled (key-gated) |

Exit code `0` = all non-skipped assertions passed.
Exit code `1` = at least one assertion failed.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HIVEMIND_E2E_BASE_URL` | `http://localhost:8080` | Server URL |
| `HIVEMIND_E2E_API_KEY` | *(empty)* | Bearer token (empty = dev mode, no auth) |
| `HIVEMIND_E2E_TENANT` | `e2e-test` | Tenant ID used for most requests |
| `HIVEMIND_E2E_SKIP_MAP` | `false` | Set `true` to skip spectral-map assertion (Postgres backends) |
| `ANTHROPIC_API_KEY` | *(empty)* | When set, LLM-gated assertions are enabled |
| `HIVEMIND_BIN` | `hivemind` | Path to the `hivemind` binary |

### Flags

| Flag | Description |
|------|-------------|
| `--base-url URL` | Override `HIVEMIND_E2E_BASE_URL` |
| `--api-key KEY` | Override `HIVEMIND_E2E_API_KEY` |
| `--tenant TENANT` | Override `HIVEMIND_E2E_TENANT` |
| `--skip-map` | Skip `GET /v1/decisions/map` (Postgres backends, hivemind-270r) |

---

## CI integration

Two jobs in `.github/workflows/ci.yml` run E2E smoke tests automatically on
every PR and `master` push:

**`e2e-compose` (SQLite):**

1. Builds the Docker image (`hivemind:e2e`).
2. Starts `hivemind` (SQLite, no-deps) via Docker Compose and waits for `/v1/health`.
3. Builds the `hivemind` binary (for CLI + MCP legs).
4. Runs `scripts/e2e_smoke.sh` (LLM assertions skipped unless
   `ANTHROPIC_API_KEY` secret is set).
5. Tears down with `docker compose down -v`.

**`e2e-compose-postgres` (Postgres):**

1. Builds the Docker image (`hivemind:e2e`).
2. Starts both `postgres` and `hivemind` (Postgres backend) and waits up to 60s
   for `/v1/health`.
3. Builds the `hivemind` binary (for CLI leg).
4. Runs `scripts/e2e_smoke.sh --skip-map` (spectral-map skipped per hivemind-270r).
5. Dumps `hivemind` and `postgres` logs on failure.
6. Tears down with `docker compose down -v`.

`ANTHROPIC_API_KEY` is an optional GitHub Actions secret (name:
`ANTHROPIC_API_KEY`). When absent from the repo secrets the LLM assertions
are skipped; CI stays green. When Alex provisions the key, the full LLM
slice activates automatically.

---

## Known limitations

- `GET /v1/decisions/map` is SQLite-only (hivemind-270r). The `--skip-map`
  flag suppresses it for Postgres runs.
- The Neon-hosted DB (`hivemind-5r6q`) is currently down and excluded from
  PR CI. It may be added as a non-blocking nightly job after hivemind-5r6q
  is resolved.
- WorkOS JWT auth is not yet tested end-to-end (tracked: hivemind-fabq.4,
  blocked on Alex provisioning WorkOS test credentials).
