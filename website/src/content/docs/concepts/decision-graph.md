---
title: Decision Graph
description: The five node types, typed edges, and how status is derived.
---

HiveMind's ledger is projected into a graph. The graph has five node types and several
typed edge kinds. Status is always derived from edges — never stored, never overwritten.

## Node types

| Type | Description |
|------|-------------|
| `Decision` | A specific choice made by a specific actor |
| `Actor` | A human, agent, or system that took an action |
| `Evidence` | A factual observation the decision rests on |
| `Option` | An alternative considered but not chosen |
| `Hypothesis` | A belief still in flight — not yet refuted or confirmed |

## Edge kinds

| Edge | Meaning |
|------|---------|
| `PROPOSED_BY` | Actor who proposed the decision |
| `ACCEPTED_BY` | Actor who accepted the decision |
| `REJECTED_BY` | Actor who rejected the decision |
| `SUPERSEDES` | This decision replaces an older one |
| `ASSUMES` | Decision depends on a hypothesis being true |
| `SUPPORTED_BY` | Decision is supported by this evidence |
| `OPTION_OF` | Option was considered for this decision |

## Status derivation

Status is derived from the edges present on a decision node:

| Status | Condition |
|--------|-----------|
| `proposed` | No `ACCEPTED_BY` or `REJECTED_BY` edge; not superseded |
| `accepted` | At least one `ACCEPTED_BY` edge; no active rejection |
| `contested` | Both `ACCEPTED_BY` and `REJECTED_BY` edges exist from different actors |
| `superseded` | A newer decision has a `SUPERSEDES` edge pointing here |

`contested` is a first-class status. Two actors disagreeing is the signal, not an error
to resolve silently. Both positions stay in the graph, queryable, reviewable, and
eventually resolvable through explicit action.

## Staleness propagation

When a `Hypothesis` is refuted, every `Decision` that `ASSUMES` it surfaces
`hypothesis_refuted: true` in queries. Staleness is visible by default — not hidden.

## Supersession chains

When one decision supersedes another, the old decision is not deleted or mutated.
A new decision carries a `SUPERSEDES` edge. You can walk the full chain backward to the
original proposal with:

```bash
hivemind query get_supersession_chain --id decision:abc123
```

## Example graph

```
Decision: "Use SQLite for local prototype"
  PROPOSED_BY → Actor: human:alice
  ACCEPTED_BY → Actor: human:alice
  SUPPORTED_BY → Evidence: "SQLite WAL is sufficient for current local writes"
  ASSUMES → Hypothesis: "Single-node deployments are the primary case for 2026"
  OPTION_OF → Option: "Postgres"
  OPTION_OF → Option: "DuckDB"
```

If the hypothesis is later refuted, the decision surfaces `hypothesis_refuted: true`.
If a new decision supersedes this one, this one's status becomes `superseded` — and
its full history remains queryable forever.
