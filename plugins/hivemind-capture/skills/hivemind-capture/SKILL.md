---
name: hivemind-capture
description: Capture durable organizational decisions from Claude Code or Codex into HiveMind with full provenance. Use when an agent session chooses between options, records architecture/integration/policy rationale, accepts or rejects a decision, supersedes prior direction, or needs to verify agent-captured decisions. Do not use for chat logs, task tracking, private scratch memory, or speculative notes.
---

# HiveMind Capture

## Capture Boundary

Capture decision memory, not conversation history. Record a decision when the
organization would later need to answer what was decided, why, by whom, which
options were considered, and what the decision depends on.

Capture these:

- A selected architecture, integration, storage, security, or process direction.
- A rejected option when the rejection matters later.
- A supersession or acceptance of an existing decision.
- Rationale that depends on durable evidence or hypotheses already in HiveMind.

Do not capture these:

- Ordinary progress updates, todos, or implementation queues.
- Raw chat transcripts or brainstorming before a conclusion exists.
- Private agent scratch memory.
- Inferred confidence or similarity judgments.

## Automatic Capture Triggers

When this skill is installed, do not wait for the user to ask for capture. Use
it during the session whenever a durable decision moment occurs:

- You make a non-trivial architecture, integration, storage, security, or
  process choice that later work will depend on.
- You choose or recommend one option from alternatives the user presented.
- You replace an earlier direction you chose in the same session.

Capture immediately after the decision is made and before moving on to dependent
work. Keep the trigger deterministic: capture the explicit choice and rationale
you just made; do not run search, similarity, ranking, or model-based inference
to decide whether the decision is important.

For supersession, first capture the replacement decision. If the previous
decision id is known, emit `decision.superseded` with the same actor identity.
If it is not known, do not invent an id; make the supersession relationship clear
in the new decision rationale.

## Capture Workflow

Use the HiveMind CLI as the write transport. Skills improve recall, but the
ledger write must stay explicit and deterministic.

1. Set the target ledger. Use local storage by default, or point at a shared
   directory/service-mounted ledger when the team has one:

   ```bash
   export HIVEMIND_DIR="${HIVEMIND_DIR:-./hivemind}"
   ```

2. Use session context as the actor identity. The plugin helper does this for
   Claude Code and Codex, so agents should not invent `actor_id`, `source`, or
   `source_ref` values:

   ```bash
   plugins/hivemind-capture/scripts/capture.sh \
     "Prefer direct CLI capture before MCP" \
     --kind decision \
     --title "Prefer direct CLI capture before MCP" \
     --rationale "The write path is explicit, testable, and does not depend on hooks or MCP setup" \
     --topic-keys agents,capture \
     --options direct-cli,mcp,hook \
     --chose direct-cli
   ```

   The helper records `source=agent`, derives `actor_id=agent:<tool>:<session>`,
   and sets `source_ref` to the same actor id unless explicitly overridden.
   Codex records as `agent:codex:<session>` using `CODEX_THREAD_ID`,
   `CODEX_SESSION_ID`, or `CODEX_TASK_ID` when present, then Gas City session
   variables. Claude Code records as `agent:claude:<session>` using
   `CLAUDE_SESSION_ID` or `CLAUDE_CODE_SESSION_ID`, then Gas City session
   variables.

   The CLI derives `actor_id=agent:codex:<session>` for Codex and
   `actor_id=agent:claude:<session>` for Claude unless `--actor-id` is
   explicitly provided. Use `--agent-tool codex --agent-session <session>` only
   when the helper is unavailable or when overriding the environment-derived
   defaults.

3. Capture a new proposed decision directly when the helper is unavailable:

   ```bash
   HIVEMIND_AGENT_SESSION="${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.capture \
     --agent-tool codex \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --title "Prefer direct CLI capture before MCP" \
     --rationale "The write path is explicit, testable, and does not depend on hooks or MCP setup" \
     --topic-keys agents,capture \
     --options direct-cli,mcp,hook \
     --chose direct-cli
   ```

   If the `hivemind` binary is not on `PATH`, run the same command from a
   HiveMind source checkout with `cargo run --` before the flags.

   From the Claude Code plugin, prefer the installed slash command:

   ```text
   /hivemind-capture:capture "Prefer direct CLI capture before MCP" --kind decision --title "Prefer direct CLI capture before MCP" --rationale "The write path is explicit, testable, and does not depend on hooks or MCP setup" --topic-keys agents,capture --options direct-cli,mcp,hook --chose direct-cli
   ```

