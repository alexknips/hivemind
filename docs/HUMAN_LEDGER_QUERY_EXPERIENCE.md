# Human Ledger Query Experience

HiveMind should make decision history easy for humans to read without turning a
history interface into a decision-making actor. The first product surface should
be a read-only explorer UI over deterministic query APIs. A read-only history
agent is useful later, but only as a citation-constrained presentation layer over
the same query model.

## Recommendation

Build both surfaces over one layer-2 query/read model, but prioritize the
read-only explorer UI first.

The UI should come first because it makes provenance, disagreement, staleness,
limits, and source events visible by default. A chat agent is convenient for
plain-language questions, but it creates a higher risk that users ask it to
recommend, choose, unblock, or resolve. That agent should wait until search,
timeline, neighborhood, citation, and export DTOs are explicit enough that the
agent can only summarize bounded query results.

The recommended sequence is:

1. Extend deterministic query APIs for search, timeline, changed-since,
   neighborhood, blockers, and read-only export.
2. Ship a read-only TUI or web explorer using those APIs.
3. Add a history agent later as a thin layer-3 summarizer with read-only
   credentials, a query allowlist, required citations, and explicit refusal
   rules for decision-making requests.

## Human Workflows

Humans need to answer these questions quickly:

- What decisions and decision-related events happened recently?
- What changed since a date, release, import run, or ledger offset?
- Why was a decision made, and what options were considered?
- Who proposed, accepted, rejected, superseded, or contested it?
- What evidence, hypotheses, and refuted assumptions does it depend on?
- What decisions, humans, or agents are blocked by stale or contested context?
- What read-only summary can be shared back to a team without losing citations?

These are ledger-history workflows, not project-management workflows. HiveMind
should surface blockers and stale assumptions when they are represented as
decision graph state, but it should not become a task board.

## Comparison Matrix

| Surface | Strengths | Risks | Best first use |
| --- | --- | --- | --- |
| CLI/query output | Fast to implement, scriptable, testable, exact JSON for agents and CI. | Hard for non-developers to browse; provenance is easy to miss in dense JSON. | Developer validation and the stable contract behind every other surface. |
| Read-only UI/TUI explorer | Provenance, status, staleness, truncation, and graph context can be visible at the same time. Easier for humans to browse and compare. | Needs enough query APIs to avoid direct storage access or UI-only inference. | First non-developer surface for search, timeline, detail, and graph navigation. |
| Read-only history agent | Natural questions such as "what changed since last week?" and "why did we choose this?" | Users may try to delegate decisions; summaries can hide uncertainty if citations are weak. | Later presentation layer for bounded, cited history answers. |
| Hybrid UI plus cited agent summaries | Combines browsable evidence with concise generated summaries; users can inspect citations immediately. | Most complex; requires strict separation between deterministic reads and layer-3 summarization. | After the UI and citation/export DTOs are stable. |

## Read Model Requirements

Both the UI and the future history agent need the same deterministic query
capabilities:

- `search_decisions(request)`: text query, topic/status/actor/source filters,
  snippets, matched fields, stable ordering, limit, cursor, and `truncated`.
- `get_decision(id)`: title, status, rationale, topic keys, actor edges,
  options, chosen option, evidence, hypotheses, supersession summary, timestamps,
  source, source refs, and event origins.
- `get_decision_neighborhood(id, depth, relation_filter?)`: typed nodes and
  typed edges for one-hop graph navigation, including actor, option, evidence,
  hypothesis, supersession, `SUPPORTS`, and `REFUTES` edges.
- `get_decisions_changed_since(since, until?, filters?)`: deterministic diff by
  event offset and/or timestamp that classifies new decisions, status changes,
  new evidence, refuted assumptions, and supersessions.
- `get_recent_activity(limit, cursor, filters?)`: bounded timeline rows for
  recent decision events with actor, event origin, source, and affected node.
- `get_blocked_or_stale_decisions(filters?)`: decisions with refuted
  assumptions, superseded dependencies, contested status, or concurrent
  supersession ambiguity.
- `export_read_only_summary(request)`: JSON or Markdown export with query
  parameters, ledger range, generated time, result count, `truncated`,
  continuation cursor, and citation map.

No query may call an LLM, perform semantic ranking, collapse disagreement, or
hide stale/refuted context. Ranking beyond explicit deterministic ordering
belongs in layer 3 and must return its basis.

## UI Shape

The explorer should be read-only and centered on scanning:

- Recent activity timeline with filters for actor, topic, source, status, and
  changed-since.
- Search results with matched field, rationale snippet, status, topic keys, and
  stale/refuted-assumption indicator.
- Decision detail view with rationale, options, chosen option, evidence,
  hypotheses, actor actions, event origins, source refs, and timestamps.
- One-hop graph context grouped by relation type, with counts and expansion for
  dense neighborhoods.
