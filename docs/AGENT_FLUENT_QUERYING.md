# Agent-Fluent Follow-Up Verbs — Design Document (tenv.1)

Status: **DRAFT — awaiting alex review before implementation begins**
Bead: hivemind-tenv.1
Parent epic: hivemind-tenv
Author: gastown.rictus (polecat), 2026-09-07

---

## 0. Purpose and Scope

This is a **plan-first deliverable**: no product code beyond the exploratory
reading already done for this document is written until alex reviews and
approves the design (see docs/INGESTION_CONNECTORS.md for the precedent this
document follows).

Today the only free-text entry points into HiveMind's query layer are
`recall <query>` (`src/cli/args.rs:1007`, positional `Option<String>`),
`search --q <query>` (`src/cli/args.rs:986`), and
`get_relevant_decisions --topic <topic>` (`src/cli/args.rs:950`). Every
follow-up verb — `disagree`, `supersede`, `get_supersession_chain`,
`get_decision_neighborhood`, `compact-view` — requires `--id`
(`src/cli/args.rs:281,290,944,959,912`), and every equivalent MCP tool
requires `decision_id` as a required string argument (`src/mcp.rs:395`,
`:441`, `:464`, `:546`). An agent that just ran `recall` knows a decision's
*content*, not an opaque id, unless it round-trips the id back out of the
JSON response first. This document designs the primitive that closes that
gap and the contract every follow-up verb adopts.

Scope (this bead, tenv.1):
- One shared resolve-by-description primitive in `src/queries`.
- Free-text positional input (with `--id` / `--pick N` escape hatches) on:
  `disagree`, `supersede`, `get_supersession_chain`, `get_decision_neighborhood`
  (aliased `why`), `compact-view`, and a **new** `review` query verb backed by
  `get_decision_outcome` (see §3.1 — this is not the existing `review`
  subcommand).
- A shared output contract (`DecisionBrief`) used by every verb above.
- The short-handle continuation mechanism (`#N`, `--from-last`).
- Tests on both backends (SQLite `MemoryGraph`/`SqliteEventLedger` +
  `shared-backend-postgres` feature).

