---
title: MCP Tools
description: Reference for all 26 tools exposed by the HiveMind MCP server.
---

The HiveMind MCP server exposes 26 tools. Write tools append events to the
ledger and require an explicit `actor_id`. Read tools query the graph and never
write. Layer-3 tools add ranked summaries or compact views.

The Markdown decision-log export (`hivemind export --format markdown --out <dir>`) is
CLI-only and has no MCP tool: its output is a directory tree, not a single result an
MCP call can return.

See [MCP Setup](../../guides/mcp-setup/) to configure your client.

---

## Write tools

### `capture_decision`

Record a decision with rationale, topic keys, and at least one option. Defaults actor_id to agent:<tool>:<name> and writes source=agent. A `chosen_option_label` means the decision was already made: it self-accepts from `actor_id` by default, or from `decided_by` when the decider differs (e.g. a human decided, an agent is scribing it). Pass `still_proposed` to keep a genuine open recommendation at `proposed` instead.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `options` | object[] | ✓ |  |
| `rationale` | string | ✓ | Self-contained why, readable without the source conversation: at least 20 characters and 4 words, and not a bare reference into an external numbered list like "1a" or "2. a" — pair `quote` with `question` instead of embedding one. |
| `title` | string | ✓ | A name, not a summary: one sentence, at most 120 characters. Longer reasoning goes in `rationale`. |
| `topic_keys` | string[] | ✓ |  |
| `actor_id` | string | — | Optional capturing actor override. Defaults to `agent:<tool>:<name>`. |
| `chosen_option_label` | string | — | Label of the option that was accepted; must match one of `options[].label`. Setting this means the decision was already made — see `still_proposed` to keep it open instead. |
| `decided_by` | string | — | Actor who actually made the decision, when it differs from `actor_id` (the recording actor/scribe) — e.g. `human:alex@example.com` when an agent is writing down a decision a human made. Requires `chosen_option_label`. Mutually exclusive with `still_proposed`. |
| `evidence_ids` | string[] | — |  |
| `hypothesis_ids` | string[] | — |  |
| `project` | string | — | Registered project handle to file the decision under. An unknown handle is refused with the register command. Omit it and the decision is saved to the actor's personal project — the reply says so (`project_notice`). HiveMind checks the handle and never works out the project itself, so pass it whenever you know it; an HTTP-served MCP cannot see the caller's working directory. |
| `project_source` | string | — | How `project` was determined. Defaults to `stated`. Requires `project`. |
| `question` | string | — | The question `quote` answers, spelled out in the capturer's own words. Requires `quote`. |
| `quote` | string | — | Verbatim words of the decider, self-contained — not a bare reference like "1a" into an external numbered list. Requires `question`. A quote with no stated question is unreadable once the source conversation is gone. |
| `still_proposed` | boolean | — | Keep the decision at `proposed` even though `chosen_option_label` is set, for a genuine open recommendation awaiting someone else's decision. Defaults to false, which self-accepts (or accepts from `decided_by`) immediately after proposing. |

---

### `capture_evidence`

Record an evidence item that can be attached to decisions or hypotheses. Defaults actor_id to agent:<tool>:<name> and writes source=agent.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `content` | string | ✓ |  |
| `actor_id` | string | — | Optional capturing actor override. Defaults to `agent:<tool>:<name>`. |

---

### `capture_hypothesis`

Record a hypothesis. Defaults actor_id to agent:<tool>:<name> and writes source=agent.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `statement` | string | ✓ |  |
| `actor_id` | string | — | Optional capturing actor override. Defaults to `agent:<tool>:<name>`. |

---

### `disagree_decision`

Record an actor disagreement with a decision and return the resulting derived status. Resolves by decision_id or a free-text description — exactly one is required. An ambiguous description returns a successful result shaped `{outcome: "ambiguous", candidates: [...]}`, not an error, and no event is appended; re-call with decision_id from that list. A description matching nothing is also a successful result, shaped `{outcome: "not_found"}`, and no event is appended. Wraps `hivemind disagree`.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `reason` | string | ✓ |  |
| `actor_id` | string | — | Disagreeing actor. Defaults to `agent:<tool>:<name>` when omitted. |
| `decision_id` | string | — | The decision to disagree with. Provide this or `description`, not both. |
| `description` | string | — | Free-text description to resolve to a decision when the id is not known. |
| `topic` | string | — | Narrows description resolution to decisions carrying this topic key. |

---

### `supersede_decision`

