# Dogfood Operations

This page is the operating contract for using HiveMind inside the HiveMind
repository itself. It is about the repo-local dogfood loop, not onboarding a new
repository or running a multi-machine deployment.

The goal is simple: a new contributor or agent started in this checkout should
be able to capture a decision, query it back, and know which shared ledger they
used in under 10 minutes.

## Ledger Location

The dogfood ledger lives at:

```text
./hivemind/ledger.sqlite
```

The path is relative to the HiveMind checkout an agent was launched from. For
the shared dogfood loop, start agents from the canonical checkout:

```bash
cd /data/projects/hivemind
```

The repo-level `.mcp.json`, Claude settings, and capture plugin defaults all pin
`HIVEMIND_DIR` or `--hivemind-dir` to `./hivemind/`. The SQLite file is created
on first write.

Use a temporary ledger for throwaway tests:

```bash
HIVEMIND_DIR="$(mktemp -d)" hivemind emit decision.capture \
  --title "Smoke test isolated capture" \
  --rationale "Verify the local binary without touching the shared dogfood ledger" \
  --topic-keys dogfood,smoke \
  --options shared-ledger,temp-ledger \
  --chose temp-ledger
```

Back up the shared ledger before moving or deleting it. For a live backup, use
SQLite's backup API when `sqlite3` is available:

```bash
mkdir -p backups
sqlite3 ./hivemind/ledger.sqlite ".backup 'backups/ledger-$(date -u +%Y%m%dT%H%M%SZ).sqlite'"
```

For a simple filesystem backup, stop active writers first and copy the whole
directory so any WAL sidecar files stay with the database:

```bash
mkdir -p backups
cp -a ./hivemind "backups/hivemind-$(date -u +%Y%m%dT%H%M%SZ)"
```

For a clean local test, prefer a temporary ledger. If you intentionally need to
reset the repo-local dogfood ledger, stop active agents and move the directory
out of the way rather than deleting it:

```bash
mkdir -p backups
mv ./hivemind "backups/hivemind-reset-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir ./hivemind
```

## Actor Convention

Every event has an actor. Dogfood captures use these actor id prefixes:

- `agent:claude:<session>` for Claude Code sessions.
- `agent:codex:<session>` for Codex sessions.
- `human:<email-or-name>` for manual terminal writes by a human.

The actor identifies who took the action. The event source identifies the
capture surface:

- `source=agent` means an agent capture path wrote the event.
- `source=human` means a bare human terminal write wrote the event.
- `source_ref` is normally the actor id for local agent and human writes. Other
  integrations can use it for their external source reference.

Do not invent actor ids by hand during normal dogfood operation. Use the plugin
helper, MCP server defaults, or `decision.capture` `--agent-tool` and
`--agent-session` flags so provenance stays consistent.

## Starting An Agent

Install or build the CLI first. Agent capture helpers look for `hivemind` on
`PATH`, then fall back to a local debug binary or `cargo run` from this checkout.

```bash
cargo build
./target/debug/hivemind --version
```

If `hivemind` is not installed on `PATH`, replace `hivemind` in the examples
below with `./target/debug/hivemind` from the repository root.

### Claude Code

Start Claude Code from the HiveMind checkout:

```bash
cd /data/projects/hivemind
claude
```

This repo's `.claude/settings.json` advertises the HiveMind marketplace, enables
`hivemind-capture@hivemind`, and sets:

```json
{
  "env": {
    "HIVEMIND_DIR": "./hivemind/",
    "HIVEMIND_CAPTURE_SCRIPT": ".claude/scripts/capture-decision.sh"
  },
  "enabledPlugins": {
    "hivemind-capture@hivemind": true
  }
}
```

If Claude Code prompts for the plugin, install it and reload plugins:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

Capture a smoke decision:

```text
/hivemind-capture:capture-decision --title "Verify Claude dogfood capture" --rationale "The repo plugin should write to ./hivemind/ with Claude actor provenance" --topic-keys dogfood,claude --options plugin,manual-cli --chose plugin
```

Query it back:

```text
/hivemind-capture:query-decisions --source agent --limit 10
```

### Codex

Start Codex from the HiveMind checkout:

```bash
cd /data/projects/hivemind
codex
```

Install the Codex plugin from the repo marketplace. From this checkout, open
`/plugins`, choose `HiveMind Plugins`, and install `HiveMind Capture`. From
another checkout or machine, add the marketplace first:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

The plugin manifest is `plugins/hivemind-capture/.codex-plugin/plugin.json`.
It installs the `$hivemind-capture` skill and points Codex at the bundled MCP
descriptor. The recommended local write path is still the deterministic helper:

```bash
plugins/hivemind-capture/scripts/capture-decision.sh \
  --agent-tool codex \
  --title "Verify Codex dogfood capture" \
  --rationale "The Codex helper should write to ./hivemind/ with Codex actor provenance" \
  --topic-keys dogfood,codex \
  --options helper,manual-cli \
  --chose helper
```

Query it back:

```bash
plugins/hivemind-capture/scripts/query-decisions.sh \
  --agent-tool codex \
  --source agent \
  --limit 10
```

### Generic MCP-Aware Client

