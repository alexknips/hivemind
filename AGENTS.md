# AGENTS.md — HiveMind High Bar

This document defines the **standard of excellence** that any agent (human or AI) contributing to HiveMind is expected to meet. It is not the implementation plan (see `PLAN.md`). It is the bar.

HiveMind is a system for **corporate, multi-human, multi-agent decision-making memory**. It must be designed and built to a standard that justifies organizations and autonomous agents trusting it as the source of truth for *what was decided, why, by whom, and what it depends on*.

---

## 1. What HiveMind must do (the bar, not the feature list)

1. **Capture organizational decision memory** — not personal notes, not chat history, not task tracking. The unit of value is a recoverable, defensible decision with full provenance.
2. **Treat humans and agents as first-class peers.** Every action is taken by an `Actor`. Agents propose, accept, reject, supersede, and contest decisions on the same footing as humans. No second-class API.
3. **Survive disagreement.** Real organizations contain dissent. `contested` is a real status, not an error. Two actors disagreeing must be preserved, queryable, and resolvable — never silently dropped, overwritten, or auto-resolved.
4. **Preserve the why.** A decision without its rationale, considered options, and underlying evidence is worthless six months later. Capturing the *reasoning context* is non-negotiable.
5. **Make the past auditable.** Every node and edge in the graph traces to a specific event in the immutable ledger. Anyone — human or agent, internal or external — must be able to answer "how did we arrive at this?" deterministically.
6. **Make staleness visible.** When a hypothesis is refuted or a decision is superseded, every dependent decision must surface that fact in queries. Silent staleness is the worst failure mode.
7. **Forget responsibly.** Compactification removes noise, never signal. Main decisions and their why are preserved; only details that are safely redundant — or already captured elsewhere — are dropped. Forgetting is itself a tracked, reversible event.

---

## 2. Non-goals (do not let scope creep here)

HiveMind is **not**:

- A project management tool, task tracker, or ticketing system.
- A chat archive or conversational memory store.
- An agent's private working memory.
- A general-purpose knowledge graph.
- A documentation system.
- A code-review tool.

If a proposed feature pulls toward any of the above, it is out of scope. The discipline is to stay a **decision graph**, not let it drift into "everything graph."

---

## 3. Architectural standards

The three-layer separation is **load-bearing** and not optional:

1. **Write/ingest layer** — validates invariants, appends events. Dumb-but-correct.
2. **Query/read layer** — pure reads. No LLMs, no ranking, no inference beyond explicit status derivation.
3. **Agentic suggestion/analysis layer** — all intelligence (compactification, similarity, ranking, suggestions) lives here and only here. It is built later and is **swappable** — the rest of the system must remain functional and correct without it.

A function that crosses layers is a bug. A query that calls an LLM is a bug. A write path that "helpfully" runs similarity to deduplicate is a bug. Smart features stay in layer 3 where they can be A/B-tested, replaced, or removed without touching ingest or queries.

---

## 4. Provenance and trust standards

- Every event has an `actor_id`. Anonymous writes are not allowed.
- Every node and edge in the graph carries `event_origin` — the ledger offset that created it.
- Optional cryptographic signing (Ed25519) is a slice-2 commitment, not a vague aspiration. When a multi-organization deployment lands, signing becomes mandatory.
- `contested` decisions are surfaced as a top-level status — never hidden in flags.
- A `Hypothesis` flipping to `refuted` propagates to every `Decision` that `ASSUMES` it. This propagation is visible in queries by default.

---

## 5. Scaling standards

HiveMind must scale past the single-developer prototype to:

- **Multi-actor concurrent writes** without losing causality. Optimistic concurrency on supersessions; concurrent supersessions of the same target both survive and are flagged as such.
- **Multi-host queries** with bounded response sizes, pagination, and cycle protection. No query ever returns the whole graph.
- **Cross-organization federation** eventually. The ledger format and identity model must not preclude federation, even if it's not built day one.
- **Tens of thousands of decisions per organization** without query latency degrading beyond agent usability (p95 < 250ms target at slice-2 scale).

The prototype (slice 1) is intentionally smaller, but no design choice in slice 1 may foreclose any of the above. If a choice is convenient now but blocks scaling later, it's wrong.

---

## 6. Honesty standards

These apply to the system itself and to its contributors:

- **No silent truncation.** Queries that hit limits return `truncated: true` and a way to continue. Never a partial result that looks complete.
- **No silent disagreement collapse.** `contested` is a status, not a fallback.
- **No silent staleness.** Refuted hypotheses and superseded decisions are visible in queries, not hidden.
- **No invented confidence.** If layer 3 ranks or scores, the score's basis must be traceable. "Because the model said so" is not acceptable provenance.
- **No undocumented invariants.** Every rule that the commands layer enforces is written down and tested. Tribal knowledge is a bug.

---

## 7. Onboarding standard

A coding agent should be able to start using HiveMind in **under five minutes**: install the CLI, point it at a directory, call `hivemind emit decision.proposed …`, and immediately query it back. If it takes longer than that, the API is wrong.

A new human contributor should be able to read this file, `PLAN.md`, and the relevant slice's beads, and understand the entire architecture in **under an hour**. If they can't, the documentation is wrong.

---

## 8. The contributor's commitment

Whether you're a human engineer or an AI agent contributing to HiveMind, you commit to:

- Respect the three-layer boundary.
- Cut speculative features rather than add them.
- Make every change traceable to a decision (eventually recorded *in HiveMind itself*).
- Surface disagreement and staleness, never suppress them.
- Build the smallest correct thing that meets the bar, then extend.

This is the standard. Anything less and we are not building organizational memory — we are building yet another note-taking app.
