---
name: classify-queue
description: Drain the HiveMind classification work queue using the agent's subscription seat (Worker A)
argument-hint: '[--limit N]'
allowed-tools: Bash(hivemind classify-queue list:*), Bash(hivemind classify-queue submit:*)
---

# Classify pending HiveMind batches (Worker A — subscription seat)

Drain the pending classification queue up to `$ARGUMENTS` batches (default 20).

Run this once per session end (e.g. from a `Stop` hook), not per batch and not
on an hourly timer — Worker A spends the agent's own subscription-seat model
call to classify, and a shared daily cap (~150 classification calls city-wide;
see `HIVEMIND_CLASSIFY_DAILY_CAP`) protects against runaway spend across every
session in the city. Pass `--session-id` to scope listing to just the calling
session's own batches, so concurrent sessions never redundantly process each
other's queue.

On a server-backed cell, set `HIVEMIND_API_URL` (and `HIVEMIND_API_KEY` if the
server requires auth) so `classify-queue list`/`submit` talk to the server
over HTTP instead of opening a local ledger — no database credential in the
agent's environment (Option B, hivemind-zdsh.2). Leave both unset for the
local `--hivemind-dir` SQLite path.

## Workflow

**Step 1: List pending batches**

```bash
hivemind classify-queue list --json --limit ${HIVEMIND_CQ_LIMIT:-20} \
  ${HIVEMIND_CQ_SESSION_ID:+--session-id "$HIVEMIND_CQ_SESSION_ID"}
```

If the list is empty, report "No pending batches" and stop. If the request
fails with a daily-cap message, report it and stop — the batches stay pending
and will be picked up by a future run once the cap resets (UTC midnight).

**Step 2: Classify each batch**

For each batch returned in step 1, read the `batch_id` and `turn_count`.
Do NOT fetch the raw turns text — it is embedded in the batch record's `batch_text`
field via the list command. Use the classification rules below.

**Classification rules (same as the server-side classifier):**

Capture an item ONLY when the conversation text shows:
- **decision**: a chosen or rejected path among plausible alternatives, with a reason.
  Use `decision-request` when the choice is requested but not yet made.
- **evidence**: a real observation (test result, measurement, verified fact) that
  supports or refutes a decision or hypothesis.
- **hypothesis**: an explicit assumption or prediction being relied on before verification.
- **blocker**: progress is materially stopped (dependency, approval, unavailable service,
  unresolved decision).
- **notification**: a handoff, merge-ready signal, or rejection state another actor
  needs after restart.

**Do NOT capture:** synthetic test data, fixture/demo content, routing chatter (gc/br
plumbing), stack traces, raw command output, status narration, todos, or private scratch
notes. When in doubt, return an empty captures array.

**Step 3: Submit classifications, grouped by session**

Batches from the same `session_id` came from one conversation — submit them
together in a single call so one write covers the whole session's captures
(pass `--batch-id` once per batch, comma-separated):

```bash
hivemind classify-queue submit \
  --batch-id <batch_id_1>,<batch_id_2> \
  --captures '<json array of CaptureItem objects covering all listed batches>'
```

A batch with no session (`session_id` empty in the list output) submits alone.

Each CaptureItem JSON object must include:
- `kind`: "decision" | "evidence" | "hypothesis" | "blocker" | "decision-request" | "notification"
- `title`: concise title (required)
- `rationale`: the why, in the words of the text (required)
- `topic_keys`: array of topic strings (required, may be empty array)
- `evidence_ids`: array of existing evidence IDs referenced (required, usually empty array)
- `options`: array of option strings or null
- `chosen_option`: string or null
- `extraction_confidence`: float in [0,1] — your confidence this item is genuinely durable

Optional fields (omit rather than null unless needed):
- `expressed_confidence`: "low" | "medium" | "high" — only when stated in the text
- `supersedes_id`: ID of a decision this supersedes, only if named in the text
- `actor_id`: the person who proposed/decided, only if named in the text
- `accepted_by`: the actor who accepted, only if named in the text
- `rejected_by`: the actor who rejected, only if named in the text

**Process all batches in sequence.** Report a summary: batches processed, total captures
written, any errors.

## Bounds

- Worker A is subscription-seat bound: each batch consumes one model invocation.
- Large backlogs drain across multiple `/classify-queue` runs — the queue is persistent.
- Queue depth is always visible via `hivemind classify-queue list --json | jq length`.
- The server-side classifier (Worker B) and Worker A drain the same queue; both can run;
  last-writer-wins per batch_id is idempotent.
