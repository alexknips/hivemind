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

Bare terminal writes such as `hivemind emit decision.proposed ...` default to
`actor_id=human:<git config user.email>` and `source=human` when `--actor` is
not supplied.

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
this folder is not attached to a project yet; run hivemind project anchor ... to attach it
```

To attach a folder, register the project (`hivemind project register billing`)
and commit a `.hivemind-project` file containing `billing` in the folder.
`hivemind project anchor --kind folder` only records a fact about the project;
the marker file is what a capture reads, and `--kind rig` with the rig's name is
what binds a Gas City rig.

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

## Verify

Capture one decision:

```text
/hivemind-capture:capture "Use the Claude plugin for local capture" --kind decision --title "Use the Claude plugin for local capture" --rationale "The plugin installs commands, skill guidance, and MCP without project-local setup" --topic-keys agents,claude,distribution --options plugin,manual-mcp --chose plugin --rests-on-assumption "Contributors install Claude Code plugins from the repository marketplace"
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