Out of scope (owned by siblings per the parent epic):
- `hivemind query situational` (working name at the time this document was
  written: `hivemind query context`; naming locked 2026-09-08 to
  `situational` — see hivemind-tenv's bead notes) — reuses this bead's
  resolver module if it lands first; coordinate through bead comments per
  tenv.2's description, do not fork a second ranker.
- The `hivemind-context` Claude/Codex plugin (tenv.3).
- Repointing `hivemind-capture`'s query script and docs/website (tenv.4).
- MCP tool schema changes are **sketched** in §3.4 for parity but the MCP
  server wiring itself is implementation work under this same bead once the
  design is approved — flagged, not deferred to another bead.

**Relationship to existing search design docs:** `docs/SEARCH_DESIGN.md`
already states the mission this bead completes — "the caller can find a
decision without already knowing its id, then decide whether to inspect,
cite, contest, or **supersede** it" (`SEARCH_DESIGN.md:16`) and "Offset
bounds are canonical. Timestamp bounds are resolved to concrete event
offset bounds" (`SEARCH_DESIGN.md:184`) — but it only specifies the *find*
step; contest/supersede-by-description was never designed, and that offset-
canonical doctrine directly supports treating `event_origin` as the
canonical recency signal (§1.2) rather than treating the missing wall-clock
timestamp as a blocking gap. `docs/DECISION_SEARCH_QUERY.md` documents the
same rank-tier system this design reuses (§1.1 below).

---

## 1. The Resolver Primitive

### 1.1 Why generalize rather than invent

`src/queries/search.rs` already contains a deterministic, backend-agnostic
matcher: `collect_graph_search_results` (`search.rs:569`) builds
`ScoredDecisionSearchResult`s by calling `evaluate_search_match`
(`search.rs:838`), which computes a `rank: u8` tier per decision — `0` for an
exact id/title match (`exact_id_or_title_match`, `search.rs:906`), otherwise
`min` over matched-field ranks, requiring **all** query terms
(`shared::query_terms`, `shared.rs:30` — lowercased whitespace split) to
match somewhere (AND semantics). This already runs against `impl GraphView`,
so it works unmodified against both `MemoryGraph`/SQLite-projected graphs and
`PostgresGraphView` (proven by `src/projector/postgres/tests.rs:14`, which
imports `search_decisions` directly and asserts parity against `MemoryGraph`
via `with_postgres_graph`, `tests.rs:227`).

**Decision:** the resolver is not a new ranking algorithm. It is
`collect_graph_search_results`'s tier system, extracted into a shared
function and extended with a recency tiebreak, wrapped in an ambiguity gate.
No new matching logic, no configuration knobs, no learned weights.

### 1.2 Recency, for free

Every node carries `event_origin` — the ledger offset at creation
(`origin_properties`, `src/projector/mod.rs:423-429`; merged onto every
`Decision` node by `project_decision_proposed`, `src/projector/mod.rs:485`).
`event_origin` is monotonically increasing, identical in meaning on both
backends, and is already used elsewhere as a recency proxy (`gap_events` in
`OutcomeReason::SupersededBy`, `src/queries/outcome.rs:40`). The resolver
reads it as a plain `Decision.event_origin` graph property (one extra column
in the existing `node_rows(graph, NodeKind::Decision)` scan,
`search.rs:578`) — no ledger access, no new backend-specific code path.

There is **no wall-clock timestamp on the projected `Decision` node today**
(only `event_origin`/`source`/`source_ref`/`tenant_id`, per
`origin_properties`). `decision_proposed_at_by_id` (`search.rs:462`) reads a
real timestamp, but from the SQLite ledger directly — not backend-agnostic,
not available for Postgres. This is a real gap for the output contract's
"when" field (§4) and is called out as Open Question 1.

### 1.3 Primitive contract

```rust
// src/queries/resolve.rs (new)

/// One candidate returned by the resolver, ordered by descending confidence.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolvedCandidate {
    pub decision_id: String,
    pub title: String,
    pub rank: u8,              // reused tier from evaluate_search_match; 0 = exact
    pub event_origin: i64,     // recency tiebreak, ascending rank order first
    pub matched_fields: Vec<String>,
}

/// Outcome of a resolve-by-description call: either a confident single
/// match, or a candidate list the caller must disambiguate.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ResolveOutcome {
    Resolved { candidate: ResolvedCandidate },
    Ambiguous { candidates: Vec<ResolvedCandidate> },
    NotFound,
}

pub fn resolve_decision_by_description(
    graph: &impl GraphView,
    description: &str,
    topic_hint: Option<&str>,   // optional --topic narrowing, same param search already accepts
) -> Result<QueryResponse<ResolveOutcome>>;
```

`ResolveOutcome` is a **value**, not an `Err`. Ambiguity is an expected,
common outcome (most descriptions will match more than zero decisions early
in a ledger's life) — modeling it as an error would force every caller into
exception-flavored control flow for the normal case, and would contradict
the project's own stance that `contested` is a status, not an error
(AGENTS.md §6). `NotFound` is likewise a value: zero matches is informative,
not exceptional.

### 1.4 Ambiguity gate — confidence margin

Given the sorted candidate list (primary key: `rank` ascending, secondary
key: `event_origin` descending — newest wins ties), define:

- **Resolved** iff there is exactly one candidate at the best `rank` tier,
  **or** the best-tier candidate's `event_origin` is strictly newer than
  every other candidate sharing that tier by more than a fixed margin (see
  Open Question 2 for the margin value — this needs a real number, not a
  polecat's guess, because it is the one knob that trades recall against
  silent-wrong-pick risk).
- **Ambiguous** otherwise: two or more candidates share the best rank tier
  with no clear recency separation. Return the full candidate list (capped
  at `MAX_QUERY_RESULTS`, `shared.rs:12`), each numbered `#1..#N` in the
  order returned (§5).
- For **write verbs** (`disagree`, `supersede`) the gate is strict: even a
  single-tier-apart near-tie forces `Ambiguous`. Read verbs (`why`,
  `compact-view`, `review`) may be slightly more permissive since a wrong
  read is cheap to notice and re-query; a wrong write is not. This
  asymmetry is deliberate and should be confirmed at the checkpoint (Open
  Question 3).

`--pick N` selects candidate `#N` from the *immediately preceding* resolver
call in the same invocation (used together with `--from-last`, §5) or is
rejected with a clear error if there is no prior candidate list to pick
from. `--id <decision_id>` bypasses resolution entirely — the existing
`decision_node_exists` check (`shared.rs:15`) becomes the validation path
for both the new positional-description mode and the legacy `--id` mode, so
`--id` behavior for existing scripts/callers is unchanged.

---

## 2. Ambiguity Is Not an Error — Compliance Note (Principles 1 & 7)

Principle 1: "No smart behavior — no search, ranking, summarization, model
call, similarity, or recommendation — ever touches the write path"
(`PRINCIPLES.md:15`). Principle 7: "Layer 2 (query) does not call LLMs,
score, cluster, or rank" (`PRINCIPLES.md:82`).

Read literally, "does not... rank" appears to forbid exactly the mechanism
this design reuses. In practice the codebase already draws a narrower line:
`search.rs`'s `rank: u8` tiering and the SQLite FTS path's `bm25` score
(`search.rs:416`) are both deterministic, non-learned, non-probabilistic
functions of exact string containment and term frequency — no model call, no
embedding, no similarity metric, nothing that could disagree with itself
between two runs on the same data. That is the reading the bead text itself
takes ("deterministic: term match + topic + recency... no LLM — PRINCIPLES
1/7"). This design does not push that line further: it reuses the existing
tier system unmodified and adds one new deterministic field
(`event_origin`, an integer ledger offset, already graph-native). No
embeddings, no fuzzy/edit-distance matching, no learned weights are
introduced. Confirming this reading is correct is Open Question 4 — it is
the load-bearing constraint for the whole bead, and "the existing code
already does something adjacent" is not the same as "alex has signed off on
extending it."

---

## 3. Verb-by-Verb Contract

Positional free text follows the `recall` precedent exactly: `pub query:
Option<String>` as the first positional field on each `*Args` struct
(`QueryRecallArgs`, `args.rs:1007`), so existing flag-only invocations are
unaffected (the field is optional) and `--id` continues to work unchanged.

| Verb | Today | Change |
|---|---|---|
| `disagree` | `--decision <id> --reason <r>` (`args.rs:281`) | add positional `Option<String>` description; `--decision`/`--id` stays the escape hatch |
| `supersede` | `--old <id> --title ... --rationale ...` (`args.rs:290`) | add positional description resolving `--old`; rest unchanged |
| `get_supersession_chain` | `--id <id>` (`QueryDecisionArgs`, `args.rs:940`) | add positional description; **new alias** `chain` (friendlier name per bead item b) |
| `get_decision_neighborhood` | `--id <id> --depth --relations --compact` (`args.rs:959`) | add positional description; **new alias** `why` |
| `compact-view` | `--id <id>` (`QueryDecisionArgs`) | add positional description |
| `review` (**new**) | does not exist as a single-decision verb today | see §3.1 |

### 3.1 The `review` naming collision

The bead lists `review` among the verbs to make fluent, glossing it as
"Verify: 'did that hold up?' by description" (parent hivemind-tenv,
item 4). But the CLI already has a `review` subcommand
(`ReviewArgs`, `args.rs:317`) that is a **bulk filter report** — actor glob
patterns, a `since`/`until` window, `--unreviewed-only` — with no
`decision_id` field at all. It answers "what decisions need review", not
"did this one decision hold up." The verb that actually answers "did that
hold up" is `get_decision_outcome` (`src/queries/outcome.rs:94`,
`DecisionOutcome.held_up` + structured `reasons`), which today is **MCP-only**
(`src/mcp.rs:1086`) with no CLI query subcommand at all.

**Recommendation:** add `get_decision_outcome` as a new CLI query subcommand
(alias `review`, matching the bead's naming and the parent's "did it hold up"
framing) accepting the same positional-description contract as the other
verbs. Leave the existing bulk `review` subcommand untouched — it is a
different, filter-shaped verb that was never a candidate for
resolve-by-description (it has no single target to resolve). This is Open
Question 5: confirm the naming does not collide confusingly once both exist
side by side (`hivemind review` = bulk report, `hivemind query review
<description>` = single-decision outcome check) — an alternate name
(`verify`, `held-up`) may be clearer as the query-subcommand alias.

### 3.2 `--pick N` and `--topic`

`--topic` is added as an optional narrowing filter on every fluent verb
(mirrors `get_relevant_decisions --topic`, `args.rs:950`, and
`search --topic`, `args.rs:981`) — passed through to
`resolve_decision_by_description` as `topic_hint`. `--pick N` is added to
every fluent verb per §1.4/§5.

### 3.3 Backward compatibility

No existing flag is removed or renamed. `--id` keeps its current behavior
and error semantics on every verb (`decision_node_exists`,
`shared.rs:15`, unchanged). Existing scripts, the `hivemind-capture` plugin,
and MCP callers that always pass `decision_id` are unaffected — this is a
strictly additive contract change.

### 3.4 MCP parity (sketch)

Every MCP tool above (`disagree_decision`, `supersede_decision`,
`get_supersession_chain`, `get_decision_neighborhood`/`get_decision_context`,
`hivemind_compact_view`, and a new `get_decision_outcome` free-text path)
gets a sibling optional `description` string parameter alongside the
existing required `decision_id` (making `decision_id` optional when
`description` is present, and vice versa — tool schemas enforce "exactly
one of" via a `oneOf`/description text since JSON Schema's `required` can't
express mutual exclusion cleanly; matches how `tool_disagree_decision`
already validates required args at `mcp.rs:832`). The ambiguity gate returns
the same `ResolveOutcome::Ambiguous` payload over MCP as over CLI JSON
output — an agent calling MCP tools gets the same candidate-list-plus-pick
UX, not a silently different contract. Full wiring is implementation work
for this bead once the design is approved, not deferred to tenv.3 (the
plugin is CLI-only and depends on these verbs already working, including
over MCP where Claude/Codex use MCP rather than shelling out).

---

## 4. Output Contract — `DecisionBrief`

No existing query response leads with "the decision, then why, then who,
then does it still hold" — the pieces exist but are scattered across three
separate query functions:

- `get_decision` (`decision.rs:37`) → `DecisionView`: id, title, rationale,
  `option_ids` (ids only, **no labels** — see below), `chosen_option_id`,
  status, evidence/hypothesis ids.
- `get_decision_context` (`context.rs`) → `DecisionContext`: `proposer_id`,
  `source`, `source_ref`, `review: ReviewShape` (unreviewed / self_accepted /
  peer_reviewed / disputed) — this is "who decided."
- `get_decision_outcome` (`outcome.rs:94`) → `DecisionOutcome`: `held_up`
  plus structured `reasons` (superseded / stale premises / contested / thin
  structure) — this is "STILL-HOLDS."

**Known gap:** `option_ids` are ids, not labels. Option node `label` is a
real graph property (`opt_props.insert("label", ...)`,
`src/projector/mod.rs:1052`) reachable via `node_rows(graph,
NodeKind::Option)` (already scanned by `collect_graph_search_results`,
`search.rs:581`), but no existing query surfaces it. There is also **no
per-option rejection rationale** anywhere in the graph schema — only
`ForeclosedOptionAttribute.description` (`events.rs:521`), which belongs to
the classifier/ingest extraction pipeline, not the core `Decision`/`Option`
model that `get_decision` reads. "REJECTED options + why" therefore means:
resolve each non-chosen `option_id` to its `label` (new lookup, small
addition), and reuse the single decision-level `rationale` string as the
shared "why" for the choice among options — there is no way to show a
*distinct* rejection reason per option today without a capture-side schema
change, which is out of scope. This is stated as a limitation in the
rendered output, not silently glossed over (AGENTS.md §6: no invented
confidence).

```rust
// src/queries/resolve.rs or a new src/queries/brief.rs

pub struct DecisionBrief {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    pub chosen_option: Option<OptionLabel>,
    pub rejected_options: Vec<OptionLabel>,   // label-resolved, rationale shared not per-option
    pub decided_by: DecidedBy,                // proposer_id, source, source_ref, review shape
    pub still_holds: StillHolds,              // held_up + reasons, from DecisionOutcome
    pub topic_keys: Vec<String>,
    pub status: DecisionStatus,               // proposed/accepted/contested/superseded
}

pub struct OptionLabel { pub option_id: String, pub label: String }
pub struct DecidedBy { pub proposer_id: Option<String>, pub source: String, pub source_ref: Option<String>, pub review: ReviewShape }
pub struct StillHolds { pub held_up: bool, pub reasons: Vec<OutcomeReason> }
```

`get_decision_brief` composes the three existing query calls plus the new
option-label lookup — it does not duplicate their logic, and it stays
`Layer 2`: three deterministic graph reads, no write, no LLM.

Every fluent verb's output, on success, leads with a `DecisionBrief` (or a
list of them for multi-decision verbs like `get_supersession_chain`). JSON
is the default (matches `QueryArgs`'s existing convention,
`args.rs:887,889`); `--summary` renders compact text through the existing
generic dispatcher `format_query_response` (`render.rs:171`), following the
established pattern of a per-verb `render_*_summary(&T) -> String` closure
(e.g. `render_neighborhood_summary`, `render.rs:342`) — but as short
labeled paragraphs (decision / rationale / rejected options / decided by /
still holds), not the tab-cell table style used by list-shaped commands
(`summary_cell`, `render.rs:456`), since a `DecisionBrief` is a single
record meant to be read, not a row in a list. IDs appear only in a trailing
"ref: <decision_id>" line — present for follow-up (`--id`, `--pick`), never
required reading to understand the answer.

---

## 5. Short-Handle Continuation

No existing precedent (`resolve`, `ambiguity`, `--pick`, `candidate` do not
appear anywhere in `src/` outside `suggest.rs`'s unrelated document-extraction
candidates, per grep — this is new ground).

**Design:** every resolver-backed response — both `Ambiguous` candidate
lists and `Resolved` single answers — writes its candidate set to a
session-local file: `--from-last <path>` (default
`$HIVEMIND_LAST_CANDIDATES` env var if set, else a fixed path under the
CLI's existing config/state directory — same directory class already used
for local ledger state, not `/tmp`). The file holds the numbered candidate
list (`decision_id` + title + rank, JSON) from the most recent resolver
call. A subsequent invocation's `#N` positional argument (recognized by a
`#` prefix, distinct from a free-text description) reads that file, picks
entry `N`, and resolves directly — equivalent to `--pick N` but addressable
across separate CLI invocations within a session, which `--pick N` alone
(single-invocation only, per §1.4) cannot do.

