# HiveMind Search Design

HiveMind search is a layer-2 read surface for finding decisions and their
explicit graph context. It is not a semantic answer engine, a recommendation
system, or a write-path deduplication tool. A good search result helps a human
or agent recover what was decided, why, by whom, what it depends on, and whether
the result is contested, superseded, or stale.

This document defines the storage-agnostic search contract. SQLite FTS5, Kuzu,
Postgres, or a future service may implement the contract differently, but CLI,
MCP, and internal callers should see the same request and response semantics.

## What Good Search Means

Search is successful when the caller can find a decision without already
knowing its id, then decide whether to inspect, cite, contest, or supersede it.
The primary result type is always a `Decision`. Evidence, hypotheses, options,
actors, and supersession edges are searchable context around decisions, not
standalone search products.

Good search must:

- Find decisions by remembered fragments from title, rationale, option text,
  evidence text, hypothesis statements, topic keys, actors, sources, and related
  decision ids.
- Filter by explicit graph state: status, actor, topic, source, time window,
  evidence, hypothesis, supersession, and stale or blocker context.
- Preserve disagreement. `contested` is a first-class status filter and result
  status, never a hidden annotation.
- Surface staleness. A decision that assumes a refuted hypothesis, or depends
  on superseded context, must carry that state in search results by default.
- Return bounded pages. Every limit hit returns `truncated: true` and an opaque
  continuation cursor.
- Report provenance. Result rows expose event origins or citation ids for the
  decision and the matched graph context, or provide stable ids that let callers
  fetch that provenance with `get_decision` or `get_decision_neighborhood`.
- Stay deterministic at a given ledger state. No query calls an LLM, performs
  semantic ranking, silently deduplicates decisions, or invents confidence.

Search must not become a general knowledge graph query surface. It can search
the graph only insofar as graph nodes explain decisions.

## Query Patterns

These are the real user questions the surface must cover.

| User question | Deterministic search shape | Notes |
| --- | --- | --- |
| Find the decision about authentication from last Tuesday. | `q=authentication`, timestamp or offset window, optional topic filter. | Relative dates are resolved by the caller or CLI into concrete bounds before the query executes. |
| What decisions did agent X make this quarter? | `actor_ids=[X]`, timestamp or offset window. | Actor filters match explicit proposal, acceptance, rejection, and supersession edges as the graph supports them. |
| What did Claude decide that contradicted what Codex decided? | `status=contested`, `actor_ids=[Claude, Codex]`, `actor_match=all`, optional topic or time filters. | Layer 2 only finds explicit disagreement. Inferring textual contradiction is layer 3 and must cite the layer-2 rows it used. |
| Find decisions on topic X that are contested. | `topic_keys=[X]`, `statuses=[contested]`. | `contested` is a top-level status. |
| Find decisions that depend on hypothesis Y. | `premised_on_hypothesis_ids=[Y]`. | Refuted hypothesis status is returned with the result. |
| Find decisions where the rationale mentions a fragment. | `query=<fragment>`, `field_scopes=[decision.rationale]`. | Field scoping is deterministic string matching, not semantic intent. |
| Find decisions supported by evidence containing a fragment. | `query=<fragment>`, `field_scopes=[evidence.content]`. | Results are decisions that reference matching evidence. |
| Find decisions superseded by decision D. | `superseded_by_decision_ids=[D]`. | Concurrent supersessions are returned as multiple explicit edges. |
| Find decisions stale because an assumption was refuted. | `staleness=stale`, optional `stale_reason=refuted_assumption`. | The reason is explicit graph state, not a model judgment. |

## Operation Name

The storage-agnostic operation is `search_decisions`.

The current CLI spelling is:

```bash
hivemind query search_decisions [flags]
```

The preferred human-facing alias is:

```bash
hivemind query search [flags]
```

Both spellings should call the same internal operation and return the same JSON.
Keeping `search_decisions` preserves compatibility with existing scripts while
`search` gives humans the shorter command the product wants.

## CLI Surface

The CLI is a thin wrapper around `search_decisions(SearchDecisionRequest)`.
Flags normalize into the same request DTO used by MCP and library callers.

