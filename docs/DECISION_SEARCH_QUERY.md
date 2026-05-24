# Decision Search Query Slice

This slice adds a read-layer search API for decision exploration:
`search_decisions_fts(SearchDecisionRequest)` and the CLI command
`hivemind query search`. The legacy `hivemind query search_decisions` command
is retained as a compatibility alias over the same surface.

CLI filters use `--topic`, `--status`, `--actor-id`, `--source`, `--limit`,
`--since`, `--until`, and `--cursor`. The actor filter uses `--actor-id`
because `--actor` is already the global emitting actor flag.

## Request Semantics

- `query` is optional text. When present, every whitespace-delimited term must
  match somewhere in the decision row or its one-hop graph context.
- Matching uses a derived SQLite FTS5 index rebuilt from the replayed ledger
  graph. There is no fuzzy, semantic, or LLM ranking in this slice.
- `topic_keys`, `statuses`, `actor_ids`, and `sources` are filters. Filter
  dimensions are ANDed together. Multiple values inside `statuses`,
  `actor_ids`, and `sources` are ORed; multiple `topic_keys` must all be
  present on the decision.
- `since` and `until` are RFC3339 bounds over the decision proposal timestamp.
- `limit=0` means the default limit of 25. Larger limits are capped at 1000.
- `cursor` is an opaque string today, encoded as a stable numeric offset. The
  response returns `next_cursor` and sets `truncated=true` when more rows exist.

## Ranking And Ordering

Ordering is deterministic and starts with SQLite FTS5 `bm25` score:

1. FTS5 score over the per-decision search document.
2. Stable decision id tie-breaker.

Each result also carries a coarse rank basis:

1. Exact decision id or title match.
2. Decision title text match.
3. Decision rationale text match.
4. Graph-context match, including topics, status, actor ids, option ids or
   labels/descriptions when projected, evidence content, hypothesis statements,
   and supersession ids.

Each result includes matched fields, short snippets, and graph context ids so a
TUI can expand into `get_decision` or `get_decision_neighborhood`.

## Backend Strategy

The query reads the graph projection, not raw ledger JSON, then rebuilds a
derived `decision_search_fts` table in the SQLite ledger file. The table is
not source of truth: it is dropped and recreated from replayed graph state on
search, so deleting it or replaying the ledger reproduces the same results.

For Kuzu, the same graph-query surface is used to pull projected nodes and
edges before rebuilding the SQLite FTS index. This avoids response-shape
divergence while the project is still local-first. A later Kuzu or remote index
can replace the temporary SQLite index, but it must preserve the DTO and
ordering contract or version the response.

A future shared remote DB/service should expose this DTO as a service contract.
It can implement indexed text search internally, but it should return explicit
rank basis fields rather than hiding semantic scores behind unstable ordering.
