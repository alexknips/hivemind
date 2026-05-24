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
ranking, recommendations, and other smart behavior belong here in slice 2 or
later, outside the write and query paths.

## Surface Uniformity

All external surfaces — CLI, MCP, future API or client library — are thin
wrappers over the same internal functions in the `commands` and `queries`
modules. There is no behavior that exists in one surface but not another. When
a new operation is added, it is added once in the internal layer and exposed
identically through every surface.

This commitment prevents surfaces from drifting apart. It also makes the
internal layer the only place where business invariants are enforced —
validation, provenance, supersession rules, multi-tenant scoping (see
[`MULTI_TENANCY.md`](MULTI_TENANCY.md)) — so adding a new transport cannot
accidentally bypass a rule.

The choice of transport (CLI for humans at terminals, MCP for agents, future
HTTP/library for embedded use) is independent of what HiveMind does. The
choice does not change the rules. The principles (`PRINCIPLES.md`) constrain
what HiveMind does; this section names how surfaces relate to that.

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

Repeated projection is intentionally idempotent for slice 1: graph state can be
wiped and rebuilt from SQLite without changing query answers.

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

SQLite is the storage backend for the slice-1 local prototype. It is a
temporary choice. The long-term storage backend is open and will be decided
under [STRATEGY.md → Shared backend](../STRATEGY.md#shared-backend). Any future
backend must preserve [auditability](../PRINCIPLES.md#2-every-state-change-is-auditable)
in full; the SQLite implementation simply happens to do so via event-sourcing
today.

Code paths that depend on SQLite specifics (FTS5 search, SQLite pragmas,
file-on-disk semantics) should be reachable only through the internal commands
and queries layer, never from CLI, MCP, or API surfaces — so that swapping
storage replaces only one layer.

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
shell out, future HTTP and library bindings will be valid for embedded use, and
all of them go through the same internal functions per the *Surface Uniformity*
commitment above.

This is an architectural decision so future agent integrations don't assume MCP
is mandatory.

## Slice 1 Storage

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

Production/default slice-1 behavior after this change is:

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

## Open Slice 2 Decisions

- The shared remote database/service architecture and rollout.
- Whether the first remote projection is Postgres tables, Neo4j, another graph
  service, or a parity-tested combination.
- Multi-organization identity, auth, and signing.
- Pagination and response continuation beyond slice-1 defensive limits.
- Compactification, similarity, ranking, and other layer-3 behavior.
- How to represent concurrent supersessions and richer decision dependencies.
