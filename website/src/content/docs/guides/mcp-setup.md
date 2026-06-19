---
title: MCP Setup
description: Connect any MCP client — Claude Desktop, Claude Code, or Cursor.
---

HiveMind ships a bundled MCP stdio server. Any MCP-aware client can capture and
query decisions through it. The server is a thin transport over the same `commands`
and `queries` APIs the CLI uses.

## Start the server

```bash
hivemind --hivemind-dir ./hivemind/ mcp
```

The server reads from stdin and writes to stdout in the MCP protocol format.

## Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "hivemind",
      "args": ["mcp"],
      "env": {
        "HIVEMIND_DIR": "/absolute/path/to/your/hivemind/dir"
      }
    }
  }
}
```

## Claude Code

Add to `.mcp.json` in the project root:

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "hivemind",
      "args": ["--hivemind-dir", "./hivemind/", "mcp"]
    }
  }
}
```

Or set `HIVEMIND_DIR` and omit `--hivemind-dir`:

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "hivemind",
      "args": ["mcp"],
      "env": { "HIVEMIND_DIR": "./hivemind/" }
    }
  }
}
```

## Cursor

Cursor uses the same shape under `mcp.servers` in `~/.cursor/mcp.json` or the
project-level `.cursor/mcp.json`:

```json
{
  "mcp": {
    "servers": {
      "hivemind": {
        "command": "hivemind",
        "args": ["mcp"],
        "env": { "HIVEMIND_DIR": "/path/to/hivemind/" }
      }
    }
  }
}
```

## Available tools

The server exposes seven tools:

| Tool | Layer | What it wraps |
|------|-------|--------------|
| `capture_decision` | write | `emit decision.proposed` |
| `capture_evidence` | write | `emit evidence.recorded` |
| `capture_hypothesis` | write | `emit hypothesis.recorded` |
| `get_decision` | read | `query get_decision` |
| `get_relevant_decisions` | read | `query get_relevant_decisions` |
| `get_supersession_chain` | read | `query get_supersession_chain` |
| `dump_graph` | read | `dump --format dot` |

## Actor requirement

Every capture call requires an explicit `actor_id`. Use the originating tool as a prefix:

```
agent:claude:<session-id>
agent:cursor:<session-id>
agent:codex:<session-id>
```

The server records `source=agent` and a per-session `source_ref` for every write.
Pass `--session-id` to override the auto-generated session identifier.

## Kuzu backend

If the binary was built with `--features graph-kuzu`, set `HIVEMIND_GRAPH_BACKEND=kuzu`
to use the persistent Kuzu projection instead of the in-memory default:

```json
{
  "env": {
    "HIVEMIND_DIR": "./hivemind/",
    "HIVEMIND_GRAPH_BACKEND": "kuzu"
  }
}
```
