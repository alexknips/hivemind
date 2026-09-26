---
name: hivemind-capture
description: Capture durable organizational decisions from Claude Code or Codex into HiveMind with full provenance. Use when an agent session chooses between options, records architecture/integration/policy rationale, accepts or rejects a decision, supersedes prior direction, or needs to verify agent-captured decisions. Every decision capture must say what the decision rests on. Do not use for chat logs, task tracking, private scratch memory, or speculative notes.
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
- What the decision rests on: a decision already made, something observed, an
  assumption, or a declared bet (see "What Does This Rest On?" below).

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
in the new decision rationale. The replacement does not rest on the decision it
replaces: name what overturned the old one instead (see "What Does This Rest
On?" below).

## Capture Once, At One Granularity

A capture-worthy exchange can span one message or a whole session, and can
carry one choice or several — a numbered list of design answers, several
linked decisions made together. Before your first `decision.capture` for that
exchange, decide its shape and hold it for the rest of the exchange:

- **One decision** when several answers only make sense together as a single
  design; the rationale spells out each part inline.
- **Several decisions** when each answer has its own options and chosen
  option and could later be superseded, accepted, or queried on its own.

Never do both for the same exchange. A bundled capture followed by per-item
captures of the same content — or per-item captures followed by a rollup that
just restates them — is a duplicate recording of one decision, not two
decisions.

Before capturing, check whether the ground is already recorded — not only to
verify your own write afterward (step 6 below). This matters most at the
start of a session, after a restart, or when resuming a conversation someone
else may have already captured pieces of: your own transcript does not carry
a prior session's writes, and `query recall` is not scoped to your own actor
by default, so it surfaces decisions any actor already recorded. This is a
deterministic free-text lookup, not the similarity/ranking/inference this
skill otherwise avoids:

```bash
hivemind --hivemind-dir "$HIVEMIND_DIR" query recall "<free-text description of what you're about to capture>"
```

or, if the `hivemind-context` plugin is installed, its fluent
`/hivemind-context:recall "<free text>"` verb. A hit that already covers this
ground means: don't capture it again — extend it with new evidence, accept or
reject it, or supersede it, using the existing decision id.

## What Does This Rest On?

Before you write a decision, answer one question: **what does this decision rest
on?** Every decision capture must answer it. A capture that names nothing is
refused (exit 2) and nothing is written, so answer up front rather than after
the refusal.

### The loop: consult, decide, capture

1. **Consult before you act.** Ask what is already decided about the ground you
   are about to change: the `hivemind-context` plugin's `situational` (it reads
   your working diff) or `recall "<free text>"`.
2. **Decide.**
3. **Capture with what you consulted as the premise.** A standing decision that
   shaped your choice is what the new one rests on. Name it the way you would
   describe it (`--rests-on-decision "bearer auth on postgres"`), or pass the
   `#N` handle or `decision-...` id the consult printed.

Consulting first is what makes the answer cheap: the premise is already in front
of you. Consulting only finds what the decision rests on; it never decides
whether to capture (that stays the deterministic trigger above).

### The four answers

Give one or more; each flag can repeat.

| It rests on | Flag | What to say |
|---|---|---|
| A decision we already made | `--rests-on-decision "<description>"` | The decision as you would describe it, or `'#N'` / the `decision-...` id from the consult. |
| Something observed | `--rests-on-evidence "<what was observed>"` with `--evidence-source "<where>"` | The observation, and where it was seen: a URL, `file@commit`, a test run, a measurement. Pin the version (commit, revision, or access date). |
| Something we assume | `--rests-on-assumption "<statement>"` | The assumption, as a statement that can later turn out false. |
| Nothing yet | `--bet ["<statement>"]`, optionally `--would-change-if "<what would change our mind>"` and `--check-by YYYY-MM-DD` | A declared judgement call with no grounding yet. It reads as a bet, which is honest; an invented grounding is not. |

`--evidence-source` lines up with `--rests-on-evidence` by position: one source
per item, or none. An answer that fits none of the four is still an answer:
record it as `--rests-on-assumption` with the text as you have it. Never drop it
and never force it into evidence.

```bash
plugins/hivemind-capture/scripts/capture.sh "Cap retry delay at 30 seconds" \
  --kind decision \
  --title "Cap retry delay at 30 seconds" \
  --rationale "An uncapped exponential delay stalled clients for minutes after a short outage" \
  --topic-keys retries \
  --options cap30,uncapped \
  --chose cap30 \
  --rests-on-decision "exponential backoff for retries" \
  --rests-on-evidence "Uncapped backoff reached 8 minutes in the June incident" \
  --evidence-source "https://example.test/incidents/june"
```

### The decider's words are not a grounding

