# HiveMind Capture Plugin

This directory is both the Codex capture plugin and the Claude Code
`hivemind-capture` plugin. The Claude package installs:

- `/hivemind-capture:capture` for explicit writes through the deterministic
  capture helper. Use `--kind decision`, `--kind evidence`, or
  `--kind hypothesis` when the kind is known.
- `/hivemind-capture:capture-decision` as the legacy decision-only wrapper for
  `emit decision.capture`.
- `/hivemind-capture:query-decisions` for free-text `query recall` reads —
  "what did we decide about X?", never a decision id. For single-decision
  follow-up (why, still-holds, disagree, supersede), install the
  `hivemind-context` plugin instead.
- `/hivemind-capture:classify-queue` (Worker A) — drains the pending
  classification work queue using the agent's subscription seat. Run after a
  session to classify batches that the server-side classifier has not yet
  processed. See [Queue drain](#queue-drain-worker-a) below.
- Hooks on Claude Code's `AskUserQuestion` tool: the question is recorded as an
  ask when the agent puts it to you, and your answer as a decision that answers
  it. See [AskUserQuestion hooks](#askuserquestion-hooks).
- A `hivemind` MCP stdio server wired to `hivemind mcp`.
- The `hivemind-capture` skill for capture boundaries and provenance rules.
- The Claude `active-capture` skill, which nudges `/capture <text>
  [--kind decision|evidence|hypothesis|blocker]` during live durable-decision
  moments while avoiding synthetic test data and routing chatter.
- The `citation` skill, which guides the agent to pin URL versions (commit
  hash, revision ID, or access date) when a link appears in a capture context.

The shared helper defaults to `source=agent` and derives
`actor_id=agent:<tool>:<name>` from a **stable** identity, not a raw session id:
Gas City's `GC_AGENT` (mirrored in `GC_ALIAS`) is checked first, since it names
a fixed crew/polecat/refinery slot that survives process restarts. Only when
neither is set does it fall back to a raw per-run session id (`CODEX_THREAD_ID`,
`CODEX_SESSION_ID`, or `CODEX_TASK_ID` under Codex; `CLAUDE_SESSION_ID` or
`CLAUDE_CODE_SESSION_ID` under Claude Code), then Gas City's session-instance
variables (`GC_SESSION_ID`/`GC_SESSION_NAME`).

## Install

Install the HiveMind CLI first so `hivemind` is on `PATH`.

From Claude Code, add this repository as a marketplace and install the plugin:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

For local development from this checkout:

```text
/plugin marketplace add .
/plugin install hivemind-capture@hivemind
/reload-plugins
```

Claude Code can also test the plugin directory directly:

```bash
claude --plugin-dir ./plugins/hivemind-capture
```

For rig-local dogfooding, copy the active capture skill into the checkout's
project skills directory:

```bash
plugins/hivemind-capture/scripts/install-active-capture-skill.sh --project-dir .
```

The default ledger is the project-local `./hivemind/` directory. The bundled MCP
server descriptor pins that location so agents launched from this checkout use
the same ledger without per-session setup. To use another ledger for slash
commands, set the plugin option `hivemind_dir`, export `HIVEMIND_DIR`, or pass
`--hivemind-dir`.

## Provenance Defaults

Claude plugin writes default to `actor_id=agent:claude:<name>` and
`source=agent`. The slash command prefers Gas City's stable `GC_AGENT`/
`GC_ALIAS` slot identity, falling back to `CLAUDE_SESSION_ID` or
`CLAUDE_CODE_SESSION_ID` when neither is set; the bundled MCP server starts
with `--agent-tool claude` and uses the same environment resolution for write
tools when `actor_id` is omitted.

Codex skill writes use the same convention with `actor_id=agent:codex:<name>`,
preferring `GC_AGENT`/`GC_ALIAS` and falling back to `CODEX_SESSION_ID`,
`CODEX_TASK_ID`, or `HIVEMIND_CODEX_SESSION`.

Bare terminal writes such as `hivemind emit decision.proposed ...` or
`hivemind emit decision.capture ...` default to
`actor_id=human:<git config user.email>` and `source=human` when `--actor` is
not supplied and no agent is present in the environment. A typed
`--actor human:<name>` is recorded as given.

## Projects

Every decision capture says which project it belongs to and how that was
determined. The capture helper passes `--project-from-context` (so does the
bundled MCP server, and the Codex skill's direct `hivemind ... emit
decision.capture` form), so the CLI works the project out from where the agent
is running. First match wins:

1. `--project HANDLE`, when the caller names one. An unregistered handle is
   refused with the register command, never guessed.
2. The `.hivemind-project` files of the folders the uncommitted change touches
   (working-tree diff plus the staged set); when none of those files sits under
   one, the nearest `.hivemind-project` walking up from the working directory.
   The file holds one project handle; a marker nested inside an attached folder
   is a sub-project and, being nearer, wins.
3. The project anchored to the Gas City rig (`GC_RIG`).
4. The current project set with `hivemind project use`. It is a per-machine
   setting kept for the CLI's `--actor` (by default the person at the terminal),
   not a ledger fact.

A change that touches folders of several projects is one decision, recorded for
the nearest project they are all part of, and the CLI says so on its stderr
(`recorded for platform: this change spans auth and billing`; the helper passes
that line through rather than folding it into the one-line confirmation). With no
project in common it is saved to the actor's personal project and the CLI names
the projects it spans and how to move it. A spanning change is never refused and
never dropped.

When none applies, the decision is saved to the actor's personal project. The
confirmation line says so and how to attach the folder:

```text
Captured HiveMind decision decision-... in ./hivemind.
project: personal:agent:claude (personal_fallback) — saved to your personal project; ...
this folder is not attached to a project yet; register the project (hivemind project register <handle>) and commit a .hivemind-project file holding the handle in this folder
```

The line is the whole recipe: register the project (`hivemind project register
billing`) and commit a `.hivemind-project` file containing `billing` in the
folder. `hivemind project anchor --kind folder` only records a fact about the
project and does not attach the folder; the marker file is what a capture reads,
and `--kind rig` with the rig's name is what binds a Gas City rig.

A session in a Gas City rig that no project in the ledger is anchored to is
refused instead of filed under the personal project: it is writing to the wrong
ledger, and the refusal names the rig, the ledger, and the ways out. A folder
marker, a rig anchor, or a current project is tried first.

Topic keys are declared per project. A capture under a registered project may use
only the keys that project declared; pass `--declare-topic KEY` (each also in
`--topic-keys`) to add a new one, and the confirmation lists what was declared.
`hivemind project show billing` lists a project's keys. A personal project has no
vocabulary.

A capture from an attached folder confirms with the project and how it was
found instead, for example `project: billing (folder_marker)`. Evidence and
hypothesis captures carry no project. `/hivemind-context:supersede` works the
same way for the replacement decision; with none found it stays in the project
of the decision it replaces. A wrong project is fixed with `hivemind move
"<description>" --to <handle>`. The `--project-from-context` flag needs a
`hivemind` CLI that has it; an older CLI refuses the flag, so update the CLI
along with the plugin. Nothing here applies over HTTP: MCP-over-HTTP takes the
project as an argument and REST takes none (its decisions land in the personal
project), and only the CLI and the stdio MCP server this plugin ships fill it in
from context. See
[`docs/AGENT_DECISION_CAPTURE.md`](../../docs/AGENT_DECISION_CAPTURE.md#which-project-a-capture-lands-in)
and [`docs/MULTI_TENANCY.md`](../../docs/MULTI_TENANCY.md#projects-inside-a-tenant).

## AskUserQuestion hooks

An agent asking you a question is an act with a time. The plugin's hooks on Claude
Code's `AskUserQuestion` tool (`hooks/hooks.json`) record it, so the ledger holds
when a question was asked and when it was answered:

| Hook | When | Writes |
|---|---|---|
| `PreToolUse` | the agent calls `AskUserQuestion` | one ask per question (MCP `request_decision`): the question, who asked, the time of the call |
| `PostToolUse` | you have answered | one decision per answered question (MCP `capture_decision`) that answers the same question |

The ask and the decision name the same question, so `hivemind why` on the decision
shows `asked_at` and `answered_at`, and the request leaves the waiting list
(`hivemind query get_waiting_requests`). The two hooks share no state.

What the decision carries:

- **Decided by** `human:<your git email>` (set `HIVEMIND_HUMAN_ACTOR=human:<name>`
  to change it), **recorded by** the agent (`agent:claude:<name>`).
- **Options** are the ones the agent offered, with their descriptions; **chosen** is
  the one you picked. Several picks in a multi-select, or words of your own
  ("Other"), become one combined option (`Search + Export + Other (own words)`); what
  you picked into it is not listed as turned down. Your own words are quoted
  verbatim.
- **Title** is `<header>: <chosen>`. **Rationale** is a fixed sentence saying no
  reasons were given, because none were. **Rests on** is a bet with nothing
  declared, because nothing was stated with the answer; add what it rests on later
  with `hivemind ground`.
- **Project** is worked out from where the agent runs, as for any capture
  (`HIVEMIND_PROJECT` names one outright).

What it does not do:

- It records only real asks. The tool call is the act and its time is the ask's
  time. An agent deciding alone, you deciding unprompted and imports write no ask,
  and nothing is back-dated. A question you decline or leave unanswered stays
  waiting; nothing is recorded for it.
- It keeps no averages and no per-person figures: the waiting list is a to-do list,
  and each decision carries its own two times.
- It never gets in the agent's way. A failed write is logged (to stderr and to
  `hook.log`, readable by you alone, under `${CLAUDE_PLUGIN_DATA}/ask-hook/`, else
  `~/.local/state/hivemind-ask-hook/`) and the question still appears; the hook
  exits 0 with nothing on stdout. When the ask could not be recorded, the answer is
  still captured, without `asked_at`.

Where it writes, and the settings it reads:

| Setting | Effect |
|---|---|
| none | the local ledger (`HIVEMIND_DIR`, else the plugin's `hivemind_dir` option, else `<repo root>/hivemind`, the same rule as `/hivemind-capture:capture`), through `hivemind mcp` |
| `HIVEMIND_API_URL`, `HIVEMIND_API_KEY` | that server's `POST /mcp` with the key as the bearer token; the token decides who is writing |
| `HIVEMIND_CAPTURE_BIN` | the `hivemind` binary to run (default: `hivemind` on `PATH`) |
| `HIVEMIND_HUMAN_ACTOR`, `HIVEMIND_PROJECT` | who answers, and the project to file under |
| `HIVEMIND_ASK_HOOK_DISABLE=1` | do nothing |

It needs `python3` (without it nothing is recorded, the question is unaffected) and
a `hivemind` CLI or server that has the ask verb (`hivemind ask`, MCP
`request_decision`). A server without it refuses the ask (logged) and the answer is
still sent as a decision, without `asked_at`. Upgrade the server before relying on
the asks.

## Verify

Capture one decision:

```text
/hivemind-capture:capture "Use the Claude plugin for local capture" --kind decision --title "Use the Claude plugin for local capture" --rationale "The plugin installs commands, skill guidance, and MCP without project-local setup" --topic-keys agents,claude,distribution --options "Claude plugin,Manual MCP setup" --chose "Claude plugin" --rests-on-assumption "Contributors install Claude Code plugins from the repository marketplace"
```

Every decision capture says what it rests on: `--rests-on-decision` (a decision we
already made), `--rests-on-evidence` with `--evidence-source` (something observed),
`--rests-on-assumption`, or `--bet` (nothing yet). A capture that names none is
refused and writes nothing. The decider's own words are not a grounding; they go
in `--quote` with `--question`. See the `hivemind-capture` skill.

Capture one evidence item:

```text
/hivemind-capture:capture "The plugin smoke test wrote a decision and queried it back" --kind evidence
```

The command prints a one-line confirmation and a query suggestion. Run the
suggested query or use:

```text
/hivemind-capture:query-decisions --actor-id agent:claude:<name> --source agent --limit 10
```

The MCP server appears as `hivemind` in Claude Code's MCP tool list after the
plugin is loaded.

## Queue drain (Worker A)

HiveMind accumulates unclassified ingest batches in a work queue. Worker A
drains this queue using the agent's subscription seat — no API key required.

### Run manually

```text
/hivemind-capture:classify-queue
```

The command lists pending batches, classifies each one using your subscription
model, and writes `IngestBatchClassified` events. Report at the end: batches
processed and total captures written.

Pass `--limit N` to cap the number of batches processed per run (default 20):

```text
/hivemind-capture:classify-queue --limit 5
```

### Check queue depth

```bash
hivemind classify-queue list --json | jq length
```

### Session hook (optional)

Add `scripts/check-classify-queue.sh` as a `Stop` hook in `.claude/settings.json`
to see a nudge when unclassified batches are waiting:

```json
{
  "hooks": {
    "Stop": [{
      "matcher": "",
      "command": "plugins/hivemind-capture/scripts/check-classify-queue.sh"
    }]
  }
}
```

The hook only prints a one-line notification; it does not drain autonomously.

### Bounds

- Each batch = one model invocation (subscription-seat bound).
- Large backlogs drain across multiple `/classify-queue` runs — the queue persists.
- The server-side classifier (Worker B) and Worker A share the same queue;
  concurrent classification is idempotent (last-writer-wins per batch).

## Uninstall

```text
/plugin uninstall hivemind-capture@hivemind --prune
```

Remove the marketplace if no other HiveMind plugins are installed:

```text
/plugin marketplace remove hivemind
```
