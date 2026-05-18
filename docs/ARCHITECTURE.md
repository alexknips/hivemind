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

## Kuzu Build Cost And Blocker

The Rust `kuzu` crate builds bundled native C++ code. Developers should run
default `cargo test` for routine work. Run `cargo test --features graph-kuzu`
only when changing the Kuzu adapter or explicit Kuzu CLI path.

Current blocker: `hivemind-oj7qr` tracks a link failure in this environment.
`cargo test --features graph-kuzu kuzu -- --nocapture` compiles the bundled Kuzu
C++ code, then `rust-lld` fails on missing cxxbridge symbols from `kuzu 0.11.3`
such as `PreparedStatement$isSuccess`, `Value$get_value_i64`, and
`QueryResult$getNext`. Until that is fixed, Kuzu runtime tests cannot execute.

## Open Slice 2 Decisions

- The shared remote database/service architecture and rollout.
- Whether the first remote projection is Postgres tables, Neo4j, another graph
  service, or a parity-tested combination.
- Multi-organization identity, auth, and signing.
- Pagination and response continuation beyond slice-1 defensive limits.
- Compactification, similarity, ranking, and other layer-3 behavior.
- How to represent concurrent supersessions and richer decision dependencies.
