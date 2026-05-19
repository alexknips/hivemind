# Decision Search Query Slice

This slice adds a read-layer search API for decision exploration:
`search_decisions(SearchDecisionRequest)` and the CLI command
`hivemind query search_decisions`.

CLI filters use `--topic`, `--status`, `--actor-id`, `--source`, `--limit`,
and `--cursor`. The actor filter uses `--actor-id` because `--actor` is already
the global emitting actor flag.

## Request Semantics

- `query` is optional text. When present, every whitespace-delimited term must
  match somewhere in the decision row or its one-hop graph context.
- Matching is case-insensitive substring matching. There is no fuzzy,
  semantic, or LLM ranking in this slice.
- `topic_keys`, `statuses`, `actor_ids`, and `sources` are filters. Filter
  dimensions are ANDed together. Multiple values inside `statuses`,
  `actor_ids`, and `sources` are ORed; multiple `topic_keys` must all be
  present on the decision.
- `limit=0` means the default limit of 25. Larger limits are capped at 1000.
- `cursor` is an opaque string today, encoded as a stable numeric offset. The
  response returns `next_cursor` and sets `truncated=true` when more rows exist.

## Ranking And Ordering

Ordering is deterministic:

1. Exact decision id or title match.
2. Decision title text match.
3. Decision rationale text match.
4. Graph-context match, including topics, status, actor ids, option ids or
   labels/descriptions when projected, evidence content, hypothesis statements,
   and supersession ids.
5. Stable decision id tie-breaker.

Each result includes matched fields, short snippets, and graph context ids so a
TUI can expand into `get_decision` or `get_decision_neighborhood`.

## Backend Strategy

The query reads the graph projection, not raw ledger JSON. For the in-memory
backend, it builds a small read-model snapshot from projected nodes and edges,
then ranks in Rust. This keeps local CLI/TUI behavior deterministic and avoids
backend-specific text search semantics.

For Kuzu, the same graph-query surface is used to pull projected nodes and
edges before applying the same Rust ranking logic. This avoids divergence
between memory and Kuzu while the project is still local-first. A later Kuzu
index can replace the scan when result volume justifies it, but it must preserve
the DTO and ordering contract or version the response.

A future shared remote DB/service should expose this DTO as a service contract.
It can implement indexed text search internally, but it should return explicit
rank basis fields rather than hiding semantic scores behind unstable ordering.
