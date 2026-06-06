---
name: active-capture
description: Nudge HiveMind capture when durable decision memory is forming: comparing alternatives, choosing or rejecting a direction, recording real evidence, stating a hypothesis, or identifying a blocker. Do not activate for synthetic test data, routing chatter, task bookkeeping, or ordinary progress updates.
---

# Active HiveMind Capture

Use this skill during the session when a capture-worthy moment is happening,
not after the turn is over. This is a nudge path only: decide whether the
moment is real, then ask the main agent to invoke `/capture`; do not write
directly to the ledger from this skill.

## Capture Moments

Consider capture when the current context includes one of these durable
organizational memory items:

- `decision`: an actor chooses, rejects, accepts, supersedes, or contests a
  direction, and later work may depend on that choice.
- `evidence`: a real test result, production observation, user-provided fact,
  measurement, or external artifact supports or refutes a decision or
  hypothesis.
- `hypothesis`: an explicit assumption, prediction, or risk model is being
  relied on before it has been verified.
- `blocker`: progress is waiting on a decision, owner, approval, evidence, or
  unresolved disagreement.

## Call Form

When the moment is real and specific, invoke:

```text
/capture <text> [--kind decision|evidence|hypothesis|blocker]
```

Use the smallest durable statement that preserves the what, why, actor, and
dependency context. Prefer `--kind decision` for selected or rejected options,
`--kind evidence` for observed facts, `--kind hypothesis` for assumptions that
may later be refuted, and `--kind blocker` for unresolved decision
dependencies.

Examples:

```text
/capture Use SQLite WAL for local concurrent writes because it preserves single-process setup while allowing read concurrency. --kind decision
/capture The WAL multiprocess test passed against two concurrent readers and one writer. --kind evidence
/capture Shared backend adoption assumes teams will accept service-managed identity instead of per-repo local actors. --kind hypothesis
/capture Release packaging is blocked on choosing GitHub Actions retries versus local DSR fallback. --kind blocker
```

## Guardrails

Do NOT call this for synthetic test data or routing chatter. Do not invoke
`/capture` for:

- Synthetic test data, fixture-only assertions, seed prompts, demos, or examples
  that do not represent a real organizational decision.
- Routing chatter such as `gc sling`, `br update`, merge-ready handoffs,
  session nudges, agent assignment, or queue plumbing.
- Routine implementation progress, todos, code formatting, branch names, status
  reports, or private scratch reasoning.
- Reworded duplicates of a capture already made in the same context.

If the signal is ambiguous, keep working and wait for a clearer decision,
evidence, hypothesis, or blocker. Never infer importance with search,
similarity, ranking, or model confidence; only capture explicit context in
front of you.
