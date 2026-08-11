---
title: MCP Tools
description: Reference for all 14 tools exposed by the HiveMind MCP server.
---

The HiveMind MCP server exposes 14 tools. All write tools require an explicit
`actor_id`. All read tools return JSON responses.

See [MCP Setup](/guides/mcp-setup/) to configure your client.

---

## Write tools

### `capture_decision`

Capture a decision to the ledger. Equivalent to `emit decision.proposed`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string | ✓ | Actor making this decision. Format: `agent:<tool>:<session>` |
| `title` | string | ✓ | Short, imperative title for the decision |
| `rationale` | string | ✓ | Why this option was chosen |
| `topic_keys` | string[] | — | Topic labels for search and grouping |
| `options` | string[] | — | Alternatives that were considered |
| `chosen` | string | — | The option that was chosen |
| `supersedes_id` | string | — | ID of a decision this supersedes |

**Returns:**

```json
{
  "decision_id": "decision:abc123",
  "status": "proposed"
}
```

---

### `capture_evidence`

Record an evidence node. Equivalent to `emit evidence.recorded`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string | ✓ | Actor recording this evidence |
| `content` | string | ✓ | The evidence content |
| `url` | string | — | Source URL |
| `for_decision_id` | string | — | Decision this evidence supports |

---

### `capture_hypothesis`

Record a hypothesis still in flight. Equivalent to `emit hypothesis.recorded`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string | ✓ | Actor stating this hypothesis |
| `statement` | string | ✓ | The hypothesis statement |
| `for_decision_id` | string | — | Decision this hypothesis underlies |

---

### `disagree_decision`

Contest a decision as an actor. Records a disagreement edge on the decision.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string | ✓ | Actor registering the disagreement |
| `decision_id` | string | ✓ | ID of the decision to contest |
| `rationale` | string | ✓ | Why the actor disagrees |

**Returns:** The decision node with `status` updated to `contested`.

---

### `supersede_decision`

Supersede a prior decision with a new one. Creates a `SUPERSEDES` edge between them.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `actor_id` | string | ✓ | Actor making the new decision |
| `title` | string | ✓ | Title of the new decision |
| `rationale` | string | ✓ | Why this supersedes the prior decision |
| `supersedes_id` | string | ✓ | ID of the decision being superseded |
| `topic_keys` | string[] | — | Topic labels for the new decision |
| `options` | string[] | — | Alternatives that were considered |
| `chosen` | string | — | The option that was chosen |

**Returns:**

```json
{
  "decision_id": "decision:new123",
  "supersedes": "decision:old456",
  "status": "proposed"
}
```

---

## Read tools

### `get_decision`

Retrieve a specific decision by ID with its current derived status.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `id` | string | ✓ | Decision ID (e.g., `decision:abc123`) |

**Returns:** Full decision node including `status`, `actors`, `evidence`, `options`,
`supersedes`, `superseded_by`, and `hypothesis_refuted`.

---

### `get_relevant_decisions`

Search decisions by topic, status, actor, or time window.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `topic` | string | — | Topic key to filter by |
| `status` | string | — | `proposed`, `accepted`, `contested`, or `superseded` |
| `actor` | string | — | Actor pattern (supports `agent:*` glob) |
| `since` | string | — | Duration string: `7d`, `30d`, `1h` |
| `limit` | integer | — | Max results (default: 20) |
| `offset` | integer | — | Pagination offset |

**Returns:**

```json
{
  "decisions": [...],
  "truncated": false,
  "total": 12
}
```

When `truncated` is `true`, increment `offset` by `limit` to fetch the next page.

---

### `get_supersession_chain`

Walk the full supersession chain backward from a given decision.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `id` | string | ✓ | Starting decision ID |

**Returns:** Ordered list of decisions from newest to the original proposal.

---

### `search_decisions`

Full-text search across the decision ledger.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | ✓ | Search terms |
| `limit` | integer | — | Max results (default: 20) |
| `offset` | integer | — | Pagination offset |

**Returns:**

```json
{
  "decisions": [...],
  "truncated": false,
  "total": 5
}
```

---

### `recent_decisions`

List recently proposed decisions within a time window.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `since` | string | ✓ | RFC3339 lower bound for proposal time |
| `until` | string | — | RFC3339 upper bound for proposal time |
| `actor` | string[] | — | Actor id patterns |
| `topic` | string[] | — | Topic key filters |
| `status` | string[] | — | `proposed`, `accepted`, `rejected`, `contested`, or `superseded` |
| `source` | string[] | — | Source filters |
| `limit` | integer | — | Max results (max 1000) |
| `cursor` | string | — | Pagination cursor |

**Returns:** List of decision nodes in reverse-chronological order.

---

### `dump_graph`

Export the full projected graph in DOT or JSON format.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `format` | string | — | `dot` (default) or `json` |

**Returns:** DOT string or JSON graph object.

---

### `hivemind_compact_view`

Return a compact, human-readable summary of a decision and its immediate context
(evidence, hypotheses, supersession chain). Useful for agents that need a quick
read on a specific decision without walking the full graph.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `id` | string | ✓ | Decision ID to summarize |

**Returns:** Compact JSON object with decision, linked evidence, hypotheses, and
chain summary.

---

### `summarize_decisions`

Return an LLM-friendly natural language summary of a set of decisions matching
the given filter criteria. Useful for giving agents a high-level picture before
they query specifics.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `topic` | string | — | Topic key to scope the summary |
| `status` | string | — | Filter by status |
| `limit` | integer | — | Max decisions to include (default: 20) |

**Returns:** A structured summary string suitable for injection into agent context.

---

### `recall_decisions`

Layer-3: search for decisions matching a query and return them ranked alongside
a concise text digest. One call answers "what was decided about X?". The rank
comes from FTS scoring (ordinal, not a confidence score). The digest is
deterministic template rendering sourced from decision fields only.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `q` | string | — | Free-text search query |
| `topic` | string[] | — | Filter by topic keys |
| `status` | string[] | — | `proposed`, `accepted`, `rejected`, `contested`, or `superseded` |
| `actor_id` | string[] | — | Actor id filters |
| `source` | string[] | — | Source filters |
| `since` | string | — | RFC3339 lower bound for proposal time |
| `until` | string | — | RFC3339 upper bound for proposal time |
| `limit` | integer | — | Max results to return and summarize (default 5, max 10) |
| `cursor` | string | — | Pagination cursor |

**Returns:**

```json
{
  "query": "...",
  "ranked": { "items": [...], "total_matches": 3, "truncated": false },
  "digest": { "summary": "...", "cited_decision_ids": ["decision:abc123"] }
}
```

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