This keeps the mechanism simple: no server-side session state, no new
ledger writes (continuation state is not decision memory — it's UI
convenience and explicitly does not belong in the ledger per AGENTS.md §2,
"not a chat archive or conversational memory store"), one file, overwritten
on every resolver call. Concurrent sessions on the same machine sharing the
same file is an accepted rough edge for v1 (Open Question 6: is a
session/PID-scoped filename needed, or is "last resolver call in this
directory" fine for a single-engineer-plus-agents tool per the VISION.md
"who it's for" framing?).

---

## 6. Test Plan

### 6.1 Resolver + ambiguity gate (new)

Unit tests in `src/queries/tests.rs` style (in-process `MemoryGraph`,
synchronous, no I/O) covering: exact-title resolve, exact-id resolve,
single-term substring match with no competitor (Resolved), two decisions
sharing a title substring with no recency separation (Ambiguous, both
listed, numbered), the same case with the write-verb strict gate applied
(still Ambiguous even where the read-verb margin would resolve it, once
Open Question 3 sets the actual thresholds), `--pick N` out of range
(clear error, not a panic), `#N` with no prior candidate file
(clear error), zero matches (`NotFound`), and `--topic` narrowing a
multi-match description down to one.

### 6.2 Dual-backend parity (existing precedent, extended)

