---
title: Auth Model
description: How actors, provenance, sessions, and multi-tenancy work in HiveMind.
---

## Actors

Every HiveMind event carries a mandatory `actor_id`. Anonymous writes are rejected.

**Actor format:**

| Kind | Format | Example |
|------|--------|---------|
| Human | `human:<identifier>` | `human:alice`, `human:alice@example.com` |
| Agent | `agent:<tool>:<session>` | `agent:claude:sess_abc123` |
| System | `system:<name>` | `system:ci`, `system:importer` |

The actor's *kind* — human, agent, system — never grants or revokes privilege in the
write or query paths. An agent's decision and a human's decision have the same shape,
the same review surface, and the same standing in the graph.

This is the principle that makes governance possible: humans can review agent decisions
because the agent's decisions are first-class, not second-class.

## Provenance fields

Beyond `actor_id`, each event carries:

| Field | Description |
|-------|-------------|
| `source` | `human` or `agent` — how the write was made |
| `source_ref` | Tool that made the write (e.g., `claude-code`, `mcp`, `cli`) |
| `session_id` | Per-session identifier for grouping related writes |
| `event_origin` | Ledger offset of the event that created this node/edge |

`event_origin` is what makes the graph auditable: every node and edge traces back to a
specific, immutable event in the ledger. Anyone can replay the ledger and get the same
graph.

## Multi-tenancy

HiveMind supports multiple tenants in a shared backend. Each organization has its own
ledger scope; events from one tenant cannot be read or written by another.

The local single-user prototype has no tenant enforcement — all events go to the same
ledger. When a shared backend is deployed, the tenant model is enforced at the
`commands` and `queries` layers, not the transport layer.

## Cryptographic signing (optional)

Ed25519 signing is planned for the multi-organization deployment direction. When a
multi-organization deployment lands, signing becomes mandatory. For now, it is an
opt-in that allows clients to verify that an event was written by a specific key.

## Setting the actor

**CLI:**
```bash
hivemind --actor human:alice emit decision.proposed ...
```

**Environment variable:**
```bash
export HIVEMIND_ACTOR=human:alice
hivemind emit decision.proposed ...
```

**Fallback chain (if `--actor` is omitted):**
1. `HIVEMIND_ACTOR` environment variable
2. `human:<git config user.email>`
3. `human:<local username>`

**MCP server:**
Every capture tool call requires an explicit `actor_id` parameter. The server records
`source=agent` and generates a `source_ref` from the session. Pass `--session-id` to
override the generated session identifier.
