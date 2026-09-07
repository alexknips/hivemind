# HiveMind End-to-End Testing

This document describes the E2E test suite that exercises the full server stack
as deployed: Docker Compose cell up → capture (CLI + MCP + HTTP) → ledger →
projection → queries/search → review (disagree / supersede) → quality-scan.

Unlike the unit and integration tests in `tests/` (which run in-process or
against an in-memory ledger), the E2E suite talks to a real running HTTP server
over TCP.

---

## Slices

| Slice | File | Backends | LLM auth | Status |
|-------|------|----------|-----------|--------|
| 1a | `scripts/e2e_smoke.sh` | SQLite | None | **Implemented** |
| 1b | `scripts/e2e_smoke.sh --skip-map` + Postgres compose profile | SQLite + Postgres | None | **Implemented** |
| 2 | LLM-gated extensions in `e2e_smoke.sh` | Both | Claude subscription, edge (`claude -p`), LOCAL-only | **Implemented** (hivemind-fabq.3) |
| 3 | WorkOS-auth compose profile | Postgres + WorkOS JWT | Test creds | Planned (hivemind-fabq.4) |

---

## Running locally

### Prerequisites

- Docker and Docker Compose v2
- `curl` and `jq`
- A built `hivemind` binary (for CLI + MCP legs) — see below
- A built `fidelity-eval` binary (for fidelity ceiling-mode smoke) — optional

```bash
# 1. Build the image and the binaries
docker compose build
cargo build --locked --bin hivemind --bin fidelity-eval

# 2. Start the server (SQLite, no auth)
docker compose up -d hivemind

# 3. Wait for health
until curl -sf http://localhost:8080/v1/health | grep -q '"ok"'; do sleep 1; done

# 4. Run the smoke suite
HIVEMIND_BIN=./target/debug/hivemind \
FIDELITY_BIN=./target/debug/fidelity-eval \
  bash scripts/e2e_smoke.sh

# 5. Tear down
docker compose down -v
```

### LLM-gated assertions (Slice 2) — subscription/edge path, LOCAL only

RE-SCOPED 2026-09-07 (hivemind-fabq.3): HiveMind never holds an
`ANTHROPIC_API_KEY` for this path. Slice 2 instead exercises the same
mechanism a real coding agent uses: decision extraction happens at the
**edge**, inside the `hivemind-capture` plugin's "Batch Capture via Haiku
Subagent (Keyless)" workflow, riding the operator's own Claude subscription
through a headless `claude -p` session (see
`plugins/hivemind-capture/skills/hivemind-capture/SKILL.md` and
hivemind-mfc7). This is unrelated to — and does not exercise — the
server-side `src/classifier.rs` / `src/scorer.rs` background workers, which
remain keyed and dark in this flow.

Prerequisites: the `claude` CLI on `PATH` (or `CLAUDE_BIN` pointing at it),
logged in via `claude auth login` (subscription or `claude.ai` auth). No
`ANTHROPIC_API_KEY` — if it's set, these legs SKIP (they specifically test
the keyless path):

```bash
HIVEMIND_BIN=./target/debug/hivemind \
FIDELITY_BIN=./target/debug/fidelity-eval \
  bash scripts/e2e_smoke.sh
```

Three legs:

- **LEG 1 (classifier enrichment)** — runs a real `claude -p` session with
  `--plugin-dir plugins/hivemind-capture` against a fixed 4-turn transcript,
  using the classifier prompt extracted verbatim from SKILL.md. Asserts the
  resulting `ingest.batch_classified` batch lands in a scratch ledger with
  schema parity to `src/classifier.rs`'s `CaptureItem` contract.
- **LEG 2 (quality-score enrichment)** — `SKIP`: the plugin has no
  scorer-edge workflow yet (`src/scorer.rs` is still server-only, keyed).
  Follow-up: hivemind-wi3u.
- **LEG 3 (fidelity smoke)** — `SKIP`: the fidelity evaluator's
  subscription/`claude-cli` backend hasn't landed yet (hivemind-265w); only
  the keyed `x-api-key` backend exists today.

This whole section is **LOCAL-only by design** — CI runners have no Claude
subscription, so `claude` is absent/unauthenticated there and every leg
SKIPs, keeping CI green regardless.

