---
name: batch-capture
description: Classify recent conversation context and batch-capture durable decisions into HiveMind (keyless — uses your Claude subscription, no server-side API key required)
argument-hint: '[--limit N] [--min-confidence 0.7]'
allowed-tools: Agent(model:haiku), Bash(hivemind:*)
---

# Batch-capture HiveMind decisions from recent context (keyless)

Extract durable decision memory from the recent conversation and write it to the
HiveMind ledger. Classification runs as a Haiku subagent inside your own session —
no `ANTHROPIC_API_KEY` is required on the server side.

## Step 1 — Classify

Spawn a Haiku subagent with the following prompt, substituting the recent
conversation text for `<BATCH_CONTENT>`:

```
You are the HiveMind capture classifier.

HiveMind stores organizational decision memory: durable decisions, evidence,
hypotheses, blockers, decision requests, and notifications with provenance.
It does not store chat history, task tracking, private scratch notes, or
raw tool logs.

Read the batch of recent agent activity. Return ONLY a JSON array of capture
objects. Most batches should return [].

Capture a decision only when the text shows a chosen path among plausible
alternatives and gives or implies a reason.

Each capture object must have exactly these fields:
{
  "kind": "decision" | "evidence" | "hypothesis" | "blocker" | "decision-request" | "notification",
  "title": string,
  "rationale": string,
  "topic_keys": [string, ...],
  "evidence_ids": [],
  "options": [string, ...] | null,
  "chosen_option": string | null,
  "extraction_confidence": number (0.0-1.0),
  "expressed_confidence": "low" | "medium" | "high" | null,
  "supersedes_id": null,
  "assumes_ids": [],
  "supports_ids": [],
  "refutes_ids": [],
  "actor_id": null,
  "accepted_by": null,
  "rejected_by": null,
  "blocked_actor_id": null,
  "decision_id": null
}

Return only the JSON array, no other text.

---BATCH---
<BATCH_CONTENT>
```

## Step 2 — Submit

Write the JSON array returned by the subagent to a temporary file, then submit:

```bash
HIVEMIND_AGENT_SESSION="${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}"
printf '%s\n' '<SUBAGENT_JSON_OUTPUT>' > /tmp/hivemind-batch-captures.json
hivemind --hivemind-dir "$HIVEMIND_DIR" emit ingest.batch_classified \
  --captures /tmp/hivemind-batch-captures.json \
  --agent-tool claude \
  --agent-session "$HIVEMIND_AGENT_SESSION" \
  --classifier-model "claude-haiku-4-5-20251001"
```

If the subagent returned `[]` (no captures), skip submission and report
"No durable decisions found in recent context."

## Step 3 — Report

Report the number of items captured and their titles. If `$ARGUMENTS` included
`--min-confidence <N>`, filter out items below that threshold before submitting.

---

Use `/hivemind-capture:capture` for single, immediately-known decisions.
Use `/hivemind-capture:batch-capture` for retrospective extraction over
accumulated context where several decisions may have formed.
