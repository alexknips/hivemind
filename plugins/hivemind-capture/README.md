# HiveMind Capture Plugin

This directory is both the Codex capture plugin and the Claude Code
`hivemind-capture` plugin. The Claude package installs:

- `/hivemind-capture:capture-decision` for explicit writes through
  `emit decision.capture`.
- `/hivemind-capture:query-decisions` for bounded `query search_decisions`
  reads.
- A `hivemind` MCP stdio server wired to `hivemind mcp`.
- The `hivemind-capture` skill for capture boundaries and provenance rules.

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

The default ledger is the project-local `./hivemind/` directory. The bundled MCP
server descriptor pins that location so agents launched from this checkout use
the same ledger without per-session setup. To use another ledger for slash
commands, set the plugin option `hivemind_dir`, export `HIVEMIND_DIR`, or pass
`--hivemind-dir`.

## Provenance Defaults

Claude plugin writes default to `actor_id=agent:claude:<session>` and
`source=agent`. The slash command uses `CLAUDE_SESSION_ID` or
`CLAUDE_CODE_SESSION_ID`; the bundled MCP server starts with `--agent-tool
claude` and uses the same session environment for write tools when `actor_id` is
omitted.

Codex skill writes use the same convention with
`actor_id=agent:codex:<session>`, deriving the session from
`CODEX_SESSION_ID`, `CODEX_TASK_ID`, or `HIVEMIND_CODEX_SESSION`.

Bare terminal writes such as `hivemind emit decision.proposed ...` default to
`actor_id=human:<git config user.email>` and `source=human` when `--actor` is
not supplied.

## Verify

Capture one decision:

```text
/hivemind-capture:capture-decision --title "Use the Claude plugin for local capture" --rationale "The plugin installs commands, skill guidance, and MCP without project-local setup" --topic-keys agents,claude,distribution --options plugin,manual-mcp --chose plugin
```

The command prints a one-line confirmation and a query suggestion. Run the
suggested query or use:

```text
/hivemind-capture:query-decisions --actor-id agent:claude:<session> --source agent --limit 10
```

The MCP server appears as `hivemind` in Claude Code's MCP tool list after the
plugin is loaded.

## Uninstall

```text
/plugin uninstall hivemind-capture@hivemind --prune
```

Remove the marketplace if no other HiveMind plugins are installed:

```text
/plugin marketplace remove hivemind
```