When a person's words drove the decision (a Slack message, a chat reply, "go
with B"), those words say *who decided and what they said*. They do not say
*what the decision rests on*. Never record them as evidence.

- **Wrong:** `--rests-on-evidence "Alex in Slack: keep it at 3, more just hides
  outages" --evidence-source "Slack #eng"`. That files the decider as the
  decision's own evidence, so the record says it is true because someone said so.
- **Right:** the words go in `--quote` (verbatim and self-contained), paired with
  `--question` (the question they answered, in your own words), with
  `--decided-by human:<name>`. Then ask what *those words* rest on: the
  observation behind them, a decision they follow from, or an assumption. If you
  do not know and the decider gave no reason, say so with `--bet`.

```bash
plugins/hivemind-capture/scripts/capture.sh "Keep the retry budget at 3" \
  --kind decision \
  --title "Keep the retry budget at 3 attempts" \
  --rationale "More retries hide an outage from the operator instead of surfacing it" \
  --topic-keys retries \
  --options three,five \
  --chose three \
  --decided-by human:alex \
  --quote "keep it at 3, more just hides outages" \
  --question "Should the worker retry budget go from 3 to 5 attempts?" \
  --rests-on-evidence "The 5-retry config stretched the June outage to 40 minutes" \
  --evidence-source "https://example.test/incidents/june"
```

### Confidence, and the question it answers

- Pass `--confidence low|medium|high` only when the decider's own words say how
  sure they are ("pretty sure", "just a guess"). Never your own certainty, never
  inferred from tone. Otherwise omit it.
- Suggested, never required: name the question the decision answers with
  `--question` ("Which retry policy for the worker pool?"), one line. It stands
  alone; a person's words answered it, `--question` with `--quote` carries both.
  Two captures whose question is the same after lowercasing, collapsing spaces
  and dropping trailing punctuation share one question node, so a decision that
  answers the same question again, or answers it differently, is found: the
  brief shows `also answered by`, and two accepted answers that chose
  differently are flagged as a conflict. Reuse the exact words when you are
  answering a question that was already answered; a decision already captured
  without one can be linked afterwards with `hivemind ground "<decision>" --answers
  "<question>"`.

### When the capture is refused

Add the grounding and re-run the same command. Never drop the capture.

- **Nothing named.** The error lists the four ways to answer. Pick one.
- **Ambiguous premise.** `'<text>' matches 2 decisions...` with numbered
  candidates and nothing written. Re-run right away with `--rests-on-decision
  '#N'` (or the `decision-...` id) in place of the description; `#N` refers to
  that last candidate list.
- **No match.** `no decision matches '<text>'`. The decision you meant is not
  recorded, or you worded it differently: try other words from `recall`. If the
  ground truly is not recorded, answer with what you do have (something
  observed, an assumption, or a bet). Never invent an id.
- **Quote without question.** A quote needs the question it answers. A question
  stands alone.

If the reply reports `premise_stale`, a decision you named has since been
superseded or rejected. The capture is recorded and reads as stale; check it
with `verify` and consider whether you meant the decision that replaced it. For
the same reason, do not name the decision you are superseding as the premise of
its replacement.

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
     --chose direct-cli \
     --rests-on-assumption "Agents already have shell access to the ledger"
   ```

   The helper records `source=agent` and derives `actor_id=agent:<tool>:<name>`.
   The `<name>` is a **stable identity**, not a raw session id: Gas City's
   `GC_AGENT` (mirrored in `GC_ALIAS`) is checked first, because it names a
   fixed crew/polecat/refinery slot that survives process restarts. Only when
   neither is set does the helper fall back to a raw per-run session id
   (`CODEX_THREAD_ID`/`CODEX_SESSION_ID`/`CODEX_TASK_ID` for Codex,
   `CLAUDE_SESSION_ID`/`CLAUDE_CODE_SESSION_ID` for Claude Code), then Gas
   City's session-instance variables, then `manual-session`. Folding a raw
   session id into `actor_id` would make the same physical agent appear as a
   different actor on every restart (hivemind-zdsh.9) -- `source_ref` is where
   that per-run session id belongs instead: the helper sets it to the raw
   session id when one exists, or to `actor_id` only when it doesn't, unless
   explicitly overridden.

   The CLI applies the same stable-identity-first resolution for Codex and
   Claude unless `--actor-id` is explicitly provided. Use
   `--agent-tool codex --agent-session <session>` only when the helper is
   unavailable or when overriding the environment-derived defaults.

3. Capture a new proposed decision directly when the helper is unavailable:

   ```bash
   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.capture \
     --project-from-context \
     --agent-tool codex \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --title "Prefer direct CLI capture before MCP" \
     --rationale "The write path is explicit, testable, and does not depend on hooks or MCP setup" \
     --topic-keys agents,capture \
     --options direct-cli,mcp,hook \
     --chose direct-cli \
     --rests-on-assumption "Agents already have shell access to the ledger"
   ```

   If the `hivemind` binary is not on `PATH`, run the same command from a
   HiveMind source checkout with `cargo run --` before the flags.

   From the Claude Code plugin, prefer the installed slash command:

   ```text
   /hivemind-capture:capture "Prefer direct CLI capture before MCP" --kind decision --title "Prefer direct CLI capture before MCP" --rationale "The write path is explicit, testable, and does not depend on hooks or MCP setup" --topic-keys agents,capture --options direct-cli,mcp,hook --chose direct-cli --rests-on-assumption "Agents already have shell access to the ledger"
   ```

4. Attach existing evidence or hypotheses only when their ids are already known.
   Ids that already exist count as what the decision rests on; prefer the
   by-description flags above whenever you do not hold the id:

   ```bash
   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.capture \
     --project-from-context \
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
   with the same actor id (the replacement decision itself is a capture, so it
   answers "what does this rest on?" like any other):

   ```bash
   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}}"
   hivemind --actor "agent:codex:$HIVEMIND_AGENT_SESSION" \
     --hivemind-dir "$HIVEMIND_DIR" emit decision.accepted \
     --decision-id decision-001
   ```

   ```bash
   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}}"
   hivemind --actor "agent:codex:$HIVEMIND_AGENT_SESSION" \
     --hivemind-dir "$HIVEMIND_DIR" emit decision.superseded \
     --old decision-001 \
     --new decision-002
   ```

