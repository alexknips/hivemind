# Remote Database Architecture

Status: shipped as of M2. This document records the architecture decision and
what was implemented. The short-run recommendation below was accepted and the
core of it has landed: HTTP REST API, Postgres backend with RLS, bearer-auth,
and multi-tenant provisioning.

The tenant model is defined in [`MULTI_TENANCY.md`](MULTI_TENANCY.md). References
to `org_id` below are storage-level tenant keys, not a separate identity model.

## Recommendation

Build the next shared HiveMind persistence layer as a HiveMind service backed
by Postgres:

- Postgres is the canonical remote event ledger.
- Postgres also holds the first shared graph projection in typed node and edge
  tables.
- All agents, CLIs, integrations, and future UIs call the HiveMind service. They
  do not write directly to the database.
- SQLite and Kuzu remain the local prototype and developer projection path, not
  shared production infrastructure.

This preserves the core contract: decisions are immutable events first, graph
state is a rebuildable projection, and query/read APIs stay deterministic.

## Canonical Model

HiveMind needs both an event ledger and a graph projection.

The event ledger is authoritative because auditability is the product. Every
write appends one event with `actor_id`, `event_uuid`, `correlation_id`,
optional `causation_event_id`, typed JSON payload, server timestamp, and later
`schema_version` and signature fields. The ledger table should use
`UNIQUE (org_id, event_uuid)` for idempotent retries.

The graph projection is derived state. It contains `Actor`, `Decision`,
`Evidence`, `Option`, `Hypothesis`, and the current relation set. Every projected
node and edge carries `org_id` and `event_origin`, pointing back to the ledger
event that created it. Decision and hypothesis status remain derived from edges
at query time.

For the short run, projection should happen inside the same Postgres transaction
as event append:

1. The service authenticates the caller and resolves an `actor_id`.
2. The commands layer validates invariants.
3. A transaction appends the event.
4. The projector mutates typed projection tables and stores `event_origin`.
5. The transaction commits and returns `event_id`.

A rebuild path must also exist: wipe projection tables, replay events in
`event_id` order, and compare query results against the live projection.

## Concurrency

Do not add uniqueness constraints that collapse disagreement.

- `decision.accepted` and `decision.rejected` remain additive events.
- A single actor cannot both accept and reject the same decision.
- Different actors can disagree; queries surface `contested`.
- Concurrent supersessions of the same old decision both append. Queries detect
  more than one incoming `SUPERSEDES` edge and surface that as concurrent
  supersession, not data corruption.
- Commands that need optimistic protection may pass an expected observed
  `event_id`; mismatch records or returns a conflict, but never deletes the
  competing event.

## API Boundary

The service owns writes, reads, auth, tenancy, and schema migration.

Expose:

- HTTP/JSON commands and queries (`/v1/*`) for UIs and non-coding clients.
  Shipped in `src/api.rs`.
- MCP or stdio wrappers as thin clients over the same service API. Shipped as
  `clients/mcp-gateway/` (TypeScript stdio gateway). See
  [`MCP_SERVICE_SPLIT.md`](MCP_SERVICE_SPLIT.md).
- Transcript ingest from agents via `POST /v1/ingest`. Shipped capture clients:
  `capture/hook_ship.py` (Claude Code hook) and `capture/sidecar.py` (poll
  daemon for hook-less harnesses).
- CLI remote mode that calls the service instead of opening local files.

Keep private:

- Database credentials.
- Direct projection writes.
- Admin-only replay, migration, and repair operations.

This boundary matters because non-developers and non-Codex agents need a stable
product API, not database credentials or a git/file synchronization workflow.

## Backend Comparison

