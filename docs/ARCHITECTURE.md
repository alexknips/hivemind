# HiveMind Architecture

HiveMind records organizational decision memory: what was decided, why, by
whom, what options were considered, and what evidence or hypotheses the decision
depends on. It is not a chat archive, notes store, task tracker, or general
knowledge graph.

## Layer Boundary

HiveMind has three layers.

Layer 1 is write and ingest. The commands module validates invariants, appends
events to the ledger, and projects explicit state. It must stay dumb and
correct.

Layer 2 is query and read. Query code reads the projected graph and derives
status from explicit edges. It does not call LLMs, rank, cluster, summarize, or
invent confidence.

Layer 3 is agentic suggestion and analysis. Compactification, similarity,
ranking, recommendations, and other smart behavior belong here, outside the
write and query paths. See [`STRATEGY.md → Layer-3 capabilities`](../STRATEGY.md#layer-3-capabilities)
for the active investment direction.

## Surface Uniformity

All external surfaces — CLI, MCP, HTTP REST API, and any future library
binding — are thin wrappers over the same internal functions in the `commands`
and `queries` modules. There is no behavior that exists in one surface but not
another. When a new operation is added, it is added once in the internal layer
and exposed identically through every surface.

This commitment prevents surfaces from drifting apart. It also makes the
internal layer the only place where business invariants are enforced —
validation, provenance, supersession rules, multi-tenant scoping (see
[`MULTI_TENANCY.md`](MULTI_TENANCY.md)) — so adding a new transport cannot
accidentally bypass a rule.

The choice of transport (CLI for humans at terminals, MCP for agents, HTTP/JSON
for UIs and embedded clients) is independent of what HiveMind does. The choice
does not change the rules. The principles (`PRINCIPLES.md`) constrain what
HiveMind does; this section names how surfaces relate to that.

The `suggest` CLI namespace is the local layer-3 surface. For example,
`suggest document-candidates` can call an external document extractor or consume
an LLM response file to produce pending-review decision candidates, and
`suggest materialize-document-candidates` can turn selected candidates into
ordinary `Decision:` blocks. Neither command appends ledger events; writes still
go through explicit `emit` or `import documents` commands.

## Event To Query Flow

The event ledger is authoritative. Every append records an actor, event UUID,
typed payload, timestamp, and provenance fields. Projection is rebuildable
derived state.

Flow:

1. A caller invokes `hivemind emit ...`.
2. The commands layer validates ids, topics, disagreement rules, and
   supersession rules.
3. `SqliteEventLedger` appends the event to `ledger.sqlite`.
4. Query or dump commands rebuild a graph projection from ledger events in
   ascending event id order.
5. Query functions run deterministic graph reads and derive decision or
   hypothesis status from edges.

Repeated projection is intentionally idempotent: graph state can be wiped and
rebuilt from the ledger without changing query answers. This is the property
the [auditability principle](../PRINCIPLES.md#2-every-state-change-is-auditable)
relies on under the current state model.

## State Model (current direction, may change)

The current implementation realizes [the auditability principle](../PRINCIPLES.md#2-every-state-change-is-auditable)
via event-sourcing: the event ledger is the source of truth, and the projected
graph (plus any indexes, derived statuses, and search structures) is rebuildable
from it.

This state model is an architectural decision, not a principle. It is the path
HiveMind takes today to satisfy auditability. The model may be revisited as the
project encounters features that are awkward or unavailable under a pure
event-sourced design — for example, operations that benefit from the graph
being directly mutable as a primary store. Any future state model must preserve
auditability in full; that constraint is non-negotiable.

When the state model is reconsidered, the change belongs in this section, in
the architecture decision log, and in the principles cross-check of any bead
that depends on the current behavior.

## Storage Backend (current direction, may change)

SQLite is the storage backend for the current local prototype. It remains the
zero-service default for onboarding, tests, and local agent workflows.

The long-term shared backend is a HiveMind service backed by Postgres, per
[`REMOTE_DB.md`](REMOTE_DB.md). Postgres is the canonical remote event ledger
and also holds the first shared graph projection in tenant-scoped typed node
and edge tables. The service, not direct database clients, owns command
validation, auth, tenancy, ledger append, projection, and deterministic query
APIs.

Any storage backend must preserve
[auditability](../PRINCIPLES.md#2-every-state-change-is-auditable) in full:
events remain authoritative, projections remain rebuildable, and every
projected node or edge traces back to ledger provenance. The SQLite
implementation satisfies that contract locally; the Postgres service satisfies
it for shared multi-user and multi-agent deployments.

Code paths that depend on storage-specific behavior (SQLite FTS5 search,
SQLite pragmas, file-on-disk semantics, Postgres JSONB/index/RLS behavior,
service migration mechanics) should be reachable only through the internal
commands and queries layer, never from CLI, MCP, or API surfaces — so storage
changes replace only the backend/service layer.

## Graph Projection Backend

Kuzu is an optional persistent graph projection, not a primary store. The
default query path is in-memory graph projection from the ledger; Kuzu is
opt-in via the `graph-kuzu` feature flag and `--graph-backend kuzu` or
`HIVEMIND_GRAPH_BACKEND=kuzu`.

This is an architectural decision: the projection is *derived* state, not
source-of-truth, regardless of whether it lives in memory or in Kuzu. If the
state-model decision changes (see *State Model* above), this section may need
to change too.

## Agent Transport

MCP is one transport HiveMind exposes for agents (and for any MCP-aware client).
It is not the only one. The CLI is equally valid for agents that prefer to
shell out. The HTTP REST API (`/v1/*`) is the shared-service transport for
multi-tenant deployments, and the TypeScript MCP gateway
(`clients/mcp-gateway/`) is a thin stdio adapter over that API. All transports
go through the same internal functions per the *Surface Uniformity* commitment
above.

The `/v1/ingest` endpoint accepts transcript batches from the Python hook
shipper (`capture/hook_ship.py`) or sidecar daemon (`capture/sidecar.py`); the
server-side classifier (see [`CAPTURE_CLASSIFIER.md`](CAPTURE_CLASSIFIER.md))
runs as an optional background worker over those batches.

This is an architectural decision so agent integrations don't assume MCP or
the CLI is mandatory.

## Current Storage Layout

`./hivemind/ledger.sqlite` is the local SQLite event ledger. It is the default
source of truth for local CLI usage.

The default query backend is in-memory. `hivemind query` and `hivemind dump`
replay SQLite events into an in-process graph view. This keeps default tests and
onboarding fast and requires no native graph build.

`./hivemind/graph.kuzu` is the local persistent Kuzu projection when the binary
is built with `--features graph-kuzu` and commands are run with
`--graph-backend kuzu` or `HIVEMIND_GRAPH_BACKEND=kuzu`. In this mode, query and
dump rebuild Kuzu from the SQLite ledger before reading it.

The in-memory graph remains the fast unit-test and golden-test path. It is not a
separate product backend.

## Current Default Behavior

Current default behavior:

- `emit` writes only to SQLite.
- `query` and `dump` use the in-memory graph unless explicitly told otherwise.
- `--graph-backend kuzu` is explicit and returns a clear error unless the binary
  was compiled with `graph-kuzu`.
- Kuzu mode is a local developer projection path, not shared production
  infrastructure.

This matches the remote-store decision in `docs/REMOTE_DB.md`: SQLite and Kuzu
are local prototype storage. Shared multi-user and multi-agent deployments need
a HiveMind service with a remote canonical ledger and projection.

## Kuzu Build Cost And Native Dependencies

The Rust `kuzu` crate builds bundled native C++ code. Developers should run
default `cargo test` for routine work. Run `cargo test --features graph-kuzu`
only when changing the Kuzu adapter or explicit Kuzu CLI path.

`kuzu 0.11.3` depends on `cxx 1.0.138`, and HiveMind pins the optional
`cxx-build` dependency to the same version so Kuzu's generated bridge symbols
match the runtime bridge crate. Do not loosen that pin without running:

```bash
cargo test --features graph-kuzu kuzu -- --nocapture
```

## Open Architectural Questions

These are open architectural questions tracked under
[`STRATEGY.md`](../STRATEGY.md) fronts (mostly *Shared backend* and *Layer-3
capabilities*). They are not committed direction; specific beads will narrow
each one when picked up.

Resolved as of M2:
- The HTTP REST API as the third transport is shipped (`src/api.rs`, `/v1/*`).
- Multi-tenant Postgres backend with RLS isolation is shipped (bearer-auth
  `hm_sk_live_…` tokens, `tenant_id`-scoped Postgres RLS, `/v1/tenants`
  provisioning endpoint).
- The TypeScript MCP gateway over the service API is shipped
  (`clients/mcp-gateway/`). See [`MCP_SERVICE_SPLIT.md`](MCP_SERVICE_SPLIT.md).
- Transcript ingest path + Haiku classifier is shipped (`/v1/ingest`,
  `src/classifier.rs`, `capture/hook_ship.py`, `capture/sidecar.py`).

Still open:
- Whether Neo4j, another graph service, or a parity-tested combination becomes
  a later non-canonical `GraphView` projection.
- Token signing, rotation, and revocation beyond the initial bearer-token model.
- Pagination and response continuation beyond the current defensive limits.
- Compactification, similarity, ranking, and other layer-3 behavior.
- How to represent concurrent supersessions and richer decision dependencies.