Propose a replacement decision and mark it as superseding an old decision. Wraps `hivemind supersede`. Resolves the old decision by `old_decision_id` or a free-text `description` (+ optional `topic`) — exactly one of `old_decision_id`/`description` is required, the same fluent resolution `get_decision_neighborhood` uses. An ambiguous description returns a successful result shaped `{outcome: "ambiguous", candidates: [...]}`, not an error, with no write; re-call with `old_decision_id` from that list. A description matching nothing is also a successful result, shaped `{outcome: "not_found"}`, also with no write.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `rationale` | string | ✓ | Self-contained why, readable without the source conversation: at least 20 characters and 4 words, and not a bare reference into an external numbered list like "1a" or "2. a". |
| `title` | string | ✓ | A name, not a summary: one sentence, at most 120 characters. Longer reasoning goes in `rationale`. |
| `actor_id` | string | — | Superseding actor. Defaults to `agent:<tool>:<name>` when omitted. |
| `chosen_option_label` | string | — |  |
| `description` | string | — | Free-text match for the decision to supersede. Required when `old_decision_id` is omitted. |
| `evidence_ids` | string[] | — |  |
| `hypothesis_ids` | string[] | — |  |
| `old_decision_id` | string | — |  |
| `options` | any[] | — |  |
| `project` | string | — | Registered project handle to file the superseding decision under. An unknown handle is refused with the register command. Omit it and the new decision inherits the old decision's project. HiveMind never works out the project itself; an HTTP-served MCP cannot see the caller's working directory. |
| `project_source` | string | — | How `project` was determined. Defaults to `stated`. Requires `project`. |
| `topic` | string | — | Optional topic_key filter narrowing the `description` match. |
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

### `get_situational_decisions`

"What should I know before I touch this?" — decisions bearing on the given situation, no id or hand-typed question needed. Matches by exact topic_keys membership and by term overlap against evidence content (deterministic, no LLM); each result names which matched and includes held_up/reasons from get_decision_outcome verbatim. Topic matches are exact; evidence matches are a fuzzy heuristic over free text, not a structural path reference — treat matched_via[].kind accordingly.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `paths` | string[] | ✓ | Touched files/dirs and/or a branch name. All entries are tokenized the same way (split on path separators, lowercased, stopwords/extensions dropped). |
| `cursor` | string | — |  |
| `limit` | integer | — |  |
| `since_offset` | integer | — | Annotate results with whether they changed since this ledger offset (exclusive). |
| `since_timestamp` | string | — | Annotate results with whether they changed since this RFC3339 timestamp. |

---

### `get_supersession_chain`

