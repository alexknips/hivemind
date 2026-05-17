# HiveMind — Plan v3 (slimmed to decided scope)

**Author tag:** independent agent.
**Purpose:** capture organizational memory of decisions (retro-style: what was decided, why, by whom, what was considered). Not agent memory, not project management. Multi-agent, multi-human, beyond coding.

---

## 0. The three-layer architecture (the core idea)

The system is strictly partitioned into three layers. **Anything "smart" lives in layer 3 only.**

```
                Layer 3 — Agentic suggestion / analysis
                  • compactification ("semantic forgetting")
                  • similarity / "find related decisions as seeds"
                  • ranking, summarization, recommendations
                  • slice 2+ only; design open
                        ▲
                        │ consumes
                Layer 2 — Query / read
                  • pure reads over the projected graph
                  • no LLMs, no scoring, no clustering
                  • deterministic, bounded, fast
                        ▲
                        │ reads
                Layer 1 — Write / ingest
                  • commands module → event ledger → projector
                  • validates invariants, appends events, projects state
                  • dumb-but-correct
```

Slice 1 = layers 1 + 2 only. Layer 3 is slice 2+ and intentionally undesigned right now.

---

## 1. Slice 1 — what we are building

**Goal:** prove that Decisions, Evidence, Options, and Hypotheses can be recorded, that supersession links work, and that queries return the live state correctly. Single-process, embedded, no server, no smartness.

### 1.1 Stack

- **Rust crate** `hivemind` with trait-first API (`EventLedger`, `GraphView` traits + default impls).
- **SQLite (WAL)** as the event ledger.
- **Kuzu (embedded)** as the graph projection.
- **CLI** subcommands: `emit`, `query`, `dump`.
- **On-disk layout:** `./hivemind/ledger.sqlite` and `./hivemind/graph.kuzu` in the current working directory.

**Deliberately not in slice 1:** HTTP API, MCP server, Postgres, Neo4j, signing, compaction (any flavor), similarity lookup, telemetry beyond basic logs.

### 1.2 Entities (5)

