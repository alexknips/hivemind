---
title: Decision Graph
description: The four node types, typed edges, and how status is derived.
---

HiveMind's ledger is projected into a graph. The graph has four content node types and several
typed edge kinds. Status is always derived from edges — never stored, never overwritten.

## Node types

| Type | Description |
|------|-------------|
| `Decision` | A specific choice made by one or more actors |
| `Option` | An alternative that was considered |
| `Evidence` | A factual observation the decision rests on |
| `Hypothesis` | A belief still in flight — not yet refuted or confirmed |

`Actor` (a human, agent, or system) is provenance metadata — every node and edge carries an
actor as attribution, but `Actor` is not a content node in the decision graph.

## Edge kinds

| Edge | From → To | Meaning |
|------|-----------|---------|
| `PROPOSED_BY` | Decision → Actor | Actor who proposed the decision |
| `ACCEPTED_BY` | Decision → Actor | Actor who accepted the decision |
| `REJECTED_BY` | Decision → Actor | Actor who rejected the decision |
| `SUPERSEDES` | Decision → Decision | This decision replaces an older one |
| `HAS_OPTION` | Decision → Option | An option that was considered |
| `CHOSE` | Decision → Option | The option that was selected |
| `BASED_ON` | Decision → Evidence | Evidence the decision rests on |
| `PREMISED_ON` | Option → Hypothesis | Hypothesis the chosen option depends on being true |
| `PREMISED_ON_DIRECT` | Decision → Hypothesis | Hypothesis a decision depends on directly (not via an option) |
| `SUPPORTS` | Evidence → Hypothesis | Evidence that corroborates a hypothesis |
| `REFUTES` | Evidence → Hypothesis | Evidence that contradicts a hypothesis |

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

When a `Hypothesis` is refuted, every `Decision` premised on it (through a chosen option's
`PREMISED_ON` edge or a direct `PREMISED_ON_DIRECT` edge) surfaces `hypothesis_refuted: true`
in queries. Staleness is visible by default — not hidden.

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
  BASED_ON    → Evidence: "SQLite WAL is sufficient for current local writes"
  HAS_OPTION  → Option: "Postgres"
  HAS_OPTION  → Option: "DuckDB"
  HAS_OPTION  → Option: "SQLite"
  CHOSE       → Option: "SQLite"
        PREMISED_ON → Hypothesis: "Single-node deployments are the primary case for 2026"
```

If the hypothesis is later refuted, the decision surfaces `hypothesis_refuted: true`.
If a new decision supersedes this one, this one's status becomes `superseded` — and
its full history remains queryable forever.
