---
title: Architecture
description: The three-layer design that keeps HiveMind trustworthy at scale.
---

HiveMind has three layers. Each does exactly one job. None of them bleed into each other.
This boundary is not a diagram in a doc — it is a property of the codebase.

## Layer 1: Write

The write layer validates invariants and appends events to the ledger. It is intentionally dumb.

**What it does:**
- Validates actor format (no anonymous writes)
- Validates topic keys, option lists, supersession targets
- Appends events with UUID, timestamp, and actor provenance
- Projects explicit state (node creation, edge creation)

**What it never does:**
- Call an LLM
- Run similarity or deduplication
- Make decisions about what the event means
- Read from layer 3

A function that crosses into layer 2 or 3 is a bug.

## Layer 2: Query

The query layer reads the projected graph and derives status from explicit edges.

**What it does:**
- Returns decisions, evidence, hypotheses, actors, options
- Derives `proposed`, `accepted`, `contested`, `superseded` from graph relations
- Paginates results and returns `truncated: true` when limits are hit
- Walks supersession chains deterministically

**What it never does:**
- Call an LLM
- Rank, score, cluster, summarize, or infer
- Write to the ledger

A query that calls an LLM is a bug.

## Layer 3: Agentic

All intelligence lives here and only here. This layer is deliberately separate so it can be
A/B-tested, swapped, or removed without touching ingest or queries.

**What it does:**
- Compactification (removing safely-redundant events)
- Similarity search and ranking
- Capture classification (signal vs. noise)
- Document candidate extraction

**Key property:** The rest of the system must remain functional and correct without layer 3.
If a smart feature requires reaching back into layers 1 or 2 from this layer, the feature
is the wrong shape — not the architecture.

## Event flow

```
Actor → emit command → [Layer 1: validate + append] → ledger.sqlite
                                                           ↓
                                               [projection: nodes + edges]
                                                           ↓
Query command → [Layer 2: derive status, paginate] → result
```

The ledger is the authority. Projection is rebuildable derived state. Queries always replay
from the ledger; there is no cached state that can drift from the events.

## Surface uniformity

All external surfaces — CLI, MCP, future API or client library — are thin wrappers over
the same internal functions in the `commands` and `queries` modules. There is no behavior
that exists in one surface but not another.

This prevents surfaces from drifting apart and keeps business invariants enforced in exactly
one place.

## Storage

The default store is SQLite at `./hivemind/ledger.sqlite`. SQLite is a deliberate
short-term choice; the long-term storage backend is open. Any implementation that preserves
complete auditability satisfies the architecture; any that doesn't, doesn't.

An optional [Kuzu](https://kuzudb.com/)-backed projection is available behind the
`--features graph-kuzu` build flag.

## Related

- [Auth Model](/concepts/auth-model/) — how actors, provenance, and multi-tenancy work
- [Decision Graph](/concepts/decision-graph/) — the five node types and typed edges