4. Attach existing evidence or hypotheses only when their ids are already known:

   ```bash
   HIVEMIND_AGENT_SESSION="${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.capture \
     --agent-tool codex \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --title "Use shared ledger storage for the integration demo" \
     --rationale "Multiple agents must query the same provenance without local file copying" \
     --topic-keys agents,capture,storage \
     --options local-ledger,shared-ledger \
     --chose shared-ledger \
     --evidence evidence-001 \
     --hypotheses hypothesis-001
   ```

5. For acceptance, rejection, or supersession, use the lower-level event verbs
   with the same actor id:

   ```bash
   HIVEMIND_AGENT_SESSION="${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}"
   hivemind --actor "agent:codex:$HIVEMIND_AGENT_SESSION" \
     --hivemind-dir "$HIVEMIND_DIR" emit decision.accepted \
     --decision-id decision-001
   ```

   ```bash
   HIVEMIND_AGENT_SESSION="${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}"
   hivemind --actor "agent:codex:$HIVEMIND_AGENT_SESSION" \
     --hivemind-dir "$HIVEMIND_DIR" emit decision.superseded \
     --old decision-001 \
     --new decision-002
   ```

6. Verify the write through the read path:

   ```bash
   HIVEMIND_AGENT_SESSION="${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" query search_decisions \
     --actor-id "agent:codex:$HIVEMIND_AGENT_SESSION" \
     --source agent \
     --limit 10
   ```

## Batch Capture via Haiku Subagent (Keyless)

When you want to extract multiple decisions from a conversation batch without
requiring a server-side `ANTHROPIC_API_KEY`, spawn a Haiku subagent inside
your own session. The subagent runs the classifier prompt against recent
activity and writes the results directly to the ledger using
`emit ingest.batch_classified`. No HiveMind-held key is required — it rides
the user's own Claude subscription.

Use this path when:
- You have a batch of conversation turns to classify at once.
- The server may not have `ANTHROPIC_API_KEY` configured (self-hosted, zero-key
  deployments).
- You want extraction to happen immediately rather than waiting for the server's
  background poll.

Explicit `emit decision.capture` remains the preferred path for single,
deterministic decisions you make in the moment. Use the batch path for
retrospective extraction over accumulated context.

### Batch Capture Workflow

1. **Collect the batch text** — the recent conversation activity to classify.
   Write it to a temporary file:

   ```bash
   cat > /tmp/hivemind-batch.txt <<'BATCH'
   [assistant] Decided to use SQLite for local ledger storage — lighter than
   Postgres for single-user deployments, sufficient for prototype scale.
   Options considered: postgres, sqlite, dolt. Chose sqlite.
   BATCH
   ```

