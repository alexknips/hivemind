# Changelog

All notable changes to HiveMind are documented here.
This project adheres to [Semantic Versioning](https://semver.org/).

## v0.3.0 — 2026-06-18 — M3: Layer-3 recall tools over the MCP gateway

HiveMind gains its first Layer-3 read tools for on-demand recall: decision
summarization and graph compactification, alongside richer search. All are
exposed as read-only tools over the authenticated, tenant-isolated MCP gateway,
so agents can recall and condense organizational decision memory on demand
without a local ledger. Layer-3 stays swappable — the write and query layers
remain fully functional without it.

### Added
- **Decision summarization** (`summarize_decisions` MCP tool, Layer-3). Produces
  a text summary of the decisions matching a query/filter, built purely from the
  read layer — no writes, no inference beyond explicit status derivation.
  (`src/summarize.rs`)
- **Graph compactification** (`hivemind_compact_view` MCP tool, Layer-3). A
  `CompactView` query that collapses safely-redundant detail while preserving
  main decisions and their rationale, per the signal/noise semantics in the
  spec. (`src/queries/compact_view.rs`)
- **Compactification specification.** `docs/COMPACTIFICATION_SPEC.md` defines the
  signal/noise rules for graph compaction — what detail is safely droppable
  versus what must always be preserved.
- **Search filter improvements.** HTTP decision search now honors the `source`
  filter and comma-separated `actor_id` values. (`src/api.rs`)
- **MCP gateway end-to-end tests.** `tests/mcp_stdio_e2e.rs` exercises the stdio
  MCP gateway across search, summarize, compact-view, and tenant isolation.

### Deferred to follow-up
- **hivemind-yfbq** (P3): tighten the UBS warning baseline now that
  assertion-heavy integration-test files under `tests/` are exempted via
  `.ubsignore`. Criticals remain gated everywhere; only the warning-count
  baseline is affected.

## v0.2.0 — 2026-06-17 — M2: Shared multi-tenant backend

HiveMind now runs as a hosted multi-tenant service. A single Postgres-backed
process serves multiple tenants with cryptographic token auth and row-level
isolation, reachable over a REST API from CLI, MCP, or any HTTP client.

### Added
- **Postgres ledger + projection.** `PostgresEventLedger` and
  `PostgresGraphView` back the write and read layers in Postgres. Schema
  migrations run at startup. Projection is still rebuildable from the ledger.
  Enabled via the `shared-backend-postgres` feature flag.
- **HTTP REST API** (`/v1/`). A third transport over the same internal commands
  and query functions: `POST /v1/decisions`, `GET /v1/decisions/{id}`,
  `GET /v1/decisions/search`, `GET /v1/decisions/relevant`,
  `POST /v1/ingest`, `POST /v1/tenants`, `GET /v1/health`.
  Bearer token auth is enforced when an API key is configured.
- **Tenant provisioning + Postgres RLS isolation.** `POST /v1/tenants`
  creates a tenant and issues a `hm_tk_<64-hex>` bearer token. Token
  resolution uses a SHA-256 hash stored in `hm_tokens`. Row-level security
  in Postgres prevents cross-tenant event reads at the database layer.
  SQLite dev mode provides header-based tenant scoping for local development.
- **Capture clients: hook + sidecar.** Two autonomous capture surfaces built
  over a shared `ingest-client` core: a commit-hook shipper and a sidecar
  daemon. Both carry actor provenance and route to the same `/v1/ingest`
  endpoint.
- **Haiku classifier for ingest batches.** `POST /v1/ingest` with the
  `shared-backend-postgres` feature routes accepted batches through a
  Claude Haiku call that extracts decision candidates, topic, and confidence
  from conversation turns. Classifier output is stored per-batch.
- **MCP read gateway.** A thin MCP server that wraps the HiveMind HTTP API,
  exposing `search_decisions` and `get_decision` tools to MCP clients
  (Claude Code, Codex, IDE plugins) without a local SQLite dependency.
- **Docker deploy.** `Dockerfile` and `docker-compose.yml` for production
  deployment. `/v1/health` endpoint and Docker healthcheck. Deployment guide
  in `docs/DEPLOYMENT.md`.
- **Multi-tenant test fixtures and integration tests.** Dedicated
  `tests/multi_tenant.rs` suite covering three isolated tenants. HTTP
  integration tests in `tests/api.rs` covering auth, RLS, ingest,
  capture+query round-trip, and supersession.
- **Decision-scoring model locked** (design only; not yet implemented).
  Layer-3, post-PoC 2-axis scoring design recorded in
  `docs/DECISION_SCORING.md` with research basis in
  `docs/DECISION_QUALITY_LITERATURE.md`.

### Architecture docs updated
- `docs/ARCHITECTURE.md` — updated to reflect M2 transport surface and
  Postgres state model.
- `docs/REMOTE_DB.md` — updated to M2 shipped state.
- `docs/AGENT_DECISION_CAPTURE.md` — updated capture flow for HTTP service.
- `docs/MCP_SERVICE_SPLIT.md` — documents MCP gateway over HTTP API.
- `docs/M2_VERIFICATION.md` — e2e verification matrix, all steps passed.

### Deferred to follow-up
- **uuq9.8**: `hivemind migrate` CLI for local-to-remote ledger migration.
  Step 5 of M2 verification is explicitly out of scope for this release.
- **uuq9.19**: Full 2-axis decision scorer (Layer-3, post-PoC). Design is
  locked and documented; implementation is not authorized until after the
  hosted MVP.

## v0.1.0 — 2026-06-15 — M1: Dogfood loop ships

First milestone release. HiveMind — an event-sourced, graph-projected decision
ledger for human governance of agentic decision-making — is now usable end to
end inside its own repository.

### Added
- **Event-sourced decision ledger.** Typed events (decision, option, evidence,
  hypothesis, relation) appended to an authoritative SQLite ledger and projected
  into a graph (Decision, Actor, Evidence, Option, Hypothesis). Status
  (proposed / accepted / contested / superseded) is derived from edges, never
  stored; the ledger is the trust boundary and smart behavior stays out of the
  write path.
- **Capture surfaces.** CLI (`hivemind emit decision.capture`), an MCP server,
  and Claude Code / Codex capture plugins — all thin wrappers over the same
  internal commands, carrying actor provenance (`agent:<tool>:<session>` /
  `human:<id>`) and `source=agent|human`.
- **Deterministic query layer.** `search_decisions` with filters (topic, status,
  actor, source, time, evidence, supersession), full-text search, bounded pages,
  and explicit rank basis. Semantic/vector ranking is intentionally kept out of
  the query path (a layer-3 concern, above the ledger).
- **Dogfood loop (M1).** Concurrent multi-agent writes to a shared ledger,
  repo-local MCP config, plugin actor-prefix conventions, the `docs/DOGFOOD.md`
  operations guide, and foundational VISION / PRINCIPLES / STRATEGY decisions
  seeded into the ledger.
- **Verification recorded in the ledger itself.** The dogfood loop is marked
  operational by two distinct agents capturing real decisions, verified end to
  end — HiveMind used by the team building it.

### Notes
- Storage is local-first SQLite today. A Postgres-backed multi-tenant service is
  the M2 direction (see `docs/REMOTE_DB.md`, `docs/MULTI_TENANCY.md`).