```bash
hivemind query search \
  --q <text> \
  --topic <topic-key>[,<topic-key>...] \
  --topic-match all|any \
  --status proposed|accepted|rejected|contested|superseded[,..] \
  --actor-id <actor-id>[,<actor-id>...] \
  --actor-match any|all \
  --source cli|agent|human|slack|document|api[,..] \
  --source-ref <source-ref>[,<source-ref>...] \
  --field all|decision.title|decision.rationale|decision.topic|decision.status|actor.id|actor.source_ref|option.label|option.description|evidence.content|hypothesis.statement|supersession.id[,..] \
  --premised-on-hypothesis-id <hypothesis-id>[,<hypothesis-id>...] \
  --based-on-evidence-id <evidence-id>[,<evidence-id>...] \
  --supersedes-decision-id <decision-id>[,<decision-id>...] \
  --superseded-by-decision-id <decision-id>[,<decision-id>...] \
  --staleness any|fresh|stale \
  --stale-reason refuted_assumption|superseded_dependency|concurrent_supersession[,..] \
  --since-offset <event-offset> \
  --until-offset <event-offset> \
  --since <rfc3339-timestamp> \
  --until <rfc3339-timestamp> \
  --order match|newest|oldest|decision_id \
  --limit <n> \
  --cursor <opaque-cursor>
```

Minimum current flags are `--q`, `--topic`, `--status`, `--actor-id`,
`--source`, `--limit`, and `--cursor`; they already cover the initial stable
search contract. The remaining flags are part of the storage-agnostic contract
and should be added without changing the operation name or result envelope.

CLI examples:

```bash
hivemind query search --q authentication --since 2026-05-19T00:00:00Z --until 2026-05-20T00:00:00Z
hivemind query search --actor-id agent:claude:s1 --since 2026-04-01T00:00:00Z --until 2026-07-01T00:00:00Z
hivemind query search --status contested --actor-id agent:claude:s1,agent:codex:s2 --actor-match all
hivemind query search --premised-on-hypothesis-id hypothesis-123
hivemind query search --q "cache eviction" --field decision.rationale
```

## Request Semantics

`SearchDecisionRequest` has these storage-independent fields:

```rust
pub struct SearchDecisionRequest {
    pub query: Option<String>,
    pub filters: SearchDecisionFilters,
    pub order: SearchDecisionOrder,
    pub limit: usize,
    pub cursor: Option<String>,
}

pub struct SearchDecisionFilters {
    pub topic_keys: Vec<String>,
    pub topic_match: MatchMode,
    pub statuses: Vec<DecisionStatus>,
    pub actor_ids: Vec<String>,
    pub actor_match: MatchMode,
    pub sources: Vec<String>,
    pub source_refs: Vec<String>,
    pub field_scopes: Vec<SearchFieldScope>,
    pub premised_on_hypothesis_ids: Vec<String>,
    pub based_on_evidence_ids: Vec<String>,
    pub supersedes_decision_ids: Vec<String>,
    pub superseded_by_decision_ids: Vec<String>,
    pub staleness: StalenessFilter,
    pub stale_reasons: Vec<StaleReason>,
    pub since_offset: Option<u64>,
    pub until_offset: Option<u64>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

pub enum MatchMode {
    Any,
    All,
}

pub enum SearchDecisionOrder {
    Match,
    Newest,
    Oldest,
    DecisionId,
}
```

Normalization rules:

- Empty strings normalize to absent values.
- Text matching is case-insensitive and term-based. Every query term must match
  at least one selected field unless a future query-language mode explicitly
  says otherwise.
- Multiple filter dimensions are ANDed together.
- Multiple values inside one filter dimension use that dimension's match mode.
  Status, source, source ref, stale reason, evidence id, hypothesis id, and
  supersession id filters default to `Any`. Topic defaults to `All`. Actor
  defaults to `Any`.
- Time filters are resolved against ledger events, not wall-clock query time.
  Offset bounds are canonical. Timestamp bounds are resolved to concrete event
  offset bounds and the resolved bounds are returned.
- `limit=0` means default limit. Implementations may cap large limits, but must
  return the effective limit in the response.
- `cursor` is opaque. Clients must not parse it or assume it is an offset.

## Result Envelope

Search returns the standard query response envelope:

```rust
pub struct QueryResponse<T> {
    pub result_count: usize,
    pub truncated: bool,
    pub latency_ms: u128,
    pub data: T,
}
```

`DecisionSearchResults` is:

```rust
pub struct DecisionSearchResults {
    pub query: Option<String>,
    pub filters: SearchDecisionFilters,
    pub order: SearchDecisionOrder,
    pub ledger_range: SearchLedgerRange,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: Option<usize>,
    pub items: Vec<DecisionSearchResult>,
}
```

`total_matches` may be omitted by remote backends when exact counts would make
the query too expensive. Omission is explicit; it must not be encoded as zero.