Use the repo-local MCP config shape when the client supports stdio MCP servers:

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "hivemind",
      "args": ["--hivemind-dir", "./hivemind/", "mcp", "--agent-tool", "codex"],
      "env": {
        "HIVEMIND_DIR": "./hivemind/"
      }
    }
  }
}
```

For clients launched outside `/data/projects/hivemind`, replace
`./hivemind/` with an absolute path to the dogfood ledger directory. If the
client can pass a stable session id, add it after the `mcp` subcommand:

```json
["--hivemind-dir", "/data/projects/hivemind/hivemind/", "mcp", "--agent-tool", "codex", "--session-id", "session-123"]
```

If no session id is supplied, the MCP server generates one for that server
process. The MCP `capture_decision`, `capture_evidence`, and
`capture_hypothesis` tools default `actor_id` to
`agent:<tool>:<session>` and write with `source=agent`.

## Concurrent-Use Contract

Same-host concurrent writes are allowed when every writer uses the normal
HiveMind CLI or MCP server. Those paths open the database through
`SqliteEventLedger::open`, which enables:

- SQLite WAL mode.
- `synchronous=NORMAL`.
- A 30 second `busy_timeout`.
- Single-row autocommit event inserts.
- Local filesystem locking.

Under those conditions, multiple local `hivemind mcp` or CLI subprocesses can
append to the same `ledger.sqlite`. SQLite serializes writers, the event id
defines ledger order, and replay reads events back in that order.

Do not rely on any of the following:

- Network filesystems or multi-machine coordination.
- External processes writing directly to the SQLite tables.
- Related events being adjacent in the ledger.
- A multi-event command being invisible until all of its events have appended.
- Resetting or moving `./hivemind/` while agents are running.

See [SHARED_LEDGER_WAL.md](SHARED_LEDGER_WAL.md) for the reproduction test and
the exact boundary.

## Querying The Dogfood Loop

Show recent agent captures:

```bash
hivemind --hivemind-dir ./hivemind/ query search_decisions \
  --source agent \
  --limit 20
```

Show today's agent captures in UTC:

```bash
hivemind --hivemind-dir ./hivemind/ query search_decisions \
  --source agent \
  --since "$(date -u +%Y-%m-%dT00:00:00Z)" \
  --limit 50
```

Filter to one session:

```bash
hivemind --hivemind-dir ./hivemind/ query search_decisions \
  --actor-id "agent:codex:<session>" \
  --source agent \
  --limit 10
```

Filter by topic:

```bash
hivemind --hivemind-dir ./hivemind/ query search_decisions \
  --topic dogfood \
  --limit 10
```

Use the plugin helper from an agent session:

```bash
plugins/hivemind-capture/scripts/query-decisions.sh --source agent --limit 10
```

The query response is bounded. If `truncated` is true, pass `next_cursor` back
with `--cursor` rather than increasing the query until it dumps the whole graph.

## Verification Log

### 2026-06-07T01:47:51Z

- Actors: `agent:claude:8b360208-e03d-44c1-a360-54b725147a62`,
  `agent:codex:gc-81837`
- Evidence event: `evidence-e033d69b-701d-41ee-8ee3-4af5dbed9a40`
- Decision events: gate test
  `decision-84ff6d35-909e-4df2-b838-3242205daab6`, meta-decision
  `decision-3225697e-7898-4ae6-813d-e8651179448f`
- Verification result: the original attempt-2 plugin capture resolved via
  `query get_decision`; the corrected re-run recorded the loop outcome as
  `evidence.recorded` in the canonical rig ledger; the accepted
  `M1 dogfood loop operational` meta-decision references the new evidence id.

## Troubleshooting

### The decision did not show up

Check the ledger directory first. Most misses are writes to one checkout and
queries against another:

```bash
ls -l ./hivemind/
hivemind --hivemind-dir ./hivemind/ query search_decisions --limit 5
```

If you launched the agent outside `/data/projects/hivemind`, use an absolute
`HIVEMIND_DIR` or `--hivemind-dir` pointing at the dogfood ledger.

### The actor is `human:*` instead of `agent:*`

Bare terminal commands default to `human:<git config user.email>` and
`source=human`. For agent decisions, use
`plugins/hivemind-capture/scripts/capture-decision.sh`, the Claude plugin
slash command, or `emit decision.capture --agent-tool ... --agent-session ...`.

### `source=agent` is present, but the actor is unexpected

The helper derives session identity from the client environment. Claude uses
`CLAUDE_SESSION_ID` or `CLAUDE_CODE_SESSION_ID`; Codex uses `CODEX_THREAD_ID`,
`CODEX_SESSION_ID`, or `CODEX_TASK_ID`; Gas City sessions can fall back to
`GC_SESSION_ID` or `GC_SESSION_NAME`.

Pass `--agent-tool` and `--agent-session` explicitly only when you are repairing
or testing that environment.

### The MCP tool is missing

Reload or reinstall the plugin, then confirm the MCP descriptor points at the
same ledger:

```bash
cat .mcp.json
cat plugins/hivemind-capture/.mcp.json
```

The server command should run `hivemind --hivemind-dir ./hivemind/ mcp` with
the appropriate `--agent-tool` when the client does not infer one.

### The database is locked

Normal writer contention should wait up to 30 seconds and then succeed. If it
does not, check that the ledger is on a local filesystem, that no process is
holding an external SQLite transaction open, and that no one is copying,
resetting, or moving `./hivemind/` while agents are active.

### The query hid contested or stale decisions

Treat that as a bug in the query surface or caller. Dogfood reads must preserve
`contested`, `superseded`, and stale dependency state rather than summarizing it
away. Use direct `query search_decisions`, `query get_decision`, or the MCP
read tools and preserve `truncated` and status fields in any wrapper output.