Return the linear supersession chain a decision sits in, oldest first. Equivalent to `hivemind query chain`. Resolves by decision_id or a free-text description — exactly one is required. An ambiguous description returns a successful result shaped `{outcome: "ambiguous", candidates: [...]}`, not an error; re-call with decision_id from that list. A description matching nothing is also a successful result, shaped `{outcome: "not_found"}` (there is no #N/--pick over MCP to retry against, so there is nothing further to disambiguate).

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | — | The decision to inspect. Provide this or `description`, not both. |
| `description` | string | — | Free-text description to resolve to a decision when the id is not known. |
| `topic` | string | — | Narrows description resolution to decisions carrying this topic key. |

---

### `get_decision_neighborhood`

"Why does this decision look the way it does?" — the one-hop graph neighborhood around a decision: proposing/accepting/rejecting actors, options, the chosen option, evidence, premised hypotheses (with their supporting/refuting evidence one hop further), and supersession links in both directions. Equivalent to `hivemind query why`. Resolves by decision_id or a free-text description — exactly one is required. An ambiguous description returns a successful result shaped `{outcome: "ambiguous", candidates: [...]}`, not an error; re-call with decision_id from that list. A description matching nothing is also a successful result, shaped `{outcome: "not_found"}` (there is no #N/--pick over MCP to retry against, so there is nothing further to disambiguate).

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | — | The decision to inspect. Provide this or `description`, not both. |
| `description` | string | — | Free-text description to resolve to a decision when the id is not known. |
| `topic` | string | — | Narrows description resolution to decisions carrying this topic key. |

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

Layer-3 compact view of a decision subgraph. Applies signal/noise semantics: terminal decision is fully preserved; superseded predecessors, unchosen options, and resolved blockers are elided and counted. Contested decisions are never compacted. Resolves by decision_id or a free-text description — exactly one is required. An ambiguous description returns a successful result shaped `{outcome: "ambiguous", candidates: [...]}`, not an error; re-call with decision_id from that list. A description matching nothing is also a successful result, shaped `{outcome: "not_found"}`. Returns data: null when a directly-supplied decision_id itself does not exist.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | — | The decision to compact. Provide this or `description`, not both. If mid-chain, the terminal (newest) decision in the supersession chain is used as the focal node. |
| `description` | string | — | Free-text description to resolve to a decision when the id is not known. |
| `topic` | string | — | Narrows description resolution to decisions carrying this topic key. |

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

"Did this decision hold up?" — leads with the decision, rationale, chosen and rejected options, and who decided it, then whether it still holds: superseded (and how fast), stale premises (premised on a refuted hypothesis), contested (unresolved disagreement), or thin structure (no options/evidence), each with its contributing reasons attached. No LLM involved; derived purely from graph edges. Equivalent to `hivemind query verify`. Resolves by decision_id or a free-text description — exactly one is required. An ambiguous description returns a successful result shaped `{outcome: "ambiguous", candidates: [...]}`, not an error; re-call with decision_id from that list. A description matching nothing is also a successful result, shaped `{outcome: "not_found"}` (there is no #N/--pick over MCP to retry against, so there is nothing further to disambiguate).

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `decision_id` | string | — | The decision to evaluate. Provide this or `description`, not both. |
| `description` | string | — | Free-text description to resolve to a decision when the id is not known. |
| `topic` | string | — | Narrows description resolution to decisions carrying this topic key. |

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

Derive the context record for a single decision: the conditions under which it was made. Returns five feature groups — authorship shape (human-authored / agent-proposed+human-accepted / agent-only / unknown) with proposer_id (who recorded it) and accepted_by (who actually decided — may differ from proposer_id, or be empty when unreviewed), source system and model/session reference, review depth (unreviewed / self_accepted / peer_reviewed / disputed), evidence and hypothesis counts, and context richness proxies (options count, rationale character count). No LLM involved; derived purely from graph edges. Pair with get_decision_outcome for causal attribution. Returns null when the decision_id is not found.

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

### `scan_misfiled_decisions`

Flag decisions carrying a caller-named "foreign" topic key — a decision tagged with another ledger's name most likely belongs there instead (hivemind-zdsh.14). Deterministic exact-match only, no LLM, no inference beyond topic-key membership: HiveMind does not yet know which project a ledger belongs to (hivemind-s15q A1/A3), so the caller supplies the foreign keys. Read-only report — never moves a decision (that needs hivemind-s15q C1, not built yet).

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `foreign_topic_keys` | string[] | ✓ | Topic keys that indicate a decision belongs to a different ledger (e.g. another rig's name). |
| `cursor` | string | — | Pagination cursor from a previous response's `next_cursor` field. |
| `limit` | integer | — | Maximum results to return (1–1000, default 25). |

---

### `analyze_failure_modes`

Failure-mode attribution: which conditions predict decisions that do not hold up? Joins outcome signals (superseded / stale-premises / contested) with context features (authorship shape, review depth, source, evidence/options richness) and computes AGGREGATE failure-rate patterns across each dimension. Reports effect sizes (failure-rate delta vs corpus baseline) and honest confidence flags based on sample size. Never returns per-person rankings — all findings are aggregate patterns. Use to answer: does agent-only authorship predict failure? Does peer review improve outcomes? Does thin context predict failure? Works on any deployment, no LLM.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `min_sample_size` | integer | — | Minimum group size required for a group to appear in top findings (default 3). Groups smaller than this are still included in breakdowns. |
| `since_event_origin` | integer | — | Minimum ledger event offset (inclusive). Filter to decisions proposed at or after this offset. Use 0 or omit for all. |

---

### `classify_queue_list`

List pending (unclassified) ingest batches with rendered turn text, plus today's classification budget. HTTP-transport only (hivemind-zdsh.18) — pass `session_id` to scope to one ingest session for the session-grouped classification cadence: one model call per session end, not one per batch.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `limit` | integer | — | Maximum pending batches to return (default 20). |
| `session_id` | string | — | Only list batches from this ingest session. |

---

### `classify_queue_submit`

Submit captures for one or more pending ingest batches as a single classification event, moving every listed batch id from pending to classified together. HTTP-transport only (hivemind-zdsh.18). Pass more than one `batch_ids` entry to cover a whole session's batches in one model call, per the session-grouped cadence. Refuses with an error once the daily classification cap is hit — batches stay pending for the next day rather than being dropped.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `batch_ids` | string[] | ✓ | Batch id(s) from `classify_queue_list`, all from the same session when more than one. |
| `captures` | object[] | ✓ | CaptureItem objects, same shape as `capture_decision`/`capture_evidence` produce. |
| `model` | string | — | Classifier identifier recorded on the event. Defaults to `agent:worker-a` (subscription-seat classification). |

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