Each result item contains:

```rust
pub struct DecisionSearchResult {
    pub decision: DecisionView,
    pub status_context: DecisionStatusContext,
    pub rank: u16,
    pub rank_basis: Vec<SearchRankBasis>,
    pub matched_fields: Vec<String>,
    pub snippets: Vec<SearchSnippet>,
    pub graph_context: SearchGraphContext,
    pub provenance: SearchResultProvenance,
}
```

Required result behavior:

- `decision.status` is derived from explicit graph edges at query time.
- `status_context` explains contested, superseded, and stale states with
  explicit ids and event origins when available.
- `rank` is an ordinal for deterministic ordering, not a confidence score.
- `rank_basis` names why the row landed where it did, such as
  `exact_decision_id`, `title_match`, `rationale_match`,
  `graph_context_match`, `newer_event`, or `decision_id_tiebreaker`.
- `matched_fields` uses stable field names so clients can display and test
  field hits without knowing the storage backend.
- `snippets` are bounded and must not be the only place a match is represented.
- `graph_context` includes ids for actors, options, evidence, hypotheses,
  supersession edges, stale reasons, and matched nodes.
- `provenance` includes the decision's creating event origin and citation ids
  or event origins for matched context when the projection exposes them.

## MCP Surface

MCP exposes the same operation as a read-only tool named `search_decisions`.
It must not require an `actor_id`, because it does not write. It returns the
same query envelope as CLI JSON.

Input schema sketch:

```json
{
  "type": "object",
  "properties": {
    "query": { "type": "string" },
    "filters": {
      "type": "object",
      "properties": {
        "topic_keys": { "type": "array", "items": { "type": "string" } },
        "topic_match": { "type": "string", "enum": ["any", "all"] },
        "statuses": {
          "type": "array",
          "items": {
            "type": "string",
            "enum": ["proposed", "accepted", "rejected", "contested", "superseded"]
          }
        },
        "actor_ids": { "type": "array", "items": { "type": "string" } },
        "actor_match": { "type": "string", "enum": ["any", "all"] },
        "sources": { "type": "array", "items": { "type": "string" } },
        "source_refs": { "type": "array", "items": { "type": "string" } },
        "field_scopes": { "type": "array", "items": { "type": "string" } },
        "premised_on_hypothesis_ids": { "type": "array", "items": { "type": "string" } },
        "based_on_evidence_ids": { "type": "array", "items": { "type": "string" } },
        "supersedes_decision_ids": { "type": "array", "items": { "type": "string" } },
        "superseded_by_decision_ids": { "type": "array", "items": { "type": "string" } },
        "staleness": { "type": "string", "enum": ["any", "fresh", "stale"] },
        "stale_reasons": { "type": "array", "items": { "type": "string" } },
        "since_offset": { "type": "integer", "minimum": 0 },
        "until_offset": { "type": "integer", "minimum": 0 },
        "since": { "type": "string", "format": "date-time" },
        "until": { "type": "string", "format": "date-time" }
      }
    },
    "order": { "type": "string", "enum": ["match", "newest", "oldest", "decision_id"] },
    "limit": { "type": "integer", "minimum": 0 },
    "cursor": { "type": "string" }
  }
}
```

Return schema sketch:

```json
{
  "result_count": 25,
  "truncated": true,
  "latency_ms": 4,
  "data": {
    "query": "authentication",
    "filters": {},
    "order": "match",
    "ledger_range": {
      "from_offset_exclusive": 0,
      "to_offset_inclusive": 1842,
      "resolved_since": null,
      "resolved_until": null
    },
    "limit": 25,
    "cursor": null,
    "next_cursor": "opaque",
    "total_matches": 83,
    "items": [
      {
        "decision": {},
        "status_context": {},
        "rank": 20,
        "rank_basis": ["title_match", "decision_id_tiebreaker"],
        "matched_fields": ["decision.title"],
        "snippets": [{ "field": "decision.title", "value": "..." }],
        "graph_context": {},
        "provenance": {}
      }
    ]
  }
}
```

## Internal Query Function

The internal API should remain a pure layer-2 query function:

```rust
pub fn search_decisions(
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
) -> Result<QueryResponse<DecisionSearchResults>>;
```

If a future backend can answer search more efficiently than `GraphView`, add an
adapter trait around the read side rather than changing CLI or MCP semantics:

```rust
pub trait DecisionSearchRead {
    fn search_decisions(
        &self,
        request: &SearchDecisionRequest,
    ) -> Result<QueryResponse<DecisionSearchResults>>;
}
```

