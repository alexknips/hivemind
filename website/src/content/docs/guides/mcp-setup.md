---
title: MCP Setup
description: Connect Claude Code, Cursor, or any MCP client to your self-hosted HiveMind.
---

HiveMind exposes its full decision-graph surface as an MCP server. Connect your agents
to your **self-hosted cell** over HTTP (a shared, team-wide decision graph), or run the
**local stdio server** yourself for single-user use.

---

## Self-hosted cell — HTTP

Your [self-hosted cell](../../getting-started/install/) serves MCP at `/mcp`. Agents
connect to that endpoint and write to a shared, team-wide decision graph — no local
binary required on the agent's machine. **Authentication is a bearer token:**
[provision a tenant](../../getting-started/install/#connect-your-agent) and use the
`hm_tk_...` token it returns. The examples below use `http://localhost:8080/mcp`;
substitute your cell's address.

### Claude Code

Add to `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "hivemind": {
      "type": "http",
      "url": "http://localhost:8080/mcp",
      "headers": { "Authorization": "Bearer hm_tk_..." }
    }
  }
}
```

Or from the CLI:

```bash
claude mcp add --transport http hivemind http://localhost:8080/mcp \
  --header "Authorization: Bearer hm_tk_..."
```

Reload Claude Code and all HiveMind tools are available.

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

```json
{
  "mcpServers": {
    "hivemind": {
      "type": "http",
      "url": "http://localhost:8080/mcp",
      "headers": { "Authorization": "Bearer hm_tk_..." }
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
        "url": "http://localhost:8080/mcp",
        "headers": { "Authorization": "Bearer hm_tk_..." }
      }
    }
  }
}
```

---

## Local stdio MCP

For single-user use, run the local stdio server. The server
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

The HiveMind MCP server exposes 26 tools. See [MCP Tools reference](../../reference/mcp-tools/)
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
| `get_situational_decisions` | read | Decisions bearing on a situation — matches by topic or evidence overlap, no ID needed |
| `get_supersession_chain` | read | Walk the full supersession history backward |
| `get_decision_neighborhood` | read | One-hop graph neighborhood around a decision — actors, options, evidence, hypotheses |
| `search_decisions` | read | Full-text search across the ledger |
| `recent_decisions` | read | List recently proposed decisions |
| `dump_graph` | read | Export the full projected graph (DOT or JSON) |
| `get_decision_outcome` | read | Whether a decision still holds — superseded, stale premises, contested, or thin |
| `hivemind_compact_view` | layer-3 | Compact summary of a decision and its context |
| `recall_decisions` | layer-3 | Search + ranked digest — answers "what was decided about X?" |
| `summarize_decisions` | layer-3 | Concise text summary of one or more decisions |
| `decision_quality_candidates` | layer-3 | Bulk quality-signal pull for external scorers |
| `get_decision_context` | layer-3 | Context record for a decision — authorship, review depth, evidence richness |
| `decision_context_candidates` | layer-3 | Bulk context-feature pull, the independent-variable side of quality scoring |
| `score_decision` | layer-3 | Explainable quality score in [0,1] with contributing reasons |
| `scan_decision_quality` | layer-3 | Bulk quality scan across all decisions using the same scoring engine |
| `scan_misfiled_decisions` | layer-3 | Flag decisions carrying a caller-named "foreign" topic key — report only, never moves anything |
| `analyze_failure_modes` | layer-3 | Aggregate failure-rate patterns across authorship, review, and context |
| `classify_queue_list` | read | List pending ingest batches with turn text, plus today's classification budget (HTTP transport only) |
| `classify_queue_submit` | write | Submit captures for one or more pending batches as one classification event (HTTP transport only) |

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

- [MCP Tools reference](../../reference/mcp-tools/) — full parameter documentation for all 26 tools
- [Agent Capture guide](../agent-capture/) — how agents capture decisions automatically
- [Self-host install](../../getting-started/install/) — install the binary and run your own server
