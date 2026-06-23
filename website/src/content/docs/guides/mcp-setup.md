---
title: MCP Setup
description: Connect Claude Code, Cursor, or any MCP client to HiveMind — remote or local.
---

HiveMind exposes its full decision-graph surface as an MCP server. You can connect via
the **managed remote server** (no local install, GitHub/Google login) or run the
**local stdio server** yourself as part of a self-hosted setup.

---

## Remote MCP — managed server

The managed HiveMind server is live at `hivemind-tti3sa.fly.dev`. Your agents connect
to the remote MCP endpoint and write to a shared, team-wide decision graph — no local
binary required. **Authentication is browser-based:** on first connect your client opens
a login page and you sign in with **GitHub or Google**. No API key or token to manage.

### Claude Code

Add to `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "hivemind": {
      "type": "http",
      "url": "https://hivemind-tti3sa.fly.dev/mcp"
    }
  }
}
```

Or from the CLI:

```bash
claude mcp add --transport http hivemind https://hivemind-tti3sa.fly.dev/mcp
```

On first use, Claude Code opens your browser to the HiveMind login — sign in with
GitHub or Google and you're connected. All 12 HiveMind tools are then available.

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "hivemind": {
      "type": "http",
      "url": "https://hivemind-tti3sa.fly.dev/mcp"
    }
  }
}
```

### Cursor

Add to `~/.cursor/mcp.json` or the project-level `.cursor/mcp.json`:

```json
{
  "mcp": {
    "servers": {
      "hivemind": {
        "url": "https://hivemind-tti3sa.fly.dev/mcp"
      }
    }
  }
}
```

---

## Local stdio MCP — self-hosted

If you are running your own HiveMind instance, use the local stdio server. The server
is a thin transport over the same commands layer the CLI uses.

### Start the server

```bash
hivemind --hivemind-dir ./hivemind/ mcp
```

The server reads from stdin and writes to stdout in the MCP protocol format.

### Claude Desktop (local)

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

### Claude Code (local)

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

### Cursor (local)

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

---

## Available tools

The HiveMind MCP server exposes 12 tools. See [MCP Tools reference](/reference/mcp-tools/)
for full parameter documentation.

| Tool | Type | Description |
|------|------|-------------|
| `capture_decision` | write | Record a decision with rationale and options |
| `capture_evidence` | write | Record supporting evidence for a decision |
| `capture_hypothesis` | write | Record a hypothesis still in flight |
| `disagree_decision` | write | Contest a decision as an actor |
| `supersede_decision` | write | Supersede a prior decision with a new one |
| `get_decision` | read | Retrieve a decision by ID with derived status |
| `get_relevant_decisions` | read | Search by topic, status, actor, or time window |
| `get_supersession_chain` | read | Walk the full supersession history backward |
| `search_decisions` | read | Full-text search across the ledger |
| `dump_graph` | read | Export the full projected graph (DOT or JSON) |
| `hivemind_compact_view` | read | Compact summary of a decision and its context |
| `summarize_decisions` | read | LLM-friendly summary of a decision set |

---

## Actor requirement (local stdio)

Every capture call requires an explicit `actor_id`. Use the originating tool as a prefix:

```
agent:claude:<session-id>
agent:cursor:<session-id>
agent:codex:<session-id>
```

The server records `source=agent` and a per-session `source_ref` for every write.

---

## Next steps

- [MCP Tools reference](/reference/mcp-tools/) — full parameter documentation for all 12 tools
- [Agent Capture guide](/guides/agent-capture/) — how agents capture decisions automatically
- [Self-host install](/getting-started/install/) — install the binary and run your own server
