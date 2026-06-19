---
title: MCP Tools
description: Reference for all tools exposed by the HiveMind MCP server.
---

The HiveMind MCP server exposes seven tools. All write tools require an explicit
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

### `dump_graph`

Export the full projected graph in DOT or JSON format.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `format` | string | — | `dot` (default) or `json` |

**Returns:** DOT string or JSON graph object.

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
