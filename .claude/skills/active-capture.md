---
name: active-capture
description: Nudge HiveMind capture when durable decision memory is forming: comparing alternatives, choosing or rejecting a direction, recording real evidence, stating a hypothesis, or identifying a blocker. Do not activate for synthetic test data, routing chatter, task bookkeeping, or ordinary progress updates.
---

# Active HiveMind Capture

Use this skill during the session when a capture-worthy moment is happening,
not after the turn is over. This is a nudge path only: decide whether the
moment is real, then ask the main agent to invoke `/capture`; do not write
directly to the ledger from this skill. When the `hivemind` MCP server is
registered over HTTP against a remote cell instead (no local `--hivemind-dir`
the CLI can reach), ask the main agent to call the equivalent
`mcp__hivemind__capture_*` tool directly instead of `/capture` — see
`hivemind-capture`'s "MCP-over-HTTP Capture" section.

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

A `decision` call carries its structured fields and the answer to "what does
this rest on?" (next section). The other kinds take the statement alone.

Examples:

```text
/capture "Use SQLite WAL for local concurrent writes" --kind decision --title "Use SQLite WAL for local concurrent writes" --rationale "It preserves single-process setup while allowing read concurrency" --topic-keys storage --options sqlite-wal,rollback-journal --chose sqlite-wal --rests-on-evidence "The WAL multiprocess test passed with two concurrent readers and one writer" --evidence-source "cargo test wal_multiprocess"
/capture The WAL multiprocess test passed against two concurrent readers and one writer. --kind evidence
/capture Shared backend adoption assumes teams will accept service-managed identity instead of per-repo local actors. --kind hypothesis
/capture Release packaging is blocked on choosing GitHub Actions retries versus local DSR fallback. --kind blocker
```

## What a Decision Capture Must Carry

A decision capture must say what it rests on; one that names nothing is refused
and writes nothing. Know the answer before you nudge. It is one or more of:

- a decision we already made: `--rests-on-decision "<description>"`, ideally
  one consulted before acting (`hivemind-context` `situational` or `recall`);
- something observed and where: `--rests-on-evidence "<what>"
  --evidence-source "<URL, file@commit, test run, measurement>"`;
- something we assume: `--rests-on-assumption "<statement>"`;
- nothing yet, a declared bet: `--bet ["<statement>"]`.

The decider's own words are not a grounding. A Slack message or chat reply goes
in `--quote` with `--question`, never in `--rests-on-evidence`. Pass
`--confidence` only when the decider's own words state it. After a refusal, add
the grounding and re-run; never drop the capture. The `hivemind-capture` skill
has the full text.

## Guardrails

Do NOT call this for synthetic test data or routing chatter. Do not invoke
`/capture` for:

- Synthetic test data, fixture-only assertions, seed prompts, demos, or examples
  that do not represent a real organizational decision.
- Routing chatter such as `gc sling`, `br update`, merge-ready handoffs,
  session nudges, agent assignment, or queue plumbing.
- Routine implementation progress, todos, code formatting, branch names, status
  reports, or private scratch reasoning.
- Reworded duplicates of a capture already made in the same context — this
  includes a rollup that restates choices already captured individually, and
  individual captures of choices already captured as one bundled decision.
  Decide once, per exchange, whether it is one decision or several, and never
  capture it both ways.

Before nudging a capture, and especially at the start of a session, after a
restart, or when resuming a conversation someone else may have already
captured pieces of, check `hivemind-context`'s `recall` for the topic — your
own transcript does not carry a prior session's writes. A hit that already
covers this ground means: don't nudge another capture.

If the signal is ambiguous, keep working and wait for a clearer decision,
evidence, hypothesis, or blocker. Never infer importance with search,
similarity, ranking, or model confidence; only capture explicit context in
front of you.