2. **Spawn a Haiku subagent** (keeps classification off your main context):

   Use the `Agent` tool with `model: "haiku"` and the following prompt,
   substituting `<BATCH_CONTENT>` with the text from step 1:

   ```
   You are the HiveMind capture classifier.

   HiveMind stores organizational decision memory: durable decisions, evidence,
   hypotheses, blockers, decision requests, and notifications with provenance.
   It does not store chat history, task tracking, private scratch notes, or
   raw tool logs.

   Read the batch of recent agent activity. Return ONLY a JSON array of
   capture objects. Most batches should return [].

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

   The subagent returns a JSON array like:
   ```json
   [
     {
       "kind": "decision",
       "title": "Use SQLite for local ledger",
       "rationale": "Lighter than Postgres for single-user deployments; sufficient for prototype scale",
       "topic_keys": ["storage", "architecture"],
       "evidence_ids": [],
       "options": ["postgres", "sqlite", "dolt"],
       "chosen_option": "sqlite",
       "extraction_confidence": 0.92,
       "expressed_confidence": null,
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
   ]
   ```

3. **Write the captures to a file** and submit to the ledger:

   ```bash
   # Write the JSON array the subagent returned
   cat > /tmp/hivemind-captures.json <<'EOF'
   [{ ... subagent output ... }]
   EOF

   HIVEMIND_AGENT_SESSION="${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit ingest.batch_classified \
     --captures /tmp/hivemind-captures.json \
     --agent-tool claude \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --classifier-model "claude-haiku-4-5-20251001"
   ```

   For Codex sessions:
   ```bash
   HIVEMIND_AGENT_SESSION="${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit ingest.batch_classified \
     --captures /tmp/hivemind-captures.json \
     --agent-tool codex \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --classifier-model "claude-haiku-4-5-20251001"
   ```

   The command returns the `batch_id` for the submitted batch. The server's
   background classifier does not re-process plugin-submitted batches because
   no `ingest.batch_received` event is created — the classifier only processes
   those.

4. **Verify** the captures are visible:

   ```bash
   hivemind --hivemind-dir "$HIVEMIND_DIR" query search_decisions --limit 5
   ```

### Schema Contract

The JSON array must match the `CaptureItem` schema from `src/classifier.rs`.
All fields listed in step 2 are required. Use empty arrays for `evidence_ids`,
`assumes_ids`, `supports_ids`, `refutes_ids`. Use `null` for all optional
string fields unless the input text explicitly names them. Do not invent ids.

## Batch Score via Haiku Subagent (Keyless)

When you want to assess the Quality and Importance of a decision capture
without a server-side `ANTHROPIC_API_KEY`, spawn a Haiku subagent inside your
own session — same idea as batch capture above. The subagent runs the scorer
prompt against one decision capture and writes the result directly to the
ledger using `emit decision.scored`. No HiveMind-held key is required — it
rides the user's own Claude subscription.

Score a decision capture right after submitting it via
`emit ingest.batch_classified` (or `emit decision.capture`), while its
`batch_id` and its position in the captures array are still at hand. This
mirrors the server's own Layer-3 scorer (`src/scorer.rs`), which runs the same
prompt against `ANTHROPIC_API_KEY`-backed batches — the two paths never
double-score the same capture (the background scorer skips node ids that
already carry a `decision.scored` event).

### Scoring Workflow

1. **Collect the decision text** — title, rationale, options considered,
   chosen option, expressed confidence — whichever fields the capture actually
   has. This is the same content submitted in the capture.

2. **Spawn a Haiku subagent** (keeps scoring off your main context) with the
   following prompt, substituting `<DECISION_TEXT>`:

   ```
   You are the HiveMind decision scorer.

   HiveMind stores organizational decision memory. Your job is to score a captured
   decision on two independent axes, assessed EX ANTE — only from what was knowable
   at decision time. Never penalise or reward a decision for its outcomes.

   AXIS 1 — Quality [0.0,1.0]: How well-made was the decision?
   Score each of the 7 dimensions from 0.0 (absent/poor) to 1.0 (excellent):
     framing        — Was the right problem/question framed?
     alternatives   — Were genuine alternatives generated and considered?
     information    — Was relevant information gathered and used?
     reasoning      — Is the inference from information to choice sound?
     values_tradeoffs — Were values and tradeoffs made explicit and weighed?
     bias_exposure  — Exposure to cognitive distortions (anchoring, confirmation,
                      sunk-cost, framing, motivated reasoning). 1.0=low bias.
     calibration    — Does expressed confidence match the evidence? 1.0=well-calibrated.

   AXIS 2 — Importance (unbounded magnitude):
     stakes         — Unbounded positive float, log-scaled. Small decisions: ~1.
                      Department-level: ~10. Company-level: ~100. Industry-level: ~1000.
                      Computed as severity × reach.
     irreversibility — [0.0,1.0]. 0=fully reversible (two-way door), 1=irreversible.
     actionability  — [0.0,1.0]. 0=not actionable (pure observation), 1=fully actionable.

   For each score, give a short (1-2 sentence) explanation grounded in the decision text.
   If a dimension cannot be assessed from the available text, score it 0.5 and explain why.

   Return only JSON matching this schema, no other text:
   {
     "quality_dims": {
       "framing": {"score": number, "explanation": string},
       "alternatives": {"score": number, "explanation": string},
       "information": {"score": number, "explanation": string},
       "reasoning": {"score": number, "explanation": string},
       "values_tradeoffs": {"score": number, "explanation": string},
       "bias_exposure": {"score": number, "explanation": string},
       "calibration": {"score": number, "explanation": string}
     },
     "importance": {
       "stakes": number,
       "stakes_explanation": string,
       "irreversibility": number,
       "irreversibility_explanation": string,
       "actionability": number,
       "actionability_explanation": string
     }
   }

   ---DECISION---
   <DECISION_TEXT>
   ```

   The subagent returns JSON like:
   ```json
   {
     "quality_dims": {
       "framing": {"score": 0.8, "explanation": "The storage-engine question was framed clearly against a stated concurrency requirement."},
       "alternatives": {"score": 0.7, "explanation": "SQLite and Postgres were both named and compared."},
       "information": {"score": 0.6, "explanation": "Concurrency need was stated but no measured load data was cited."},
       "reasoning": {"score": 0.75, "explanation": "The chosen option follows directly from the stated concurrency requirement."},
       "values_tradeoffs": {"score": 0.5, "explanation": "Operational cost of Postgres was not explicitly weighed."},
       "bias_exposure": {"score": 0.8, "explanation": "No evidence of anchoring or motivated reasoning."},
       "calibration": {"score": 0.5, "explanation": "No expressed confidence was stated to check against."}
     },
     "importance": {
       "stakes": 8.0,
       "stakes_explanation": "Affects the shared event ledger used by every tenant.",
       "irreversibility": 0.6,
       "irreversibility_explanation": "Migrating storage engines later is possible but costly.",
       "actionability": 1.0,
       "actionability_explanation": "Directly determines what gets built next."
     }
   }
   ```

3. **Write the scores to a file** and submit to the ledger, referencing the
   `batch_id` and capture index from the earlier `ingest.batch_classified`
   submission:

   ```bash
   cat > /tmp/hivemind-scores.json <<'EOF'
   { ... subagent output ... }
   EOF

   HIVEMIND_AGENT_SESSION="${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.scored \
     --batch-id "$EDGE_BATCH_ID" \
     --capture-index 0 \
     --scores /tmp/hivemind-scores.json \
     --agent-tool claude \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --scorer-model "claude-haiku-4-5-20251001"
   ```

   `--capture-index` is the 0-based position of the decision capture within
   the JSON array you submitted to `ingest.batch_classified` (usually `0` for
   a single-decision batch). The command resolves `--batch-id` +
   `--capture-index` to the canonical capture node internally — never
   construct the `capture:{event_id}:{idx}` node-id format yourself. It fails
   if the referenced capture is not a `"decision"` (evidence, hypothesis,
   blocker, and other capture kinds are not scored) or if no matching batch is
   found.

4. **Verify** the score event landed:

   ```bash
   hivemind --hivemind-dir "$HIVEMIND_DIR" query get_recent_activity --limit 5
   ```

   Look for an item with `"event_type": "decision.scored"`.

### Scoring Schema Contract

The scores JSON must match the `quality_dims` + `importance` schema produced
by `src/scorer.rs`'s `SCORER_PROMPT` (the same shape shown in step 2 above).
All 7 quality dimensions and all 3 importance factors are required, each with
an `explanation`. Scores outside `[0,1]` are clamped server-side; `stakes`
must be non-negative — the server validates and clamps the same way for both
the keyless plugin path and its own Haiku call. Do not invent a
`capture_node_id`; it is derived from `--batch-id` + `--capture-index`.

## Quality Rules

- Use the helper or HiveMind CLI commands; do not write directly to the ledger from this skill.
- Preserve disagreement and staleness. Do not hide contested, rejected, refuted,
  or superseded context just because it complicates the answer.
- Write the rationale in durable organizational language. Avoid "because we
  discussed it" or "seems best" as the only why.
- Include all meaningful options in `--options`, and set `--chose` only when a
  selected option exists.
- Do not invent evidence, hypothesis, or decision ids. Query first if unsure.
- Prefer `decision.capture` for new bundled proposals. Use direct event verbs
  only for status transitions or graph relations that already have ids.

## Backend Selection

The default local backend is whatever `--hivemind-dir` points at, normally
`./hivemind/`. To switch to a shared backend, set `HIVEMIND_DIR` to the shared
ledger mount or service-managed directory before running the same commands. The
capture verb, actor format, and query behavior stay unchanged.
