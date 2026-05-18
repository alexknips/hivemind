# Decision Search And Graph TUI Requirements

Status: investigation output for `hivemind-decision-search-graph-tui-requirements-dci1`.

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

## First Useful Slice

Build a read-only TUI after the query layer has real search support:

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

This slice is explicitly read-only. No proposing, accepting, rejecting, or
superseding decisions from the TUI until search/navigation proves useful.

## Search And Filters

Minimum search input:

- Text query over decision title and rationale.
- Topic filter, including multiple topic keys.
- Status filter: proposed, accepted, rejected, contested, superseded.
- Actor filter for proposed/accepted/rejected edges.
- Evidence, option, and hypothesis text matching once those properties are
  queryable.
- Date range once event timestamps are exposed in query DTOs.
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
- Query limit hit: show `truncated: true` and the next-page action.
- Missing graph projection or replay failure: show the query error and keep the
  user in the TUI shell.
- Refuted hypothesis: render as a warning on every decision that assumes it.
- Contested decision: render as a top-level status, not a badge hidden in detail.

## Stack Recommendation

Stay CLI/query-first until `search_decisions` and `get_decision_neighborhood`
exist. Then add a Rust TUI using `ratatui` plus `crossterm`; this keeps the
binary in the current Rust crate and avoids a second implementation language.
Add dependencies only with a TUI feature flag if build cost becomes noticeable.

Tests should cover query DTOs first, then pure view-model reducers for keyboard
state. Terminal snapshot tests are useful after the first layout stabilizes;
they should not block the query work.

## Query Gaps Before TUI

- `search_decisions(request)` with text query, filters, limit/cursor, snippets,
  matched fields, deterministic ordering, and `truncated`.
- `get_decision_neighborhood(id, depth, relation_filter?)` returning typed nodes
  and edges suitable for an ego graph.
- Richer `get_decision` detail: actor edges, option/evidence/hypothesis labels
  and content, timestamps, and event origins where available.
- Pagination on topic/status queries.
- DTO fields for stale assumptions and branched supersession chains.

## Out Of Scope

- Board/kanban columns as the primary interface.
- Whole-graph terminal canvas for large organizations.
- LLM summarization, semantic ranking, or deduplication inside the read/query
  layer.
- Write actions from the first TUI slice.
- Direct database access from the TUI; it should call the same CLI/query API or
  hosted service API as other clients.

## Follow-Up Beads

- `hivemind-decision-search-query-capability-4cyy`: implement decision search.
- `hivemind-kilj`: add a decision neighborhood query for ego-graph navigation.
- `hivemind-a2q5`: add a read-only `hivemind tui` prototype behind a feature
  flag after search and neighborhood queries exist.
- `hivemind-25h9`: add seed/golden fixtures for actor filters, evidence text
  matches, branchy supersession, empty pages, and truncated search results.