- **Decision** — a choice (proposed/accepted/rejected/superseded; status *derived from edges*, see §1.4).
- **Actor** — agent / human / system. Identified by id; no signing in slice 1.
- **Evidence** — supporting material (text, link, file ref). Established fact-like.
- **Option** — an alternative considered before a decision was made. First-class node (so it can later be evidenced individually).
- **Hypothesis** — a claim that's not yet verified. Distinct from Decision (it isn't acted on) and Evidence (it isn't established). Has its own lifecycle (`open | supported | refuted`) so multiple decisions can rest on it, and later evidence can flip all of them at once.

**Topic** is a normalized **string property** (`topic_keys: [string]`) on Decision. *Not* a node. Normalization (lowercase, slug) happens in the commands layer.

### 1.3 Relations (10)

| Relation | From → To | Meaning |
|---|---|---|
| `PROPOSED_BY` | Decision → Actor | "X proposed this" |
| `ACCEPTED_BY` | Decision → Actor | "X accepted this" |
| `REJECTED_BY` | Decision → Actor | "X rejected this" |
| `SUPERSEDES` | NewDecision → OldDecision | "B replaces A" |
| `BASED_ON` | Decision → Evidence | "this decision rests on this evidence" |
| `HAS_OPTION` | Decision → Option | "this option was considered" |
| `CHOSE` | Decision → Option | "we picked this one" |
| `ASSUMES` | Decision → Hypothesis | "this decision rests on this claim" |
| `SUPPORTS` | Evidence → Hypothesis | "this evidence corroborates the claim" |
| `REFUTES` | Evidence → Hypothesis | "this evidence undermines the claim" |

**Excluded from slice 1** (→ slice 2): `DEPENDS_ON`, `CONTRADICTS`, anything topic-related (topic is a property, no edge), anything compaction-related (`COMPACTS`, `PART_OF`, etc.).

### 1.4 Status (derived, not stored)

A `Decision`'s status is **computed at query time** from its edges, not stored as a property:

| Edges present | Status |
|---|---|
| any incoming `SUPERSEDES` from another Decision | `superseded` |
| `ACCEPTED_BY` exists, `REJECTED_BY` does not | `accepted` |
| `REJECTED_BY` exists, `ACCEPTED_BY` does not | `rejected` |
| Both `ACCEPTED_BY` and `REJECTED_BY` exist | `contested` |
| Only `PROPOSED_BY` so far | `proposed` |

`contested` is surfaced explicitly — disagreement is information, not an error to swallow.

Hypothesis status (`open | supported | refuted`) is derived similarly: it's `supported` if it has ≥1 `SUPPORTS` and 0 `REFUTES`, `refuted` if it has ≥1 `REFUTES`, else `open`. (Tie-breaking + thresholds are slice-2 questions for layer 3.)

### 1.5 Event envelope

Every event carries:

- `event_id` — monotonic sequence assigned by the ledger on append
- `event_uuid` — client-generated UUID for idempotent retries
- `correlation_id` — groups events from the same session/thread of work
- `causation_event_id` — the prior event that caused this one (optional)
- `type` — see §1.6
- `actor_id` — who did it
- `payload` — typed JSON, validated by the commands layer before append
- `ts` — server timestamp on append

Not in slice 1: `signature`, `schema_version`, `branch_id`. (→ slice 2 if/when needed.)

### 1.6 Event types (7)

- `decision.proposed` — creates a Decision; payload includes title, rationale, `topic_keys[]`, and refs to existing Options/Hypotheses/Evidence
- `decision.accepted` — adds `ACCEPTED_BY` edge
- `decision.rejected` — adds `REJECTED_BY` edge
- `decision.superseded` — adds `SUPERSEDES` edge from new to old; both must exist
- `evidence.recorded` — creates an Evidence node; can be done standalone or implicitly via a decision
- `hypothesis.recorded` — creates a Hypothesis node with `status=open`
- `relation.added` — generic edge for `ASSUMES`, `SUPPORTS`, `REFUTES`, `HAS_OPTION`, `CHOSE`. Payload specifies the relation type and endpoints.

### 1.7 Commands module (the invariant layer)

The only place rules live. CLI, future HTTP, future MCP all go through this.

```
propose_decision(actor, title, rationale, topic_keys, option_ids, hypothesis_ids, evidence_ids) → Decision
accept_decision(decision_id, actor)
reject_decision(decision_id, actor)
supersede_decision(old_id, new_id, actor)
attach_evidence(decision_id, evidence_id, actor)
record_evidence(actor, content) → Evidence
record_hypothesis(actor, statement) → Hypothesis
record_option(actor, label, description) → Option
relate_evidence_to_hypothesis(evidence_id, hypothesis_id, kind: supports|refutes, actor)
```

Invariants enforced here (not in the ledger trait, not in callers):

- All referenced ids must exist before being linked.
- `supersede` requires both decisions to exist; the old one need not already be accepted.
- `accept`/`reject` are *additive*; the same actor doing both is rejected at write time; different actors disagreeing is allowed (→ `contested`).
- Topic keys are normalized (lowercase, slugified) before persistence.

### 1.8 Queries (3 ops, layer 2)

Pure reads. No ranking, no LLM, no inference beyond status derivation.

- `get_decision(id)` → the Decision node + immediate neighbors (options, chosen option, evidence, hypotheses) + derived status
- `get_relevant_decisions(topic, status?)` → all decisions whose `topic_keys` contains the given key; optionally filter by derived status
- `get_supersession_chain(decision_id)` → walk `SUPERSEDES` in both directions; return ordered list with a cycle-guard

Every response carries: `result_count`, `truncated` (always false in slice 1 — no pagination yet), `latency_ms`. Hard ceiling at 1000 nodes per response defensively.

### 1.9 CLI

Three subcommands:

```
hivemind emit <command> [--actor <id>] [args]    # wraps the commands module
hivemind query <op> [args]                        # wraps queries; JSON output
hivemind dump --format dot > graph.dot            # whole projection as Graphviz DOT
```

No `init`, no `rebuild`, no `replay`, no `log`, no `snapshot` in slice 1. (→ slice 2 if useful.)

### 1.10 Tests

- **Unit tests** on the commands module — invariant enforcement.
- **Integration tests** — record-decision → query loop returns expected state.
- **Property tests** — random sequences of valid commands keep the system well-formed; queries never panic.
- **Replay smoke test** (not a release gate) — wipe Kuzu, re-project from SQLite ledger, confirm the same three queries produce identical results. Used to debug projector bugs; failing it is a yellow flag, not red.

### 1.11 Success criteria

Slice 1 ships when:

1. A coding agent can call `hivemind emit decision.proposed …` then `hivemind query get_relevant_decisions topic=foo status=accepted` and see only live, on-topic decisions with their rationale and chosen option.
2. Supersession works end-to-end: superseding decision B over A makes A no longer appear in `accepted` filters, and `get_supersession_chain` returns both.
3. A Hypothesis can be recorded, two Decisions can `ASSUMES` it, and a later `Evidence` can `REFUTES` it — making the Hypothesis's derived status flip to `refuted` and surfacing in the affected Decisions' contexts.
4. The replay smoke test passes locally (not in CI as a gate).

---

## 2. Slice 2 — everything not yet decided

Listed by theme. None of these are designed; they are the open frontier.

### 2.1 The agentic layer (layer 3)

Where all the smart features live. Strictly separate from layers 1 and 2.

- **Compactification.** Goal per spec: keep the main decisions and their why; safely drop details that aren't needed anymore; don't duplicate state that lives elsewhere (e.g., in code). Closer to "semantic forgetting" than to "cluster by tag." **Open question:** rule-based (declarative pattern matching) vs. threshold/mathematical (scores, decay functions, decay-by-age, info-density) vs. hybrid. Will need a configurable substrate either way.
- **Similarity / "find related decisions as seeds."** Look up past decisions adjacent to a new question. Embedding-based likely. Out-of-process so the rest of the system has no ML dependency.
- **Ranking / "what should I read first?"** Given a query result, return an order. Open whether this lives next to queries or is a separate consumer.
- **Suggestion / "what did similar groups decide?"** Trace social/actor graph; surface analogous decisions made elsewhere.

### 2.2 Engine swap (Kuzu → Neo4j)

- Implement Neo4j `GraphView`.
- Run identical query corpus against both; verify result parity.
- Cut over when slice-2 demands a server (multi-agent, multi-host, concurrent writers).
- The DSL → Cypher layer is the only engine-specific code.

### 2.3 Server + API

- HTTP/JSON service wrapping the commands and queries modules.
- MCP wrapper as a thin layer on top of HTTP (or directly on the lib via stdio).
- Auth and rate-limiting policy: open.

### 2.4 Richer relations

- `DEPENDS_ON` (Decision → Decision) — once we have a real use case beyond supersession.
- `CONTRADICTS` (Decision → Decision) — once an agent needs to surface conflicts.
- `EXPANDS_TO` / `CONSTRAINS` — only if a query is genuinely awkward without them.

### 2.5 Multi-writer / Postgres

- Postgres `EventLedger` impl behind the same trait.
- Trigger to migrate: ≥ 2 concurrent writer agents, or sustained > 50 events/sec, or cross-host queries.
- Optimistic concurrency: how do we resolve two agents superseding the same decision concurrently? Open. Likely: both events recorded, the second flagged `concurrent_with: <other_event_id>`, resolution itself a future event.

### 2.6 Signing / provenance

- Optional Ed25519 signatures on events.
- `Actor.pubkey` registered via an `actor.registered` event.
- Query responses flag `verified: true | false | unsigned`.
- Promotion to required-signing is itself an event.

### 2.7 Snapshot / archival

- Periodic snapshots of the Kuzu projection.
- Cold-storage policy for old events (still queryable via lazy loader).

### 2.8 Beads adapter

- Each Bead state transition optionally emits a HiveMind event.
- Reverse: an accepted Decision can update a Bead.
- Slice 2 problem; the graph engine has no Beads dependency.

### 2.9 Hypothesis lifecycle deepening

- Bayesian credence vs. discrete `open | supported | refuted`.
- Thresholds for flipping to `supported` / `refuted` (e.g., k pieces of corroborating evidence).
- Whether `supports`/`refutes` should carry strength values.

---

## 3. Tradeoffs (slice 1 only)

| Choice | Why now | What we pay |
|---|---|---|
| Event ledger + projection | Auditability, rebuildability, three-layer cleanliness | Two stores to keep in sync |
| SQLite + Kuzu embedded | Zero ops; ship fast; prove the model | Will swap to server engine at slice 2 |
| Trait-first Rust crate | Future engines slot in without rewrite | Slightly more upfront design |
| Commands module for invariants | One place for rules; portable across callers | Adds a layer between caller and ledger |
| Status derived from graph edges | Topology *is* the truth; no denormalization; debugging by inspection | Queries must compute status — fine at slice-1 scale |
| Topic as string, not node | Smallest correct model; topic-as-node only earns its keep with compaction | Some normalization logic lives in commands layer |
| No compaction in slice 1 | Compaction is layer 3; not designed yet | Slice 1 graph can grow unbounded — acceptable for prototype |
| HTTP/MCP spec'd but not built | Future-proofs the lib API shape without paying the cost | One round of integration work later |

---

## 4. Risks (slice 1 only)

Only what matters for slice 1. Slice-2 unknowns are listed as open questions in §2, not as risks.

1. **Kuzu-isms creep in.** Kuzu's Cypher dialect has quirks. Mitigation: keep Cypher conservative; the engine swap to Neo4j stays cheap.
2. **Status-from-edges gets slow.** Fine at prototype scale (< 10k decisions). Slice 2 may need a materialized status or an index. Acceptable cost for slice 1 cleanliness.
3. **Single-writer SQLite ceiling.** Fine; only the dev or one CLI process at a time hits it. Slice 2 problem.
4. **Commands module skipped by callers.** If a future caller bypasses commands and writes events directly, invariants break. Mitigation: ledger trait method is `append(event)` only at the bottom; callers in slice 1 (CLI) go through commands by construction. Document the contract.
5. **Replay smoke test rots.** Without being a release gate, it can drift broken. Mitigation: run it on every CI build but only as a warning, not failure. Reconsider as a gate when we have a real projector to break.
6. **Hypothesis status changes invalidate prior decisions silently.** When a Hypothesis flips to `refuted`, decisions that `ASSUMES` it are now resting on shaky ground. Slice-1 mitigation: `get_decision` surfaces hypothesis statuses in the response so the caller sees it. Slice-2 problem: should we actively surface this as a "stale assumptions" view? (→ layer 3.)

---

## 5. Beads (slice 1 only — 7 items)

All P0. Slice 2 beads are deliberately omitted until slice 2 is designed.

**B1 — Event ledger (SQLite) behind `EventLedger` trait**
- AC: append + read of 7 v0 event types; monotonic `event_id`; idempotent on duplicate `event_uuid`; full replay API; in-memory trait impl for tests.

**B2 — Kuzu projector + `GraphView` trait**
- AC: consumes events; builds 5 node types + 10 relation types; idempotent; full rebuild from ledger.

**B3 — Commands module (the rules layer)**
- AC: 9 commands (§1.7) implemented; invariant unit tests cover every rule; topic normalization works; CLI/HTTP/MCP would all call this — slice 1 only the CLI does.

**B4 — Queries module (`get_decision`, `get_relevant_decisions`, `get_supersession_chain`)**
- AC: derived status correct for all 5 cases including `contested`; supersession chain walks both directions; cycle-guard; integration tests against a fixture.

**B5 — CLI `hivemind emit | query | dump`**
- AC: all three subcommands work end-to-end; DOT output renders with graphviz; JSON output for queries.

**B6 — Seed dataset + integration tests**
- AC: ≥ 30 fabricated decisions covering: ≥1 superseded, ≥1 contested, ≥1 hypothesis flipped to `refuted` invalidating two decisions, ≥3 topics, ≥1 multi-option decision; golden JSON snapshots for each of the 3 queries.

**B7 — Replay smoke test**
- AC: `wipe Kuzu → reproject from ledger → re-run all 3 queries → byte-identical JSON`. Runs in CI; warnings on diff, not failure.

---

## 6. Unresolved decisions (all → slice 2)

- **U1.** Compaction approach: rule-based / threshold-mathematical / declarative / hybrid. Per spec the substrate must allow several; design open.
- **U2.** Whether layer 3 (agentic analysis) runs in-process as plugins or out-of-process as services. Likely the latter.
- **U3.** Whether `ASSUMES`/`SUPPORTS`/`REFUTES` strength should be a scalar (Bayesian credence) or remain discrete.
- **U4.** Whether `contested` decisions should be allowed to be superseded, or whether contestation must be resolved first.
- **U5.** Auth model — slice-2 problem.
- **U6.** Whether `correlation_id` is per-session, per-thread, or per-issue. Slice 1 treats it as opaque.

---

## 7. Implementation guardrails

The three rules we don't break:

1. **No mixing layers.** A function does not write *and* query *and* analyze. Compaction is layer 3 even when it's tempting to call it from a query. Similarity is layer 3. Anything LLM-touched is layer 3.
2. **No overinvestment.** If a feature isn't required to ship slice 1, it isn't in slice 1. Forward-compat hooks (`signature`, `branch_id`, `schema_version`) are absent in slice 1. They go in when slice 2 needs them.
3. **No Kuzu-isms.** Stay in portable Cypher. The slice-2 swap to Neo4j must be a small piece of adapter code, not a rewrite.
