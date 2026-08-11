---
title: MCP Tools
description: Reference for all 14 tools exposed by the HiveMind MCP server.
---

The HiveMind MCP server exposes 14 tools. Write tools append events to the
ledger and require an explicit `actor_id`. Read tools query the graph and never
write. Layer-3 tools add ranked summaries or compact views.

See [MCP Setup](/guides/mcp-setup/) to configure your client.

---

## Write tools

### `capture_decision`

Record a proposed decision with rationale, topic keys, and at least one option. Defaults actor_id to agent:<tool>:<session> and writes source=agent.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `options` | object[] | ✓ |  |
| `rationale` | string | ✓ |  |
| `title` | string | ✓ |  |
| `topic_keys` | string[] | ✓ |  |
| `actor_id` | string | — | Optional capturing actor override. Defaults to `agent:<tool>:<session>`. |
| `chosen_option_label` | string | — | Label of the option that was accepted; must match one of `options[].label`. |
| `evidence_ids` | string[] | — |  |
| `hypothesis_ids` | string[] | — |  |

---

### `capture_evidence`

Record an evidence item that can be attached to decisions or hypotheses. Defaults actor_id to agent:<tool>:<session> and writes source=agent.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `content` | string | ✓ |  |
| `actor_id` | string | — | Optional capturing actor override. Defaults to `agent:<tool>:<session>`. |

---

### `capture_hypothesis`

Record a hypothesis. Defaults actor_id to agent:<tool>:<session> and writes source=agent.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `statement` | string | ✓ |  |
| `actor_id` | string | — | Optional capturing actor override. Defaults to `agent:<tool>:<session>`. |

---

### `disagree_decision`

Record an actor disagreement with a decision and return the resulting derived status. Wraps `hivemind disagree`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ |  |
| `reason` | string | ✓ |  |
| `actor_id` | string | — | Disagreeing actor. Defaults to `agent:codex:<session>` when omitted. |

---

### `supersede_decision`

Propose a replacement decision and mark it as superseding an old decision. Wraps `hivemind supersede`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `old_decision_id` | string | ✓ |  |
| `rationale` | string | ✓ |  |
| `title` | string | ✓ |  |
| `actor_id` | string | — | Superseding actor. Defaults to `agent:codex:<session>` when omitted. |
| `chosen_option_label` | string | — |  |
| `evidence_ids` | string[] | — |  |
| `hypothesis_ids` | string[] | — |  |
| `options` | any[] | — |  |
| `topic_keys` | string[] | — |  |

---

## Read tools

### `get_decision`

Fetch a single decision by id. Returns null when absent.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ |  |

---

### `get_relevant_decisions`

List decisions whose topic_keys contain the given topic. Optional status filter.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `topic` | string | ✓ |  |
| `status` | string | — |  |

---

### `get_supersession_chain`

Return the linear supersession chain a decision sits in, oldest first.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ |  |

---

### `search_decisions`

Full-text search over decisions. Equivalent to `hivemind query search`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string[] | — |  |
| `cursor` | string | — |  |
| `limit` | integer | — |  |
| `q` | string | — | Full-text query. |
| `since` | string | — | RFC3339 lower bound for decision proposal time. |
| `source` | string[] | — |  |
| `status` | string[] | — |  |
| `topic` | string[] | — |  |
| `until` | string | — | RFC3339 upper bound for decision proposal time. |

---

### `recall_decisions`

Layer-3: search for decisions matching a query and return them ranked alongside a concise text digest — one call answers 'what was decided about X?'. The rank comes from FTS scoring (ordinal, not a confidence score). The digest is deterministic template rendering sourced from decision fields only; every contributing decision ID is listed in digest.cited_decision_ids. Returns: { query, ranked: { items, total_matches, truncated }, digest: { summary, cited_decision_ids } }.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string[] | — |  |
| `cursor` | string | — |  |
| `limit` | integer | — | Max results to return and summarize (default 5, max 10). |
| `q` | string | — | Free-text search query. |
| `since` | string | — | RFC3339 lower bound for decision proposal time. |
| `source` | string[] | — |  |
| `status` | string[] | — |  |
| `topic` | string[] | — | Filter by topic keys. |
| `until` | string | — | RFC3339 upper bound for decision proposal time. |

---

### `recent_decisions`

List recently proposed decisions. Equivalent to `hivemind query recent_decisions`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `since` | string | ✓ | RFC3339 lower bound for decision proposal time. |
| `actor` | string[] | — | Actor id patterns, matching the CLI --actor filter. |
| `cursor` | string | — |  |
| `limit` | integer | — |  |
| `source` | string[] | — |  |
| `status` | string[] | — |  |
| `topic` | string[] | — |  |
| `until` | string | — | RFC3339 upper bound for decision proposal time. |

---

### `dump_graph`

Render the current decision graph as Graphviz DOT.

---

### `hivemind_compact_view`

Layer-3 compact view of a decision subgraph. Applies signal/noise semantics: terminal decision is fully preserved; superseded predecessors, unchosen options, and resolved blockers are elided and counted. Contested decisions are never compacted. Returns null when the decision_id is not found.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ | The decision to compact. If mid-chain, the terminal (newest) decision in the supersession chain is used as the focal node. |

---

### `summarize_decisions`

Layer-3: produce a concise text summary of one or more decisions. All content is sourced from decision record fields — no invented content. Every decision that contributed to the summary is listed in cited_decision_ids. Modes: single (one decision), cluster (multi-decision synthesis), chain (follows the supersession chain from the given decision_id).

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_ids` | string[] | ✓ | IDs of decisions to summarize (1–10). |
| `mode` | string | — | single = one decision digest; cluster = multi-decision synthesis; chain = supersession chain evolution. Defaults to single when one ID is given, cluster when multiple. |

---

## Error handling

All tools return a standard error envelope on failure:

```json
{
  "error": {
    "code": "ACTOR_REQUIRED",
    "message": "actor_id is required for all write operations"
  }
}
```

Common error codes:

| Code | Meaning |
|------|---------|
| `ACTOR_REQUIRED` | Write tool called without `actor_id` |
| `DECISION_NOT_FOUND` | ID does not exist in the ledger |
| `SUPERSESSION_CYCLE` | `supersedes_id` would create a cycle |
| `INVALID_TOPIC_KEY` | Topic key contains invalid characters |
