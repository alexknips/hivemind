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

---

## 9. Documentation boundary

Work tracking belongs in Beads, not ordinary repo files.

- Use Beads for task breakdowns, dependencies, milestones, follow-ups, routing, status, and implementation queues.
- Keep ordinary docs and source comments focused on stable product behavior, architecture, commands, contributor rules, and validation guidance.
- `PLAN.md` is the sole planning exception. Do not add planning sections to other docs as a substitute for beads.
- When a planning detail must be preserved, update or create the relevant bead or bead note instead of adding it to docs.

<!-- bv-agent-instructions-v2 -->

---

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`) for issue tracking and [beads_viewer](https://github.com/Dicklesworthstone/beads_viewer) (`bv`) for graph-aware triage. Issues are stored in `.beads/` and tracked in git.

### Using bv as an AI sidecar

bv is a graph-aware triage engine for Beads projects (.beads/beads.jsonl). Instead of parsing JSONL or hallucinating graph traversal, use robot flags for deterministic, dependency-aware outputs with precomputed metrics (PageRank, betweenness, critical path, cycles, HITS, eigenvector, k-core).

**Scope boundary:** bv handles *what to work on* (triage, priority, planning). `br` handles creating, modifying, and closing beads.

**CRITICAL: Use ONLY --robot-* flags. Bare bv launches an interactive TUI that blocks your session.**

#### The Workflow: Start With Triage

**`bv --robot-triage` is your single entry point.** It returns everything you need in one call:
- `quick_ref`: at-a-glance counts + top 3 picks
- `recommendations`: ranked actionable items with scores, reasons, unblock info
- `quick_wins`: low-effort high-impact items
- `blockers_to_clear`: items that unblock the most downstream work
- `project_health`: status/type/priority distributions, graph metrics
- `commands`: copy-paste shell commands for next steps

```bash
bv --robot-triage        # THE MEGA-COMMAND: start here
bv --robot-next          # Minimal: just the single top pick + claim command

# Token-optimized output (TOON) for lower LLM context usage:
bv --robot-triage --format toon
```

Before claiming, verify current state with `br show <id> --json` or `br ready --json`. `recommendations` can include graph-important blocked or assigned work; only `quick_ref.top_picks` and non-empty `claim_command` fields represent claimable work.

#### Other bv Commands

| Command | Returns |
|---------|---------|
| `--robot-plan` | Parallel execution tracks with unblocks lists |
| `--robot-priority` | Priority misalignment detection with confidence |
| `--robot-insights` | Full metrics: PageRank, betweenness, HITS, eigenvector, critical path, cycles, k-core |
| `--robot-alerts` | Stale issues, blocking cascades, priority mismatches |
| `--robot-suggest` | Hygiene: duplicates, missing deps, label suggestions, cycle breaks |
| `--robot-diff --diff-since <ref>` | Changes since ref: new/closed/modified issues |
| `--robot-graph [--graph-format=json\|dot\|mermaid]` | Dependency graph export |

#### Scoping & Filtering

```bash
bv --robot-plan --label backend              # Scope to label's subgraph
bv --robot-insights --as-of HEAD~30          # Historical point-in-time
bv --recipe actionable --robot-plan          # Pre-filter: ready to work (no blockers)
bv --recipe high-impact --robot-triage       # Pre-filter: top PageRank scores
```

### br Commands for Issue Management

```bash
br ready              # Show issues ready to work (no blockers)
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br create --title="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason="Completed"
br close <id1> <id2>  # Close multiple issues at once
br sync --flush-only  # Export DB to JSONL
```

### Workflow Pattern

1. **Triage**: Run `bv --robot-triage` to find the highest-impact actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Always run `br sync --flush-only` at session end

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers 0-4, not words)
- **Types**: task, bug, feature, epic, chore, docs, question
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads changes to JSONL
git commit -m "..."     # Commit everything
git push                # Push to remote
```

<!-- end-bv-agent-instructions -->
