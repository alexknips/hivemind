# Remote Database Architecture

Status: accepted short-run recommendation, 2026-05-18.

## Recommendation

Build the next shared HiveMind persistence slice as a HiveMind service backed by
Postgres:

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

- HTTP/JSON commands and queries for UIs and non-coding clients.
- MCP or stdio wrappers as thin clients over the same service API.
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
| Postgres ledger plus Postgres projection | Yes | Best short-run fit: mature Rust clients, hosted and self-hosted options, ACID transactions, JSONB payloads, indexes, row-level security options, backups, and operational familiarity. Graph traversal is less elegant than Cypher, but HiveMind's slice-2 queries are bounded and status derivation is explicit. |
| Neo4j / Aura | Not canonical yet | Strong graph tooling, Cypher, hosted Aura, ACID graph transactions, and visualization. The Rust path is weaker than Postgres: Neo4j's official driver list does not include Rust, so Rust service code uses the HTTP Query API or unofficial Bolt crates. It also introduces a second remote system before the ledger/service contract is proven. Keep as a later `GraphView` projection candidate. |
| Memgraph | Not canonical yet | Attractive for Cypher-compatible server graph queries and streaming-style graph workloads. It should not own HiveMind writes because ledger audit, org tenancy, command validation, and non-developer auth still need the HiveMind service. Query portability with Neo4j/Kuzu is close but not identical. |
| FalkorDB | Not canonical yet | Has a first-party Rust client and simple server deployment, plus openCypher-style querying. It is a reasonable future graph projection experiment, but Redis-module operations, product lock-in, and auth/multi-tenancy shape are worse short-run fits than one Postgres-backed service. |
| SQLite plus Kuzu | Local only | This is still the fastest developer/prototype path. It is embedded and file-local, which is exactly why it is wrong as the shared remote source of truth for humans and agents. |
| Direct DB clients from agents | No | This would bypass command invariants, make audit/auth inconsistent, and expose projection internals as public API. |

## Local To Remote Mapping

Current slice-1 storage remains useful:

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

## Follow-Up Beads

1. `feature/P0`: Add Postgres migrations for `events`, `actors`, typed graph
   projection tables, edges, and projection checkpoint metadata.
2. `feature/P0`: Implement `PostgresEventLedger` with idempotent append,
   ordered reads, replay, and transaction tests.
3. `feature/P0`: Implement transactional Postgres projector and query parity
   tests against the current Kuzu path.
4. `feature/P0`: Add HiveMind HTTP service that wraps commands and queries,
   including actor authentication and org scoping.
5. `feature/P1`: Add CLI remote mode and keep local SQLite/Kuzu mode as the
   five-minute onboarding path.
6. `task/P1`: Add query pagination, `truncated`, and concurrent supersession
   fixtures before exposing multi-user remote queries.
7. `task/P1`: Spike Neo4j and FalkorDB `GraphView` projections only after the
   Postgres-backed service passes parity and concurrency tests.

## Checked Inputs

Reference facts checked on 2026-05-18: Neo4j official docs list supported
drivers and the HTTP Query API; FalkorDB docs list `falkordb-rs`; Kuzu docs
describe an embedded in-process graph database; Postgres docs cover JSONB
indexing, transaction isolation, and row-level security.