| Backend | Use now? | Reason |
| --- | --- | --- |
| Postgres ledger plus Postgres projection | Yes | Best short-run fit: mature Rust clients, hosted and self-hosted options, ACID transactions, JSONB payloads, indexes, row-level security options, backups, and operational familiarity. Graph traversal is less elegant than Cypher, but HiveMind's shared-backend queries are bounded and status derivation is explicit. |
| Neo4j / Aura | Not canonical yet | Strong graph tooling, Cypher, hosted Aura, ACID graph transactions, and visualization. The Rust path is weaker than Postgres: Neo4j's official driver list does not include Rust, so Rust service code uses the HTTP Query API or unofficial Bolt crates. It also introduces a second remote system before the ledger/service contract is proven. Keep as a later `GraphView` projection candidate. |
| Memgraph | Not canonical yet | Attractive for Cypher-compatible server graph queries and streaming-style graph workloads. It should not own HiveMind writes because ledger audit, org tenancy, command validation, and non-developer auth still need the HiveMind service. Query portability with Neo4j/Kuzu is close but not identical. |
| FalkorDB | Not canonical yet | Has a first-party Rust client and simple server deployment, plus openCypher-style querying. It is a reasonable future graph projection experiment, but Redis-module operations, product lock-in, and auth/multi-tenancy shape are worse short-run fits than one Postgres-backed service. |
| SQLite plus Kuzu | Local only | This is still the fastest developer/prototype path. It is embedded and file-local, which is exactly why it is wrong as the shared remote source of truth for humans and agents. |
| Direct DB clients from agents | No | This would bypass command invariants, make audit/auth inconsistent, and expose projection internals as public API. |

## Local To Remote Mapping

Current local storage remains useful:

- `SqliteEventLedger` stays the zero-ops local ledger.
- `KuzuGraph` stays the local graph projection and query parity target.
- New `PostgresEventLedger` implements the same ledger trait.
- New `PostgresGraphView` implements the same projection/query trait, using SQL
  over typed node and edge tables.
- The CLI defaults to local mode for quick onboarding and supports remote mode
  with `HIVEMIND_URL` plus an actor token.

Remote mode must treat the service as authoritative. Local SQLite/Kuzu copies
may cache, replay, or debug remote events, but they are not shared truth.

## Non-Developer UX Implications

The service must model identity and permissions from the start:

- Actors include humans, agents, services, and system jobs.
- Organizations/workspaces scope every event and projection row.
- Human users authenticate through normal product auth; agents use scoped
  tokens tied to `Actor` records.
- UI and API responses show who proposed, accepted, rejected, superseded, or
  contested a decision.
- Audit views read the ledger, not chat logs or git history.
- Shared decision history is visible immediately after commit; no pull/push
  mental model is exposed to users.

## Checked Inputs

Reference facts checked on 2026-05-18: Neo4j official docs list supported
drivers and the HTTP Query API; FalkorDB docs list `falkordb-rs`; Kuzu docs
describe an embedded in-process graph database; Postgres docs cover JSONB
indexing, transaction isolation, and row-level security.

## Reconsidered: Dolt for the Shared Graph

Status: noted on 2026-06-06. Not adopted; revisit when the Postgres path hits
specific scaling failure modes (see triggers below).

Dolt — a SQL database with git-style branching, cell-level merge, and native
sync — keeps surfacing as an attractive alternative for the shared graph:

- Decision capture is increasingly moving toward batched, classifier-driven
  emission (see beads for the auto-detect classifier subagent). Dolt's
  branch-and-merge semantics would let batches of captured events from many
  agents reconcile cleanly rather than serializing through a single writer.
- The merge characteristics also fit a future world where agents capture
  locally, push when network allows, and resolve conflicts at merge time
  instead of holding a connection open.

### Why we are not switching now

The original commitment recorded against this question — "every decision done
is captured" — pushed us toward an immediate-write model. Postgres serves that
model directly: one canonical event log, immediately visible, no branch
reconciliation step between capture and visibility.

The current investment (Postgres event ledger landed under
`hivemind-m2-shared-backend-lives-uuq9.3`, projection in flight under `.4`) is
the path we are deepening. Switching backends mid-milestone destabilizes the
M2 acceptance criteria and pushes the local-to-remote migration story (`.8`)
into a less-defined shape.

### Trigger points for revisiting

Re-open this question if and only if one of these is observed:

1. Postgres write contention measurably blocks the classifier-driven capture
   throughput at the scale of N concurrent agents (N to be discovered through
   load testing, not guessed up front).
2. Offline / partition tolerance becomes a real user need — agents that must
   capture during network loss and reconcile later — and the merge story under
   Postgres requires custom application-layer logic Dolt would give for free.
3. Cross-tenant or cross-org merge ("import this org's decision history into
   ours, reconcile conflicts") becomes a roadmap item.

### Posture

Optimize the Postgres path for extreme scalability and see where it breaks.
The breakage points define the next decision, with empirical inputs that a
Dolt-vs-Postgres comparison made today would lack.
