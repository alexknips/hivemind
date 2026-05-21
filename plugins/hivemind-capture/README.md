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

The default ledger is `${CLAUDE_PROJECT_DIR}/hivemind`. To use a shared backend
or another local ledger, set the plugin option `hivemind_dir`, export
`HIVEMIND_DIR`, or pass `--hivemind-dir` to the commands.

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
