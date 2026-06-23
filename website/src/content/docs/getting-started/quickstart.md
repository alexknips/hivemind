---
title: Quickstart
description: Connect your coding agent or capture your first decision in under five minutes.
---

import { Steps } from '@astrojs/starlight/components';

Choose your path:

- **[Connect your agent via MCP](#connect-via-mcp)** — the primary onboarding. No CLI needed.
- **[Use the CLI](#cli-quickstart)** — for self-hosters who prefer the terminal.

---

## Connect via MCP

<Steps>

1. **Add HiveMind to your MCP client**

   For Claude Code, add to `.mcp.json` in your project root (local stdio — no install if you have the binary):

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

   Or use the managed remote server — see [MCP Setup](/guides/mcp-setup/) for the remote config
   and bearer-token setup.

2. **Reload your MCP client**

   Claude Code picks up `.mcp.json` changes on restart. The HiveMind tools appear
   in your agent's tool list.

3. **Start capturing decisions**

   Ask your agent: *"Capture a decision: we chose SQLite for the MVP ledger because it
   has zero infra overhead. We considered Postgres and rejected it as premature."*

   The agent calls `capture_decision` and the decision lands in your shared ledger.

4. **Query what's been decided**

   Ask your agent: *"What architecture decisions have we made about the database?"*

   The agent calls `search_decisions` or `get_relevant_decisions` and returns the structured
   decision graph — who decided, when, what options were weighed.

</Steps>

---

## CLI quickstart

For self-hosted installs. First [install the binary](/getting-started/install/).

The fastest first run uses an isolated temporary ledger:

```bash
hivemind --actor human:alice quickstart
```

This creates a temp ledger, records a sample decision, queries it back, and prints the result.
No files are left behind.

### Step by step

<Steps>

1. **Capture a decision**

   ```bash
   hivemind --actor human:alice --hivemind-dir ./hivemind emit decision.proposed \
     --title "Use HiveMind for architecture decisions" \
     --rationale "We need a durable record of what was decided, why, and by whom" \
     --topic-keys onboarding,architecture \
     --options hivemind,notes \
     --chose hivemind
   ```

   Every write requires `--actor`. Use `human:<id>` for humans, `agent:<tool>:<session>` for agents.
   `--hivemind-dir` sets the ledger location; it is created on first write.

2. **Query it back**

   ```bash
   hivemind --hivemind-dir ./hivemind query search_decisions \
     --topic onboarding \
     --limit 5
   ```

3. **Record evidence**

   ```bash
   hivemind --actor human:alice --hivemind-dir ./hivemind emit evidence.recorded \
     --content "Alternatives considered: shared docs wiki, Notion, plain git notes"
   ```

4. **Record a hypothesis (still in flight)**

   ```bash
   hivemind --actor human:alice --hivemind-dir ./hivemind emit hypothesis.recorded \
     --statement "HiveMind will reduce decision-reconstruction time by 80%"
   ```

5. **Review all recent decisions**

   ```bash
   hivemind --hivemind-dir ./hivemind query decisions_list --limit 10
   ```

</Steps>

### What just happened

- An append-only **event ledger** was created at `./hivemind/ledger.sqlite`.
- Each `emit` command appended a typed event with actor provenance, a UUID, and a timestamp.
- The `query` commands derived the current status from graph edges without touching the write path.
- Status (`proposed`, `accepted`, `contested`, `superseded`) is never stored — it is always derived.

### Actor format

| Context | Actor format |
|---------|-------------|
| Human (named) | `human:alice` or `human:alice@example.com` |
| Agent (Claude Code) | `agent:claude:<session-id>` |
| Agent (Codex) | `agent:codex:<session-id>` |
| System | `system:ci` |

If `--actor` is omitted, HiveMind falls back to `HIVEMIND_ACTOR`, then `human:<git config user.email>`.

---

## Next steps

- [MCP Setup](/guides/mcp-setup/) — full remote and local MCP configuration for all clients
- [Agent Capture](/guides/agent-capture/) — how agents capture decisions automatically
- [Architecture](/concepts/architecture/) — understand the three-layer design