The trait is read-only. It cannot append events, mutate projection state, call
LLMs, or run background indexing as part of a query. Index maintenance may be a
separate rebuildable projection process, but the query path observes only an
explicit ledger watermark.

## Ordering Guarantees

Default `order=match` is deterministic relevance by explicit match basis:

1. Exact decision id match.
2. Exact decision title match.
3. Decision title term match.
4. Decision rationale term match.
5. Direct decision metadata match: topic, status, source, source ref.
6. One-hop graph context match: actor, option, evidence, hypothesis, and
   supersession ids or text.
7. Staleness or blocker context match.
8. Stable tie-breaker.

The tie-breaker should be `decision_id` for local deterministic projections.
Remote services may use `(ledger_watermark, decision_id)` or another stable key
as long as identical inputs at the same ledger state return identical order.

`order=newest` and `order=oldest` sort by explicit decision event time or event
offset after filters are applied. They do not turn search into ranking by
popularity or confidence.

Layer-3 ranking may reorder layer-2 rows only in a separate response that
preserves the original deterministic order, cites the layer-2 row ids, and
explains the score basis.

## Pagination And Limits

Every implementation must bound responses. The recommended default limit is 25
and the recommended hard cap is 1000 unless a deployment sets a lower cap for
service protection.

Pagination rules:

- A non-null `next_cursor` means there are more rows for the same normalized
  request at the same ledger state.
- `truncated` is true whenever `next_cursor` is non-null or the backend hit a
  protective cap before proving there are no more rows.
- Cursors are scoped to the normalized request, order, and ledger watermark.
- Changing any query parameter invalidates the cursor.
- Backends may use offset or keyset pagination internally. The cursor encoding
  is not part of the public contract.

No response may silently omit rows because a backend limit was reached.

## Storage-Bound Behaviors

These behaviors may vary by backend and must not leak into the public contract
unless HiveMind explicitly versions the contract:

- FTS5 tokenizer details, stemming, prefix matching, phrase grammar, `MATCH`
  syntax, BM25 values, snippet highlighting, and offset APIs.
- SQL collation rules and Unicode case folding beyond the documented
  case-insensitive minimum.
- Whether exact `total_matches` is cheap enough to return.
- Whether pagination is implemented with numeric offsets, keysets, row ids, or
  service cursors.
- Whether a text index is synchronous, asynchronous, persistent, or rebuilt on
  demand.
- Native graph query syntax such as Cypher, Kuzu-specific operators, or SQL
  table names.

The public contract is the normalized request, deterministic result envelope,
visible truncation, explicit rank basis, and provenance-bearing decision graph
context.

## Semantic Search Boundary

Semantic search, vector similarity, clustering, summarization, and natural
language contradiction detection are layer-3 capabilities.

Layer 3 may:

- Generate candidate layer-2 search requests from a user question.
- Run one or more bounded `search_decisions` calls.
- Re-rank or summarize returned rows if it reports the basis and citations.
- Ask the user to inspect contested, stale, or truncated results.

Layer 3 must not:

- Append or alter ledger events as part of search.
- Hide `contested`, superseded, stale, or truncated states.
- Replace deterministic status derivation with model judgment.
- Present a semantic score as provenance.
- Deduplicate disagreements because two results "look similar."

If a semantic answer says "Claude contradicted Codex," the answer must cite the
explicit contested decisions or explain that the claim is model-inferred and
not a layer-2 fact.

## Migration Narrative

A backend migration should not change CLI flags, MCP tool names, or result
schemas. The safe migration path is:

1. Preserve the event ledger as the audit source and define the new backend as
   a rebuildable read model or service projection.
2. Implement `DecisionSearchRead` for both the current backend and the new
   backend.
3. Run a shared search conformance corpus covering text fragments, status,
   contested decisions, refuted hypotheses, supersessions, actor filters, time
   windows, truncation, empty pages, and cursor continuation.
4. Compare normalized JSON responses. Ignore storage-specific latency and
   cursor byte strings; require equal ids, statuses, matched fields, truncation
   semantics, rank basis, and provenance.
5. Dual-read in development or CI until parity is stable.
6. Switch the backend behind CLI, MCP, and service adapters without changing
   the operation name or DTO.
7. Keep old cursors invalid across the backend cutover and return a clear
   cursor-version error rather than mispaging.

FTS5 can be an implementation detail for a local SQLite baseline, but the
contract above is what survives replacement by another text index, graph
database, or remote HiveMind service.