`src/projector/postgres/tests.rs` already runs `crate::queries::{get_decision,
get_supersession_chain, search_decisions}` against both `MemoryGraph` and
`PostgresGraphView` via `with_postgres_graph` (`tests.rs:227`, gated on
`HIVEMIND_TEST_POSTGRES_URL` and the `shared-backend-postgres` feature,
skips cleanly when the env var is unset). `resolve_decision_by_description`
and `get_decision_brief` are added to that same parity test — same fixture
ledger, same ranked-candidate assertion, both backends, one test body. This
is the established pattern in this codebase for dual-backend query
coverage; no new test harness is needed.

### 6.3 Reference docs

`cargo run --bin generate-reference -- --check` must pass — every new/changed
CLI subcommand and flag needs its `--help` text updated so the generated
`cli.md` reference stays accurate (per AGENTS.md §7 mandatory gate).

---

## 7. Open Questions for Alex

1. **`event_origin`-as-recency vs. a real timestamp.** `event_origin` stays
   canonical for the resolver's *ranking* — that matches existing doctrine
   ("Offset bounds are canonical. Timestamp bounds are resolved to concrete
   event offset bounds", `SEARCH_DESIGN.md:184`) and needs no design change.
   The open question is narrower: should the projector *also* stamp
   `occurred_at` onto the `Decision` node (small, deterministic,
   Layer-1-compliant addition to `project_decision_proposed`,
   `projector/mod.rs:485`) purely so `DecisionBrief`'s "when" is a date an
   agent can read, rather than a bare ledger offset? Recommendation: yes,
   as a *display* field only, never as a ranking input — the output
   contract's "when" is explicitly required and an offset alone is a worse
   answer for a human/agent reading the brief, even though it stays correct
   for the resolver itself.

2. **Confidence margin value.** What recency gap (in `event_origin` units,
   or as "top candidate must be the only one at best rank" with no numeric
   margin at all) counts as "clearly ahead" for the ambiguity gate?
   Recommendation: start with **no numeric margin** — Resolved only when
   exactly one candidate occupies the best rank tier; any tie at the best
   tier is Ambiguous, full stop. This is the simplest rule that cannot
   silently pick the wrong one of two similarly-recent, similarly-matching
   decisions, and it can be loosened later (layer-3 candidate, not layer-2)
   if it proves too conservative in practice.

3. **Read vs. write gate asymmetry.** Confirm read verbs (`why`,
   `compact-view`, `review`) may use the same gate as write verbs
   (`disagree`, `supersede`), or should be strictly identical for v1.
   Recommendation: identical gate for all verbs in v1 (simpler, and matches
   §7.2's "no numeric margin" recommendation which leaves no daylight to
   asymmetrize anyway) — revisit only if it proves too conservative for
   read verbs in practice.

4. **Principle 1/7 reading.** Confirm the "deterministic tiering + ledger-
   offset recency, no embeddings/fuzzy matching/learned weights" reading of
   "Layer 2 does not... rank" (§2) is the intended one — this is the
   load-bearing constraint for the entire bead.

5. **`review` naming collision.** New CLI query alias for
   `get_decision_outcome` — keep `review` (matches the bead's own wording,
   collides in spirit with the existing bulk `review` report subcommand) or
   use a different alias (`verify`, `held-up`)? Recommendation: `verify`,
   with `review` left free for the existing bulk-report meaning, since
   having `hivemind review` (bulk) and `hivemind query review <desc>`
   (single-decision) coexist under the same word is exactly the kind of
   ambiguity this whole bead exists to eliminate for agents.

6. **Continuation file scope.** Single fixed path (last resolver call,
   process-global) or session/PID-scoped? Recommendation: fixed path for
   v1, matching the single-engineer-plus-agents scope in VISION.md.

7. **`get_supersession_chain` alias name.** Bead item (b) asks for "a
   friendlier alias" but does not name it. Recommendation: `chain`.

---

## 8. Layers Compliance Check

| Concern | Layer | Verdict |
|---|---|---|
| `resolve_decision_by_description` (term match, rank tiering, recency) | Layer 2 | Deterministic graph read reusing `evaluate_search_match`'s existing tiering; no LLM, no learned weights |
| Ambiguity gate | Layer 2 | Pure comparison over already-computed ranks/`event_origin`; returns a value, never throws |
| `get_decision_brief` | Layer 2 | Composes three existing Layer-2 query functions + one new deterministic node-property read (`Option.label`) |
| `--pick N` / `#N` continuation file | CLI-layer UX, not a query or a ledger write | Local convenience state only; explicitly not decision memory (AGENTS.md §2) |
| Optional `occurred_at` projector stamp (Open Q1) | Layer 1 write | Deterministic, no LLM — same shape as existing `event_origin`/`source` stamping |

No layer boundary is crossed. The write path (`disagree`, `supersede`) only
gains a *resolution step before* the write — the write itself
(`decision.disagreed`/`decision.superseded` event emission) is unchanged and
still keyed by a concrete `decision_id`, exactly as today, once resolved.

---

## 9. Summary and Next Steps

This design adds one shared, deterministic resolve-by-description primitive
(reusing `search.rs`'s existing rank-tiering, adding an `event_origin`
recency tiebreak and a conservative no-numeric-margin ambiguity gate),
extends six CLI verbs (plus their MCP equivalents) to accept free text with
`--id`/`--pick N` as escape hatches, introduces a shared `DecisionBrief`
output contract composing three existing query functions, and a simple
file-backed `#N` continuation mechanism. It surfaces one genuine schema gap
(no per-option rejection rationale) honestly rather than papering over it,
and one naming collision (`review`) that needs a decision before
implementation, not after.

**Immediate next steps (after alex sign-off):**
1. Alex resolves §7 open questions 1-7 (margin value, `occurred_at`
   stamping, `review` naming, and the Principle 7 reading in particular).
2. Implement `src/queries/resolve.rs` (primitive + ambiguity gate) and its
   dual-backend parity tests (§6.2).
3. Implement `get_decision_brief` and the `--summary` renderers.
4. Wire the six CLI verbs + `get_decision_outcome`'s new CLI subcommand +
   MCP tool parity (§3.4).
5. `generate-reference --check`, full gate set, draft PR for CI validation.

Nothing in steps 2-5 begins before alex reviews §7.