**Note:** The `fidelity-eval --ceiling` (Slice 2 infra smoke, no LLM) runs
unconditionally when the binary is available, validating the binary + projector
machinery without any API calls.

### Postgres backend

The Postgres backend enables mandatory bearer-token auth.  You need to set
`HIVEMIND_ADMIN_KEY` before starting the containers, then provision two tenants
(one for the main test, one for the multi-tenant isolation assertion).

```bash
# 1. Build the image and the binary
docker compose build
cargo build --locked --bin hivemind

# 2. Start postgres + hivemind (Postgres backend) with an admin key
HIVEMIND_ADMIN_KEY=my-local-admin-key \
docker compose \
  -f docker-compose.yml \
  -f docker-compose.e2e-postgres.yml \
  up -d

# 3. Wait for health
until curl -sf http://localhost:8080/v1/health | grep -q '"ok"'; do sleep 1; done

# 4. Provision tenants A and B via the admin API
TOKEN_A=$(curl -sf -X POST http://localhost:8080/v1/tenants \
  -H "Authorization: Bearer my-local-admin-key" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"e2e-test","display_name":"E2E Test Tenant A"}' \
  | jq -r '.token_secret')

TOKEN_B=$(curl -sf -X POST http://localhost:8080/v1/tenants \
  -H "Authorization: Bearer my-local-admin-key" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"e2e-test-other","display_name":"E2E Test Tenant B"}' \
  | jq -r '.token_secret')

# 5. Run the smoke suite (spectral map skipped — SQLite only per hivemind-270r)
HIVEMIND_BIN=./target/debug/hivemind \
HIVEMIND_E2E_API_KEY="$TOKEN_A" \
HIVEMIND_E2E_API_KEY_B="$TOKEN_B" \
  bash scripts/e2e_smoke.sh --skip-map --skip-search

# 6. Tear down
docker compose \
  -f docker-compose.yml \
  -f docker-compose.e2e-postgres.yml \
  down -v
```

`--skip-map` is required because `GET /v1/decisions/map` is SQLite-only
(tracked in hivemind-270r).

`HIVEMIND_E2E_API_KEY_B` carries the bearer token for the second tenant used in
the multi-tenant isolation assertion.  Without it, the assertion skips gracefully
in SQLite no-auth mode but is required for Postgres.

---

## Script overview: `scripts/e2e_smoke.sh`

The script exercises these paths in order:

| Section | Endpoints / transports |
|---------|------------------------|
| Health | `GET /v1/health` |
| Capture — HTTP | `POST /v1/decisions`, `/v1/evidence`, `/v1/hypotheses` |
| Capture — CLI | `hivemind emit decision.proposed` (local dir) |
| Capture — MCP HTTP | MCP `capture_decision` tool (HTTP) |
| Query / projection | `GET /v1/decisions/{id}`, `/{id}/compact-view`, `/v1/graph` |
| Search | `GET /v1/decisions/search`, `/v1/decisions/relevant` |
| Spectral map | `GET /v1/decisions/map` (skipped on Postgres) |
| Review | `POST /v1/decisions/{id}/disagreements`, `/{id}/supersessions` |
| Supersession chain | `GET /v1/decisions/{id}/supersession-chain` |
| Multi-tenant isolation | Two-tenant capture + graph cross-check |
| Quality scan | MCP `scan_decision_quality` tool |
| Quality score + summarize | MCP `score_decision`, `summarize_decisions` (rule-based) |
| Fidelity binary (ceiling) | `fidelity-eval --ceiling` (no LLM; validates binary + projector) |
| LLM-gated (Slice 2) | Edge classifier enrichment via `claude -p` + plugin (LOCAL-only, subscription); quality-score + fidelity legs SKIP with reason |

