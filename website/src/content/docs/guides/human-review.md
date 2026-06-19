---
title: Human Review
description: Review, accept, reject, or supersede agent decisions.
---

HiveMind lets humans review agent decisions using the same write primitives they
use for their own choices. Review events are explicit, traceable writes — not a
separate approval workflow bolted on top.

## Review flow

```bash
hivemind --actor human:lead --hivemind-dir ./hivemind review \
  --actor 'agent:*' \
  --since 7d \
  --unreviewed-only
```

This opens an interactive terminal flow that reads candidate decisions through
the deterministic `query recent` path, then presents each one for a decision:

| Action | What HiveMind writes |
|--------|---------------------|
| **Approve** | `decision.accepted` by the reviewer |
| **Disagree** | `decision.rejected` with the reason |
| **Supersede** | `decision.proposed` (replacement) + `decision.superseded` |
| **Skip** | Nothing — no write, no implicit approval |

Reviewed/unreviewed state is derived from the presence of explicit accept, reject,
or supersede events from reviewer-authored actors. There is no separate review flag
or review event type.

## Disagreement

When a reviewer disagrees with an agent decision, HiveMind does not overwrite or
delete the agent's decision. Both positions stay in the graph:

- The agent's original decision exists with its `proposed` status
- The reviewer's `decision.rejected` event creates a `REJECTED_BY` edge
- Queries now surface the decision as `contested`
- `contested` is a first-class status — not an error, not a fallback

To resolve a contested decision, explicitly supersede with a new decision:

```bash
hivemind --actor human:lead --hivemind-dir ./hivemind emit decision.proposed \
  --title "Use Postgres instead of SQLite for shared backend" \
  --rationale "Agent decision assumed single-node; requirements changed" \
  --supersedes decision:abc123 \
  --topic-keys architecture,storage \
  --options postgres,sqlite \
  --chose postgres
```

## Finding unreviewed agent decisions

```bash
# All unreviewed agent decisions in the last 7 days
hivemind --hivemind-dir ./hivemind query recent \
  --actor 'agent:*' \
  --since 7d \
  --unreviewed-only

# Search by topic
hivemind --hivemind-dir ./hivemind query search_decisions \
  --topic architecture \
  --actor 'agent:*'
```

## Governance at scale

The governing principle: humans retain governance over agentic work because they can
see, query, and contest every agent decision after the fact — at the speed of a query,
not the speed of reading a transcript.

This means:
- You do not need to watch agents work in real time
- You do not need to read agent transcripts to understand what was decided
- You can query by topic, time window, or actor pattern
- Disagreement, once expressed, stays in the graph and propagates to dependent decisions
