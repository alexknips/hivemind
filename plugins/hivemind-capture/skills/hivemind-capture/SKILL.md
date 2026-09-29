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
reject it, or supersede it, using the existing decision id. A hit that lists
`missing_terms` (`missing=` in `--summary`) matched only some of your words:
read it before treating it as covering the ground.

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
  --options "Cap at 30 seconds,Uncapped" \
  --chose "Cap at 30 seconds" \
  --rests-on-decision "exponential backoff for retries" \
  --rests-on-evidence "Uncapped backoff reached 8 minutes in the June incident" \
  --evidence-source "https://example.test/incidents/june"
```

### Options are short human labels

Write each option the way a person would say it in a sentence: a short phrase
with capitals and spaces (`"Cap at 30 seconds"`, `"Direct CLI"`), quoted so the
shell keeps it as one value. `--chose` repeats one of those labels exactly.

- No slugs (`cap-at-30`, `direct-cli`) and no codes (`cap30`, `q1-a-...`,
  `option-a`). `why` and `verify` print the label as written, and a reader
  should not have to decode it. A slug that slips through is read as words and
  shown with what was recorded beside it (`Upheld (recorded as: name-a-upheld)`).
- The choice is a fact on the record: `--chose` alone says which option won.
  Never put the winning letter, "chosen", or "recommended" inside a label.
- One label per real alternative. No bucket such as "other options", and no
  label that packs several answers together. Put detail in the rationale.
- A label cannot contain a comma: `--options` splits on commas.

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
  --options "Keep 3 attempts,Raise to 5 attempts" \
  --chose "Keep 3 attempts" \
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

### Questions you ask with AskUserQuestion (Claude Code)

The Claude plugin's hooks record these for you. When you call `AskUserQuestion`,
the question is written as an ask at that moment; when the person answers, their
answer is written as a decision they made (`decided by` them, you as the
recorder), with the options you offered, the one they picked, and any words of
their own quoted verbatim. Do not capture that answer again: a second decision on
the same question reads as `also answered by`, or as a conflict.

- The recorded answer's rationale is the note the person wrote on their pick, word
  for word (a note too short to read on its own is quoted instead). Only with
  neither a note nor words of their own does it say no reasons were given. It rests
  on a bet with nothing declared, because nothing was stated. If the person then
  says what it rests on
  (an observation, a decision it follows from, an assumption), record that with
  `hivemind ground "<decision>" ...`; do not file a duplicate.
- A question you never put to the person (deciding alone, working from the
  task) has no ask and no answer to record. Capture the decision as usual, with
  `--question` if you want it findable.
- A question the person did not answer stays listed as waiting
  (`hivemind query get_waiting_requests`). Nothing is recorded for them.

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
     --options "Direct CLI,MCP server,Git hook" \
     --chose "Direct CLI" \
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
     --options "Direct CLI,MCP server,Git hook" \
     --chose "Direct CLI" \
     --rests-on-assumption "Agents already have shell access to the ledger"
   ```

   If the `hivemind` binary is not on `PATH`, run the same command from a
   HiveMind source checkout with `cargo run --` before the flags.

   From the Claude Code plugin, prefer the installed slash command:

   ```text
   /hivemind-capture:capture "Prefer direct CLI capture before MCP" --kind decision --title "Prefer direct CLI capture before MCP" --rationale "The write path is explicit, testable, and does not depend on hooks or MCP setup" --topic-keys agents,capture --options "Direct CLI,MCP server,Git hook" --chose "Direct CLI" --rests-on-assumption "Agents already have shell access to the ledger"
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
     --options "Local ledger,Shared ledger" \
     --chose "Shared ledger" \
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
project and that the folder is not attached to a project yet, with the two steps
that attach it: register the project (`hivemind project register <handle>`) and
commit a `.hivemind-project` file holding the handle in the folder.
`hivemind project anchor --kind folder` does not attach a folder: it records a
fact that a capture never reads back. Relay that reminder to the user rather
than dropping it: a
decision left in a personal project is easy to lose track of, and it can be moved
to the right project later.

A session in a Gas City rig that no project in this ledger is anchored to is
not an unattached folder: it is writing to the wrong ledger. The capture is
refused with the rig, the ledger and the ways out (write to the ledger that holds
the rig's project, `hivemind project anchor --handle <project> --kind rig --value
<rig>`, or name the project with `--project`). Nothing is written, so relay the
refusal instead of retrying blindly.

`supersede` (from the `hivemind-context` plugin) works the same way, except
that a replacement with no project found stays in the project the decision it
replaces is filed in now (after a move, the project it was moved to). A decision
saved to a personal project is replaced into your own personal project. Evidence
and hypotheses carry no project.

## Topic Keys Are Declared Per Project

Topic keys are lowercase kebab (`Pricing Model` becomes `pricing-model`). A
registered project has a vocabulary of declared keys, and a capture filed under it
may use only those. Before inventing a key, look at what the project has
(`hivemind project show <handle>` lists them as `topics=`). When you need a new key,
say so in the capture with `--declare-topic KEY` (each declared key must also be in
`--topic-keys`); the confirmation lists what was declared. A key you neither found
in the vocabulary nor declared is refused with the declared keys and how to add it.
Do not declare a key just to get past the refusal: use an existing one when it
means the same thing. A personal project has no vocabulary, so any key works there.

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

   Write `title` as one plain sentence a stranger understands, and keep
   `rationale` readable without the tracker. If a tracker or ticket id (a
   bead, Jira, GitHub issue) appears in the source text, leave it out of the
   title and rationale — it is not part of the captured content.

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

## Assess via Haiku Subagent (Keyless)

When you want to assess the seven quality dimensions of a decision without a
server-side `ANTHROPIC_API_KEY`, spawn a Haiku subagent inside your own
session — same idea as batch capture above. The subagent runs the assessor
prompt against one decision and writes the result directly to the ledger
using `emit decision.scored`. No HiveMind-held key is required — it rides the
user's own Claude subscription.

Assess a decision right after submitting it — either a classifier-extracted
capture (`emit ingest.batch_classified`, while its `batch_id` and position in
the captures array are still at hand) or a decision captured directly
(`emit decision.capture` / `emit decision.proposed`, which returns its
`decision_id`). This mirrors the server's own Layer-3 scorer (`src/scorer.rs`),
which runs the same prompt against `ANTHROPIC_API_KEY`-backed decisions of
either kind — the two paths never double-assess the same decision (the
background scorer skips decision ids that already carry a version-2
`decision.scored` event).

### Scoring Workflow

1. **Collect the decision's own recorded text** — title, question, quote and
   rationale for a directly captured decision (plus each option's label and
   description); title, rationale, options considered and chosen option for a
   classifier-extracted capture. This is the same content submitted at
   capture, and the only text a quote may be copied from.

2. **Spawn a Haiku subagent** (keeps scoring off your main context) with the
   following prompt, substituting `<DECISION_TEXT>` with the fields from step
   1, one per labeled line:

   ```
   You are the HiveMind decision quality assessor.

   HiveMind records organizational decisions. Assess one decision's seven quality
   dimensions, EX ANTE — only from what was knowable at decision time. Never judge
   a dimension by how the decision turned out.

   The seven dimensions:
     framing          — Was the right problem/question framed?
     alternatives     — Were genuine alternatives generated and considered?
     information      — Was relevant information gathered and used?
     reasoning        — Is the inference from information to choice sound?
     values_tradeoffs — Were values and tradeoffs made explicit and weighed?
     bias_exposure    — Exposure to cognitive distortions (anchoring, confirmation,
                        sunk-cost, framing, motivated reasoning), other than
                        confidence miscalibration.
     calibration      — Does expressed confidence match the evidence?

   Framing and values_tradeoffs have no mechanical floor beyond "a question was
   recorded" — assess both. Enrich any of the other five only where the decision's
   own text gives you real grounds; leave the rest not_assessed.

   Answer each dimension one of two ways, never a placeholder:
     {"status": "assessed", "level": "none"|"partial"|"solid", "explanation": "...", "quote": "..."}
     {"status": "not_assessed", "reason": "..."}

   `level` is ordinal (solid > partial > none), never a number. `explanation` is
   always required. `quote` is REQUIRED at level "partial" or "solid": copy a
   passage VERBATIM from a single one of the decision's own recorded fields below
   — never combine two fields, paraphrase, or invent one, and never include the
   field's label. `quote` is optional at level "none" (an absence usually cannot
   be quoted). A `not_assessed` answer's `reason` says what is missing — the
   honest answer, never a guess dressed up as a score.

   The decision's recorded text follows, one field per labeled line. Quote only
   from within a single field's value, not its label.

   Return only JSON matching the schema.

   ---DECISION---
   <DECISION_TEXT>
   ```

   The subagent returns JSON like:
   ```json
   {
     "dimensions": {
       "framing": {"status": "assessed", "level": "partial", "explanation": "The storage-engine question is named but not spelled out as an explicit question.", "quote": "Use Postgres for the shared event ledger"},
       "alternatives": {"status": "assessed", "level": "partial", "explanation": "SQLite and Postgres were both named.", "quote": "sqlite"},
       "information": {"status": "not_assessed", "reason": "No evidence or measured load data is cited in the text."},
       "reasoning": {"status": "assessed", "level": "partial", "explanation": "The chosen option follows from the stated concurrency requirement.", "quote": "Concurrent multi-tenant writes are a day-one requirement"},
       "values_tradeoffs": {"status": "not_assessed", "reason": "Operational cost of Postgres is not weighed in the text."},
       "bias_exposure": {"status": "assessed", "level": "none", "explanation": "Nothing in the record bears on a distortion."},
       "calibration": {"status": "not_assessed", "reason": "No expressed confidence is stated to check against."}
     }
   }
   ```

3. **Write the scores to a file** and submit to the ledger, naming the
   decision either by `--decision-id` (a decision captured directly) or by
   the `batch_id` + capture index from an earlier `ingest.batch_classified`
   submission:

   ```bash
   cat > /tmp/hivemind-scores.json <<'EOF'
   { ... subagent output ... }
   EOF

   HIVEMIND_AGENT_SESSION="${GC_AGENT:-${GC_ALIAS:-${CLAUDE_SESSION_ID:-${CLAUDE_CODE_SESSION_ID:-${GC_SESSION_ID:-${GC_SESSION_NAME:-manual-session}}}}}}"

   # Directly captured decision:
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.scored \
     --decision-id "$DECISION_ID" \
     --scores /tmp/hivemind-scores.json \
     --agent-tool claude \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --model "claude-haiku-4-5-20251001" \
     --prompt-version "plugin-assessment-v1"

   # Classifier-extracted capture:
   hivemind --hivemind-dir "$HIVEMIND_DIR" emit decision.scored \
     --batch-id "$EDGE_BATCH_ID" \
     --capture-index 0 \
     --scores /tmp/hivemind-scores.json \
     --agent-tool claude \
     --agent-session "$HIVEMIND_AGENT_SESSION" \
     --model "claude-haiku-4-5-20251001" \
     --prompt-version "plugin-assessment-v1"
   ```

   Exactly one of `--decision-id` or `--batch-id`/`--capture-index` is
   required. `--capture-index` is the 0-based position of the decision capture
   within the JSON array you submitted to `ingest.batch_classified` (usually
   `0` for a single-decision batch). The command resolves `--batch-id` +
   `--capture-index` to the canonical capture node internally — never
   construct the `capture:{event_id}:{idx}` node-id format yourself. It fails
   if the referenced capture is not a `"decision"` (evidence, hypothesis,
   blocker, and other capture kinds are not scored), if no matching batch or
   decision is found, or if any quote given does not occur verbatim in the
   decision's own recorded text — in every refusal, nothing is written.
   `--prompt-version` names the wording you gave the subagent (e.g.
   `plugin-assessment-v1`), so an assessment can be traced back to it.

4. **Verify** the score event landed:

   ```bash
   hivemind --hivemind-dir "$HIVEMIND_DIR" query get_recent_activity --limit 5
   ```

   Look for an item with `"event_type": "decision.scored"`.

### Scoring Schema Contract

The scores JSON must match the `dimensions` schema produced by
`src/scorer.rs`'s `ASSESSOR_PROMPT` (the same shape shown in step 2 above),
optionally with an `importance` object alongside it (a separate axis, never
elicited by this prompt). Every one of the seven dimensions is required,
either `assessed` (`level`, `explanation`, and a non-blank `quote` — required
at level `partial` or `solid`, optional at `none`) or `not_assessed` (a
non-blank `reason`). There is no placeholder float and nothing is clamped: a
malformed payload, an unrecorded decision, or a quote not found verbatim in
the decision's own text is refused whole, and nothing is written — the server
validates the keyless plugin path exactly as strictly as its own Haiku call.
Do not invent a `capture_node_id`; a capture's target is derived from
`--batch-id` + `--capture-index`.

## Quality Rules

- Use the helper or HiveMind CLI commands; do not write directly to the ledger from this skill.
- Preserve disagreement and staleness. Do not hide contested, rejected, refuted,
  or superseded context just because it complicates the answer.
- Write the rationale in durable organizational language. Avoid "because we
  discussed it" or "seems best" as the only why.
- Write each option as a short human label (see "Options are short human
  labels"), never a slug or a letter code.
- Write the title as one plain sentence a stranger understands, and keep the
  rationale readable without the tracker. A tracker or ticket id (a bead,
  Jira, or GitHub issue) goes in the capture's source reference or as linked
  evidence — never in the title, and not as the rationale's subject.
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
  "options": [{ "label": "Cap at 30 seconds" }, { "label": "Uncapped" }],
  "chosen_option_label": "Cap at 30 seconds",
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