Exit code `0` = all non-skipped assertions passed.
Exit code `1` = at least one assertion failed.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HIVEMIND_E2E_BASE_URL` | `http://localhost:8080` | Server URL |
| `HIVEMIND_E2E_API_KEY` | *(empty)* | Bearer token for tenant A (empty = dev/no-auth mode) |
| `HIVEMIND_E2E_API_KEY_B` | *(falls back to `API_KEY`)* | Bearer token for tenant B in the multi-tenant isolation test; required in Postgres auth mode to produce real isolation |
| `HIVEMIND_E2E_TENANT` | `e2e-test` | Tenant ID used for most requests |
| `HIVEMIND_E2E_SKIP_MAP` | `false` | Set `true` to skip spectral-map assertion (Postgres backends) |
| `HIVEMIND_E2E_SKIP_SEARCH` | `false` | Set `true` to skip FTS search assertion (Postgres backends) |
| `HIVEMIND_BIN` | `hivemind` | Path to the `hivemind` binary |
| `FIDELITY_BIN` | `fidelity-eval` | Path to the `fidelity-eval` binary (Slice 2 ceiling smoke) |
| `FIDELITY_CORPUS` | `benchmarks/fidelity/corpus.yaml` | Full corpus for ceiling mode |
| `FIDELITY_CORPUS_SMOKE` | `benchmarks/fidelity/corpus-smoke.yaml` | 2-case corpus for LLM smoke (unused until hivemind-265w lands) |
| `CLAUDE_BIN` | `claude` | Path to the `claude` CLI, used only by the LOCAL-only subscription/edge LLM legs |
| `ANTHROPIC_API_KEY` | *(empty)* | If SET, the subscription/edge LLM legs SKIP — they specifically test the keyless path |

### Flags

| Flag | Description |
|------|-------------|
| `--base-url URL` | Override `HIVEMIND_E2E_BASE_URL` |
| `--api-key KEY` | Override `HIVEMIND_E2E_API_KEY` |
| `--tenant TENANT` | Override `HIVEMIND_E2E_TENANT` |
| `--skip-map` | Skip `GET /v1/decisions/map` (Postgres backends, hivemind-270r) |
| `--skip-search` | Skip `GET /v1/decisions/search` (Postgres backends, FTS not in shared-backend) |

---

## CI integration

Two jobs in `.github/workflows/ci.yml` run E2E smoke tests automatically on
every PR and `master` push:

**`e2e-compose` (SQLite):**

1. Builds the Docker image (`hivemind:e2e`).
2. Starts `hivemind` (SQLite, no-deps) via Docker Compose and waits for `/v1/health`.
3. Builds `hivemind` and `fidelity-eval` binaries.
4. Runs `scripts/e2e_smoke.sh` (Slice 2 subscription/edge LLM legs always
   SKIP on CI runners — no Claude subscription; ceiling-mode fidelity smoke
   always runs when the binary is available).
5. Tears down with `docker compose down -v`.

**`e2e-compose-postgres` (Postgres):**

1. Builds the Docker image (`hivemind:e2e`).
2. Starts both `postgres` and `hivemind` (Postgres backend, `HIVEMIND_ADMIN_KEY=e2e-admin-key-ci`) and waits up to 60s for `/v1/health`.
3. Builds the `hivemind` binary (for CLI leg).
4. Provisions two tenants via the admin API (`e2e-test`, `e2e-test-other`) and captures their bearer tokens.
5. Runs `scripts/e2e_smoke.sh --skip-map --skip-search` with both tokens (`HIVEMIND_E2E_API_KEY` / `HIVEMIND_E2E_API_KEY_B`); spectral-map and FTS search skipped (Postgres limitations); 401 regression assertion included.
6. Dumps `hivemind` and `postgres` logs on failure.
7. Tears down with `docker compose down -v`.

The Slice 2 subscription/edge LLM legs (LEG 1-3, see above) are LOCAL-only:
CI runners have no Claude subscription, so `claude` is absent/unauthenticated
and every leg SKIPs unconditionally — this requires no secret and no
provisioning step, by design.

---

## Known limitations

- `GET /v1/decisions/map` is SQLite-only (hivemind-270r). The `--skip-map`
  flag suppresses it for Postgres runs.
- `GET /v1/decisions/search` (full-text search) is SQLite-only; the shared-backend
  returns a validation error. Use `--skip-search` for Postgres runs; topic-based
  queries via `GET /v1/decisions/relevant` work on all backends.
- The Neon-hosted DB (`hivemind-5r6q`) is currently down and excluded from
  PR CI. It may be added as a non-blocking nightly job after hivemind-5r6q
  is resolved.
- WorkOS JWT auth is not yet tested end-to-end (tracked: hivemind-fabq.4,
  blocked on Alex provisioning WorkOS test credentials).