6. Verify the write through the read path. Prefer the fluent `recall` verb
   (free text first, never a decision id — see
   [docs/AGENT_DECISION_CONTEXT.md](../../../../docs/AGENT_DECISION_CONTEXT.md)
   for the full fluent surface shipped in the `hivemind-context` plugin):

   ```bash
   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" query recall \
     --actor-id "agent:codex:$HIVEMIND_AGENT_SESSION" \
     --source agent \
     --limit 10
   ```

## Which Project A Capture Lands In

Every decision belongs to a project, and the capture says which and how that
was determined. You do not work it out: the helper (and the direct
`hivemind ... emit decision.capture` form above) passes
`--project-from-context`, and the CLI answers, first match wins:

1. The project you named with `--project <handle>` (only when you were told
   which project — never guess a handle; an unregistered one is refused).
2. The `.hivemind-project` files of the folders the uncommitted change touches
   or, when none of those files sits under one, the nearest `.hivemind-project`
   walking up from the working directory. A file holds one project handle; a
   nested one is a sub-project and, being nearer, wins. A change that touches
   several projects is recorded for the nearest project they are all part of; with
   none in common it is saved to your personal project and the CLI names the
   projects it spans.
3. The project anchored to the Gas City rig (`GC_RIG`).
4. The current project set with `hivemind project use`.
5. None of these: the decision is saved to your personal project.

The confirmation shows the outcome as a `project: <handle> (<how>)` line
(`folder_marker`, `rig`, `current_project`, `stated`, or `personal_fallback`).
On the personal fallback it also says the decision was saved to your personal
project and that the folder is not attached to a project yet, with a hint that
names `hivemind project anchor`. A folder is really attached by registering the
project (`hivemind project register <handle>`) and committing a
`.hivemind-project` file holding the handle; a folder anchor is only recorded and
never read back. Relay that reminder to the user rather than dropping it: a
decision left in a personal project is easy to lose track of, and it can be moved
to the right project later.

`supersede` (from the `hivemind-context` plugin) works the same way, except
that a replacement with no project found stays in the project of the decision
it replaces. Evidence and hypotheses carry no project.

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
retrospective extraction over accumulated context. The batch path does not ask
what a decision rests on: extracted decisions read `nothing declared` until
someone grounds them (the `hivemind-context` plugin's `ground` verb).

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

   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}"
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit ingest.batch_classified \
     --captures /tmp/hivemind-captures.json \
     --agent-tool claude \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --classifier-model "claude-haiku-4-5-20251001"
   ```

   For Codex sessions:
   ```bash
   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CODEX_THREAD_ID:-${CODEX_SESSION_ID:-${CODEX_TASK_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}}"
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
   hivemind --hivemind-dir "$HIVEMIND_DIR" query recall --limit 5
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

   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}"
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
  selected option exists. `--chose` means the decision was already made — it
  self-accepts immediately (or accepts from `--decided-by` when someone else
  decided). Pass `--still-proposed` instead when floating a leaning that still
  awaits someone else's decision.
