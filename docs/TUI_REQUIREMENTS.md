# Decision Search And Graph TUI Requirements

This document captures stable product requirements for searching and inspecting
HiveMind decisions in a terminal UI.

## Context

HiveMind needs a small terminal UI for finding decisions and inspecting why
they exist. The current CLI exposes three deterministic read operations:
`get_decision`, `get_relevant_decisions(topic, status?)`, and
`get_supersession_chain`, plus a DOT dump of the projected graph. The seed data
covers supersession, contested decisions, evidence, hypotheses, and a refuted
hypothesis affecting dependent decisions.

`beads_viewer` is useful as an interaction reference for keyboard-first list
browsing, split list/detail layouts, fuzzy search, help overlays, graph export,
and focused graph neighborhoods. Its board view is not a good primary model for
HiveMind because decisions do not flow through work columns. HiveMind's primary
shape is a provenance graph: decisions, actors, options, evidence, hypotheses,
and supersession edges.

## Local Workflows

The search/TUI surface must support two local-first workflows:

1. Explore decisions captured from agents, with actor/source provenance and fast
   local graph navigation.
2. Explore decisions imported from external textual documents, including the
   recurring question: "what decisions were added to the graph since last week?"

This is not a board, kanban, task tracker, or project-management UI. Any view
that looks like columns of work states is out of scope unless a concrete
decision-provenance workflow later proves it necessary. The replacement model is
search results plus an ego-centric provenance graph.

## Explorer Shape

The TUI is read-only and centered on search plus provenance navigation:

1. Start on a search screen with a result list and compact filter bar.
2. Open a selected decision into a detail pane showing title, status, actor
   edges, rationale, topic keys, chosen option, other options, evidence,
   hypotheses with status, and supersession summary.
3. Show a graph-context pane centered on the selected decision: incoming and
   outgoing one-hop edges grouped by relation type.
4. Let users jump from any connected node to its detail, then go back through a
   breadcrumb/path stack.
5. Offer DOT export handoff for full graph rendering instead of trying to draw
   the whole organization graph in the terminal.

The document-import workflow uses the same explorer shape:

1. Add source/document filters for imported textual decisions.
2. Add a temporal decision-diff entry point for "decisions added since <time>",
   using the relative-date and added-node semantics in
   `docs/TEXT_IMPORT_AND_DIFF_SEMANTICS.md`.
3. Let each diff result open the same decision detail and graph-context panes,
   with provenance back to the imported document source.
4. Distinguish newly added decision nodes from status changes or new evidence on
   an existing decision.

## Search And Filters

Minimum search input:

- Text query over decision title and rationale.
- Topic filter, including multiple topic keys.
- Status filter: proposed, accepted, rejected, contested, superseded.
- Actor filter for proposed/accepted/rejected edges.
- Source filter, at least `source=agent` for agent-capture workflows and
  `source=document` for document-import workflows.
- Evidence, option, and hypothesis text matching once those properties are
  queryable.
- Date range once event timestamps are exposed in query DTOs.
- Added-since filter for temporal decision diff workflows.
- Limit and cursor/pagination; never silently truncate.

Minimum result row:

- Decision id, title, status, topic keys, short rationale snippet, matched field,
  and stale/refuted-assumption indicator when present.
- Deterministic sort for slice 1: exact id/title matches first, then title,
  rationale, graph-neighbor matches, and stable id order as final tiebreaker.
- Ranking or semantic search is later layer-3 work unless the score basis is
  explicit and returned with the result.

## Graph Navigation

Terminal graph UI should be ego-centric, not canvas-style:

- Center node: selected decision.
- Incoming: newer decisions that `SUPERSEDES` this decision.
- Outgoing: superseded decisions, options, chosen option, evidence, hypotheses,
  and actor edges.
- Related evidence-to-hypothesis edges should appear when a displayed hypothesis
  is supported or refuted by visible evidence.
- Edge labels must be short and explicit: `PROPOSED_BY`, `ACCEPTED_BY`,
  `REJECTED_BY`, `SUPERSEDES`, `BASED_ON`, `HAS_OPTION`, `CHOSE`, `ASSUMES`,
  `SUPPORTS`, `REFUTES`.
- Dense graphs collapse by relation group with counts and an expand action.
- Cycles, branched supersession chains, and missing nodes are visible error
  states, not hidden.

Preferred shape: result list on the left, detail pane on the right, and a
bottom or alternate graph-context pane. On narrow terminals, switch to one pane
at a time: results -> detail -> graph context.

## Keyboard Model

- `j`/`k` or arrows: move selection.
- `/`: focus search input.
- `tab`: rotate results/detail/graph focus.
- `enter`: open selected decision or connected node.
- `b`: back in breadcrumb stack.
- `[`/`]`: previous/next supersession decision.
- `o`: cycle status filter.
- `t`: edit topic filter.
- `a`: edit actor filter.
- `e`, `h`, `p`: focus evidence, hypotheses, or provenance/actor edges.
- `x`: export current focused neighborhood as DOT.
- `?`: context help.
- `esc`: leave input/close overlay.
- `q`: quit.

## Empty And Error States

- Empty ledger: show commands to seed or emit the first decision.
- No search results: keep filters visible and show which constraints removed all
  results.
- No added decisions in diff view: show the resolved time window and make clear
  that the graph changed zero decision nodes in that interval.
- Query limit hit: show `truncated: true` and the next-page action.
- Missing graph projection or replay failure: show the query error and keep the
  user in the TUI shell.
- Refuted hypothesis: render as a warning on every decision that assumes it.
- Contested decision: render as a top-level status, not a badge hidden in detail.

## Stack Recommendation

The TUI should call explicit query APIs such as `search_decisions` and
`get_decision_neighborhood`; it should not reach around the query layer into
storage internals. A Rust TUI using `ratatui` plus `crossterm` keeps the binary
in the current Rust crate and avoids a second implementation language.
Add dependencies only with a TUI feature flag if build cost becomes noticeable.

Tests should cover query DTOs first, then pure view-model reducers for keyboard
state. Terminal snapshot tests are useful after the first layout stabilizes;
they should not block the query work.

## Query Capabilities

- `search_decisions(request)` with text query, filters, limit/cursor, snippets,
  matched fields, deterministic ordering, and `truncated`.
- `get_decision_neighborhood(id, depth, relation_filter?)` returning typed nodes
  and edges suitable for an ego graph.
- `get_decisions_added_since(since, until?, filters?)` or equivalent temporal
  diff query for the local document-import workflow. Detailed import,
  provenance, idempotency, and "since last week" semantics are defined in
  `docs/TEXT_IMPORT_AND_DIFF_SEMANTICS.md`.
- Richer `get_decision` detail: actor edges, option/evidence/hypothesis labels
  and content, timestamps, and event origins where available.
- Pagination on topic/status queries.
- DTO fields for stale assumptions, branched supersession chains, source
  provenance, and imported document references where applicable.

The slice-1 `search_decisions` DTO, ranking contract, pagination behavior, and
backend tradeoffs are specified in `docs/DECISION_SEARCH_QUERY.md`.

## Out Of Scope

- Board/kanban columns as the primary interface, including status columns that
  make decisions look like tickets.
- Whole-graph terminal canvas for large organizations.
- LLM summarization, semantic ranking, or deduplication inside the read/query
  layer.
- Write actions from the first TUI slice.
- Hosted/SaaS ingestion or collaboration UX for these local workflows.
- Direct database access from the TUI; it should call the same CLI/query API or
  hosted service API as other clients.
