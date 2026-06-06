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