- When a human explicitly delegated a class of decision to you and you decided
  within that scope, add `--delegated-by human:<name>` to the capture. It marks
  "decided by the agent, within a scope the human delegated" so it reads
  differently from "the agent decided alone" (no flag). Do not use it when a
  human chose (that is `--decided-by`), and never claim a delegation you were
  not given — capture without the flag instead. The same value on every capture
  in that scope is how a standing delegation is expressed.
- Do not invent evidence, hypothesis, or decision ids. Query first if unsure.
- Say what every decision rests on, and never file the decider's own words as
  evidence: they go in `--quote` with `--question`. Do not invent a grounding to
  get past the refusal; a bet is the honest answer when there is nothing yet.
- Prefer `decision.capture` for new bundled proposals. Use direct event verbs
  only for status transitions or graph relations that already have ids.

## Backend Selection

The default local backend is whatever `--hivemind-dir` points at, normally
`./hivemind/`. To switch to a shared backend, set `HIVEMIND_DIR` to the shared
ledger mount or service-managed directory before running the same commands. The
capture verb, actor format, and query behavior stay unchanged.

**This CLI transport only reaches a local directory or a database the CLI
process can open directly** (a mounted SQLite dir, or Postgres via
`--database-url`/`HIVEMIND_DATABASE_URL` on a `shared-backend-postgres`
build). It has no HTTP-client mode — it cannot reach a remote cell's `/v1`
API or `/mcp` endpoint (see `docs/SELF_HOSTING.md`'s "Using the CLI / MCP
from local agents"). If your session's `hivemind-dir`/database target isn't
reachable from where this CLI runs, captures made with `capture.sh` /
`capture-decision.sh` land in whatever local fallback directory `--hivemind-
dir` resolves to instead — silently fragmenting the ledger rather than
failing loudly. Verify with `query recall` (step 6 above) after any backend
change, especially the first capture in a new environment.

## MCP-over-HTTP Capture (remote cell, no local CLI reach)

When a self-hosted cell is reachable only over HTTP — no local directory, no
direct Postgres credential on this session — configure `.mcp.json` to point
at the cell instead of spawning a local `hivemind mcp` process
(`docs/SELF_HOSTING.md`'s "Configuring an agent to use MCP-over-HTTP"):

```json
{
  "mcpServers": {
    "hivemind": {
      "url": "http://<cell-host>:8080/mcp",
      "headers": { "Authorization": "Bearer <role-or-user-token>" }
    }
  }
}
```

When this is how `hivemind` is registered, call the `mcp__hivemind__*` tools
(`capture_decision`, `capture_evidence`, `capture_hypothesis`,
`disagree_decision`, `supersede_decision`, and the read tools) directly
instead of shelling out to `capture.sh` — the CLI transport above cannot
reach this backend at all.

`capture_decision` and `supersede_decision` ask the same question through a
required `grounding` array (at least one item), one item per answer:

```json
{
  "title": "Cap retry delay at 30 seconds",
  "rationale": "An uncapped exponential delay stalled clients for minutes after a short outage",
  "topic_keys": ["retries"],
  "options": [{ "label": "cap30" }, { "label": "uncapped" }],
  "chosen_option_label": "cap30",
  "grounding": [
    { "kind": "decision", "description": "exponential backoff for retries" },
    { "kind": "evidence", "content": "Uncapped backoff reached 8 minutes in the June incident", "source": "https://example.test/incidents/june" },
    { "kind": "assumption", "statement": "Clients retry from a single region" },
    { "kind": "bet", "statement": "30 seconds is long enough", "would_change_if": "retries still storm", "check_by": "2026-12-01" }
  ]
}
```

The four kinds are the same four answers. An ambiguous or unmatched
`description` comes back as a successful `{outcome: "ambiguous" | "not_found",
field: "grounding[i]"}` result with nothing written (there is no `#N` over MCP):
re-call with that item's `decision_id`. The decider's words go in `quote` with
`question` (the question alone is fine: it names the shared question node, and the
reply's `question_id` says which), and `expressed_confidence` follows the same
rule as `--confidence`. To link a decision captured without a question, call
`ground_decision` with `answers`.

**Always pass `actor_id` explicitly on every write call** when the token is
shared across more than one session (e.g. one per-role token for a pool of
agent instances): `agent:<tool>:<name>`, using the same stable-identity-first
resolution as the CLI helper (`GC_AGENT`/`GC_ALIAS` first, then a raw session
id, never omitted). Without an explicit `actor_id`, the server falls back to
`agent:mcp-http:<mcp-session-id>` — a fresh, unstable id per connection, not
the calling agent's identity — or, if no session id is present at all, to the
token's own bound identity, collapsing every caller sharing that token into
one actor. This is a caller-asserted override, not a verified one: anyone
holding the token can claim any `actor_id`, so only rely on it across callers
you already trust (see `docs/SELF_HOSTING.md`).
