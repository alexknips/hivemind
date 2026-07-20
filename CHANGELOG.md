# Changelog

All notable changes to HiveMind are documented here.
This project adheres to [Semantic Versioning](https://semver.org/).

## v0.4.0 — 2026-07-20 — M4: Self-hosting GA, keyless capture, and Ingestion v1

Organizational self-hosting exits beta: per-user bearer tokens, a
compose-first deployment cell, WorkOS AuthKit OAuth for the hosted MCP
gateway, and self-host polish bring the self-hosted path to GA readiness.
Keyless capture (no `ANTHROPIC_API_KEY` on the server) ships via a
subscription-seat Worker A path. Ingestion v1 lands as **experimental**
connectors: a `Connector` trait, `GitFileConnector`, `GoogleDocsConnector`
(OAuth), and a same-as/dedup layer with same-as review commands. Layer-3
gains a 2-axis decision scorer and a semantic spectral decision map; the CLI
adds `recall` and `digest` subcommands; the MCP gateway adds HTTP Streamable
transport and a `recall_decisions` tool. Fidelity measurement matures via a
36-case gold corpus, A/B harness, and Tier 1.5 synthetic dispersion cases.

Prebuilt binaries ship for **linux-x86_64, linux-arm64, and macos-arm64**.
Intel macOS (`x86_64-apple-darwin`) is not currently published as a prebuilt
— the ONNX runtime that backs the spectral map ships no prebuilt for that
target — so Intel Mac users install via `cargo install --git`.

### Added

#### Self-hosting and auth
- **Per-user bearer tokens** for self-hosted cells. Each provisioned user
  receives an `hm_tk_<64-hex>` token validated against a per-cell token
  store. (`src/ledger/postgres/tenant_store.rs`, `src/ledger/sqlite/mod.rs`,
  hivemind-iioq, f57dd30)
- **Self-hosted cell compose wiring.** `docker-compose.yml` ships a
  production-ready cell configuration — env surface, volume mounts, and
  health-check; `docs/SELF_HOSTING.md` is the authoritative runbook.
  (hivemind-ppw3, 5780e7a)
- **WorkOS AuthKit OAuth.** The hosted MCP gateway accepts WorkOS-issued
  JWTs, enabling browser-based GitHub/Google login for MCP clients.
  (`src/api.rs`, hivemind-k6hv, 74d7ec0)
- **CORS layer**, opt-in. Browser SPA clients can call `/v1/` directly when
  origins are set via `HIVEMIND_CORS_ORIGINS`; off by default, so
  self-hosters are unaffected unless they enable it. (hivemind-2l8h, 936a3d7)
- **`hivemind digest` command.** Generates a weekly decision-digest summary
  for a given time window. (`src/cli/mod.rs`, `src/summarize.rs`,
  hivemind-k125, d798df7)
- **`hivemind migrate` CLI**, behind the `shared-backend-postgres` build
  feature (off by default). Migrates a local SQLite ledger to a remote
  Postgres-backed shared backend. (hivemind-uuq9.8, d4b7e7b)
- **SPA static serving + `GET /v1/graph`.** The server serves the SPA demo
  bundle and exposes `GET /v1/graph`, which returns the full decision graph
  as JSON (nodes + edges, no coordinates). The 2-D spectral *layout* is a
  separate endpoint — see the decision map below. (98fb8a7)

#### Keyless capture (no server-side ANTHROPIC_API_KEY)
- **Worker A — subscription-seat keyless path.** `hivemind classify-queue`
  CLI and capture plugin drain unclassified batches using the subscription
  seat's API key, not the server's. Enables zero-API-key self-hosted
  deployments. (hivemind-v8im, 731d367)
- **Haiku edge classifier subagent.** The capture path (`hivemind emit`
  plus the capture plugin/skill) can classify graph edges (actor, evidence,
  option relationships) at capture time via a local Haiku call rather than
  the server-side classifier. (hivemind-mfc7, 97f869d)
- **Keyless capture docs and onboarding.** `docs/KEYLESS_CAPTURE.md` —
  zero-to-first-decision walkthrough. The self-hosting funnel (README,
  `docs/SELF_HOSTING.md`, `docs/AGENT_DECISION_CAPTURE.md`) updated to
  surface the keyless path. (hivemind-kj84, 0268e8b)

#### Ingestion v1 connectors (experimental)
> These connectors are **experimental** — the `Connector` trait and
> ingestion API are subject to change before a stabilization milestone.

- **`Connector` trait + version-walk pipeline.** Common interface for
  pull-based ingestion; `GitFileConnector` walks git history and extracts
  decision candidates per version. (`src/connector/`, hivemind-2tcl.1,
  edd2af4)
- **`GoogleDocsConnector`.** Ingests Google Docs via Drive API OAuth2 flow
  with credentials and token caching. (hivemind-2tcl.2, ced6ed9)
- **Same-as / dedup layer.** Deduplicates connector output across ingestion
  runs; `hivemind import connector same-as-candidates` /
  `confirm-same-as` / `retract-same-as` for manual review of same-as
  candidates. (hivemind-2tcl.3, 7d7269a)

#### Layer-3 intelligence
- **2-axis decision scorer.** Layer-3 scorer annotates decisions (via
  `decision.scored` events) on two axes: **Quality** [0,1], a weighted
  composite over 7 dimensions, and **Importance** (Stakes × Irreversibility
  × Actionability). Swappable — query and write layers are unaffected. Note:
  scoring calls a model and requires a server-side `ANTHROPIC_API_KEY`; it is
  distinct from the keyless *capture* path. (`src/scorer.rs`,
  hivemind-uuq9.19, 7b64b8c)
- **2-D spectral decision map.** `GET /v1/decisions/map` returns a spectral
  layout (x=time, y=Fiedler eigenmap) backed by `fastembed-rs`
  BGE-small-en-v1.5 semantic embeddings. **Currently SQLite-backend only** —
  the endpoint returns a validation error on the Postgres backend, so the map
  is not yet available on the hosted deployment. (hivemind-plvq, 36519f6 /
  0c34846)
- **`hivemind query recall` CLI.** Wraps search + summarize in one command
  for fast on-demand recall. (M3/ynt5.2, 80d74c0)
- **`recall_decisions` MCP tool.** One-call search + summarize over the MCP
  gateway. (M3/ynt5.1, 825dd9e)
- **TUI summary and compact-graph views.** `'s'` opens a summary panel;
  `'v'` toggles compact-graph mode in `hivemind query`. (hivemind-0o9e,
  4d528ec)

#### MCP and transport
- **HTTP Streamable MCP transport.** MCP tools available over the HTTP
  Streamable transport in addition to stdio, enabling web-based MCP clients.
  (hivemind-iwz9, 8448868)

#### Measurement (M4)
- **Capture-fidelity evaluator and gold corpus.** `benchmarks/fidelity/` —
  36-case gold corpus, schema-ceiling scorecard, and `--ceiling` mode.
  (hivemind-21zi, hivemind-0d2v, 7dc29e2)
- **A/B uplift harness.** `ab-eval` binary compares two classifier
  strategies on the same corpus; Phase 1 scorecard published at
  `benchmarks/fidelity/ab-eval-scorecard-v1-phase1-2026-07-13.txt`.
  (ee1798d / df894c2)
- **Tier 1.5 synthetic corpus** (G1–G3): dispersion-graded cases for
  evaluating classifier robustness across signal density.
  (hivemind-92fs, d56f799)

### Fixed
- Self-host GA polish: removed broken `--agent hivemind-classifier` call in
  `capture.sh`, added batch-capture slash command, corrected non-existent
  remote MCP flags in docs, fixed `hm_sk_live_` → `hm_tk_` token-prefix
  mismatch, added Postgres FTS note in quickstart.
  (hivemind-669v, c87a819)
- Postgres AppState build/startup panic; `extract_ctx` Postgres lookups
  wrapped in `spawn_blocking`. (hivemind-noc9, hivemind-e8zp)
- Release pipeline could publish an incomplete release. `continue-on-error`
  on the build matrix plus an `if: !cancelled()` publish gate let a release
  publish with platform artifacts missing. Publish is now gated on all
  builds succeeding, with an explicit guard that refuses to publish unless
  every expected platform tarball and checksum is present. linux-arm64
  cross-linking (`cannot find -lstdc++`) is fixed by installing the aarch64
  C++ toolchain. (hivemind-ehwd, 375f81e)
- Three `cargo-audit` advisories resolved: crossbeam-epoch, quinn-proto,
  lopdf. (hivemind-7g5o)
- Docker base image bumped to Debian Trixie (glibc 2.40) for `ort`
  pre-built ONNX compatibility. (hivemind-plvq)

### Changed
- **README install snippet.** `HIVEMIND_VERSION` example updated to
  `v0.4.0`.

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
