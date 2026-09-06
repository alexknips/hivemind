---
title: MCP Tools
description: Reference for all 21 tools exposed by the HiveMind MCP server.
---

The HiveMind MCP server exposes 21 tools. Write tools append events to the
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

### `get_decision_outcome`

Derive the outcome record for a single decision: did it hold up? Returns four quality signals — superseded (and how fast), stale premises (premised on a refuted hypothesis), contested (unresolved disagreement), thin structure (no options/evidence) — each with its contributing reasons attached. No LLM involved; derived purely from graph edges. Returns null when the decision_id is not found.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ | The decision to evaluate. |

---

### `decision_quality_candidates`

Bulk quality-signal pull for external scorers: returns outcome records for all decisions (or a filtered subset), each with the four quality signals and their contributing reasons. Designed for Mechanism A — the factory loop calls this to pull recent decisions and their signals, then defines its own scoring logic. No LLM involved.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `cursor` | string | — | Pagination cursor from a previous response's `next_cursor` field. |
| `limit` | integer | — | Maximum results to return (1–1000, default 25). |
| `only_with_signals` | boolean | — | When true, only decisions with at least one quality signal are returned. Default false. |
| `since_event_origin` | integer | — | Minimum ledger event offset (inclusive). Filter to decisions proposed at or after this offset. Use 0 or omit for all. |

---

### `get_decision_context`

Derive the context record for a single decision: the conditions under which it was made. Returns five feature groups — authorship shape (human-authored / agent-proposed+human-accepted / agent-only / unknown), source system and model/session reference, review depth (unreviewed / self_accepted / peer_reviewed / disputed), evidence and hypothesis counts, and context richness proxies (options count, rationale character count). No LLM involved; derived purely from graph edges. Pair with get_decision_outcome for causal attribution. Returns null when the decision_id is not found.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ | The decision to evaluate. |

---

### `decision_context_candidates`

Bulk context-feature pull: returns context records for all decisions (or a filtered subset), each with authorship shape, source, review depth, evidence/hypothesis counts, and rationale richness proxies. Designed to complement decision_quality_candidates — context is the independent variable side of the causal pair. No LLM involved.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `cursor` | string | — | Pagination cursor from a previous response's `next_cursor` field. |
| `limit` | integer | — | Maximum results to return (1–1000, default 25). |
| `since_event_origin` | integer | — | Minimum ledger event offset (inclusive). Filter to decisions proposed at or after this offset. Use 0 or omit for all. |

---

### `score_decision`

In-house explainable quality score for a single decision. Combines outcome signals (superseded, stale premises, contested, thin structure) with context features (authorship, review depth) into a score in [0,1] and a quality tier. ALWAYS returns the full list of contributing reasons with their deductions and contributing node IDs — never a bare number. No LLM involved; works self-hosted. Returns null when decision_id is not found.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | ✓ | The decision to score. |

---

### `scan_decision_quality`

Bulk in-house quality scan: scores all decisions (or a filtered subset) using the same explainable graph-signal engine as score_decision. Each result carries score, tier, reasons, and contributing node IDs. Designed for the scheduled quality-scan loop and for the MCP surface. No LLM involved. Precision-biased: use min_tier to surface only significant or high-concern decisions.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `cursor` | string | — | Pagination cursor from a previous response's `next_cursor` field. |
| `limit` | integer | — | Maximum results to return (1–1000, default 25). |
| `min_tier` | string | — | Only return decisions at this tier or worse. Omit for all. Use 'significant_concerns' or 'high_concern' for precision-biased alerting. |
| `since_event_origin` | integer | — | Minimum ledger event offset (inclusive). Filter to decisions proposed at or after this offset. Use 0 or omit for all. |

---

### `analyze_failure_modes`

Failure-mode attribution: which conditions predict decisions that do not hold up? Joins outcome signals (superseded / stale-premises / contested) with context features (authorship shape, review depth, source, evidence/options richness) and computes AGGREGATE failure-rate patterns across each dimension. Reports effect sizes (failure-rate delta vs corpus baseline) and honest confidence flags based on sample size. Never returns per-person rankings — all findings are aggregate patterns. Use to answer: does agent-only authorship predict failure? Does peer review improve outcomes? Does thin context predict failure? Works on any deployment, no LLM.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `min_sample_size` | integer | — | Minimum group size required for a group to appear in top findings (default 3). Groups smaller than this are still included in breakdowns. |
| `since_event_origin` | integer | — | Minimum ledger event offset (inclusive). Filter to decisions proposed at or after this offset. Use 0 or omit for all. |

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
