---
name: classify-queue
description: Drain the HiveMind classification work queue using the agent's subscription seat (Worker A)
argument-hint: '[--limit N]'
allowed-tools: Bash(hivemind classify-queue list:*), Bash(hivemind classify-queue submit:*)
---

# Classify pending HiveMind batches (Worker A — subscription seat)

Drain the pending classification queue up to `$ARGUMENTS` batches (default 20).

## Workflow

**Step 1: List pending batches**

```bash
hivemind classify-queue list --json --limit ${HIVEMIND_CQ_LIMIT:-20}
```

If the list is empty, report "No pending batches" and stop.

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

**Step 3: Submit each classification**

For each batch, build the captures array and submit:

```bash
hivemind classify-queue submit \
  --batch-id <batch_id> \
  --captures '<json array of CaptureItem objects>'
```

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