- Changed-since view that distinguishes newly added decision nodes from updates
  to existing decisions.
- Export/share action that produces a read-only cited summary, not a new
  decision event.

The UI should call the same public query APIs as CLI, MCP, or service clients.
It should not read SQLite, Kuzu, Postgres, or projection tables directly.

## History Agent Guardrails

A future history agent may summarize, list, cite, and reformat ledger history.
It must not make or recommend decisions.

Allowed:

- Search, timeline, changed-since, neighborhood, blocker/staleness, and export
  queries from a read-only allowlist.
- Summaries that cite event origins, decision ids, actors, timestamps, and source
  refs.
- Statements of uncertainty when a query is truncated, empty, stale, contested,
  or missing expected provenance.

Forbidden:

- Any write command, state mutation, or hidden tool that can propose, accept,
  reject, supersede, contest, resolve, or compact decisions.
- Recommendations such as "choose option A", "approve this", or "this blocker
  is resolved" unless those are direct quotes from existing decisions and cited
  as history.
- Silent synthesis across disagreement. Contested decisions remain contested in
  the answer.
- Silent truncation. The agent must report `truncated`, continuation cursors,
  and unresolved query limits.
- Invented confidence. If the agent ranks or scores in layer 3, the score basis
  must be cited and separable from the deterministic query result.

When asked to decide, the agent should answer with history only: "I cannot make
or recommend a decision. Here are the prior decisions, blockers, evidence, and
actors relevant to that question."

## First Slice

The smallest useful non-developer slice is a read-only explorer over existing
and near-term query APIs:

1. Search decisions by text, topic, status, actor, and source.
2. Open a decision detail view with rationale, options, evidence, hypotheses,
   actor provenance, timestamps, and event origins.
3. Show one-hop graph context for the selected decision.
4. Show recent activity and changed-since results with bounded pagination.
5. Export a cited read-only summary with query parameters and truncation state.

This slice helps teams unblock work because a human can inspect what changed,
why it changed, who was involved, and which decisions are stale or contested.
Agents can use the same exports as cited context without receiving decision
authority.

## Timeline, Changed-Since, And Export DTOs

The first layer-2 history DTOs are deterministic ledger reads. They do not call
LLMs, mutate state, or rank beyond explicit ledger ordering.

`get_recent_activity(request)` returns newest-first timeline rows:

- `filters`: applied `actor_ids`, `sources`, `source_refs`, `topic_keys`, and
  decision `statuses`.
- `limit`, `cursor`, `next_cursor`, and `total_matches`: bounded offset
  pagination over the filtered timeline.
- `ledger_range`: `from_offset_exclusive` and `to_offset_inclusive` for the
  scanned ledger range.
- `items`: event rows with `event_origin`, `event_uuid`, event `type`,
  `change_kind`, actor, source/source ref, timestamp, affected decision ids,
  affected graph nodes, and `citation_id`.

`get_decisions_changed_since(request)` returns oldest-first changes in a
resolved ledger window:

- `resolved_since` and `resolved_until`: concrete offsets, plus caller-supplied
  timestamps when timestamp bounds were used.
- `boundary_event_offsets`: timestamp-to-offset resolutions so the same query
  can be replayed exactly.
- `filters`, `limit`, `cursor`, `next_cursor`, `total_matches`, and
  `ledger_range`.
- `items`: event rows classified as `new_decision`, `status_change`,
  `new_evidence`, `refuted_assumption`, `supersession`, or `context_change`,
  with the same citation and affected-node fields as timeline rows.

Offset bounds are canonical. A `since_offset` is exclusive; `until_offset` is
inclusive. Timestamp bounds resolve to the greatest event offset at or before
the timestamp, then the query uses those offsets.

`export_read_only_summary(request)` wraps either query for sharing:

- `query`, `format`, `query_params`, `ledger_range`, and `generated_at`.
- `result_count`, `truncated`, and `continuation_cursor`.
- `citation_map`: maps `event:<offset>` citation ids to event id, UUID, type,
  actor, source/source ref, and timestamp.
- `json` for JSON exports or `markdown` for deterministic Markdown exports.

The Markdown export is a mechanical rendering of bounded query rows and
citations. It is not a narrative summary and must not hide truncation.

## Related Implementation Beads

The UI path is represented by `hivemind-a2q5` after query support exists.
Search support is represented by
`hivemind-decision-search-query-capability-4cyy`. Ego-graph navigation is
represented by `hivemind-kilj`.

Two follow-up beads cover gaps this document makes explicit:

- `hivemind-timeline-export-query-capability-qib4w`: timeline,
  changed-since, blocker/staleness, and cited export DTOs.
- `hivemind-history-agent-guardrail-contract-sw9dy`: read-only history-agent
  allowlist, refusals, citation requirements, and test prompts.
