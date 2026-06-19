---
title: Quickstart
description: Capture and query your first decision in under five minutes.
---

import { Steps } from '@astrojs/starlight/components';

The fastest first run uses an isolated temporary ledger:

```bash
hivemind --actor human:alice quickstart
```

This creates a temp ledger, records a sample decision, queries it back, and prints the result.
No files are left behind.

## Step by step

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
   hivemind --hivemind-dir ./hivemind query recent --limit 10
   ```

</Steps>

## What just happened

- An append-only **event ledger** was created at `./hivemind/ledger.sqlite`.
- Each `emit` command appended a typed event with actor provenance, a UUID, and a timestamp.
- The `query` commands derived the current status from graph edges without touching the write path.
- Status (`proposed`, `accepted`, `contested`, `superseded`) is never stored — it is always derived.

## Actor format

| Context | Actor format |
|---------|-------------|
| Human (named) | `human:alice` or `human:alice@example.com` |
| Agent (Claude Code) | `agent:claude:<session-id>` |
| Agent (Codex) | `agent:codex:<session-id>` |
| System | `system:ci` |

If `--actor` is omitted, HiveMind falls back to `HIVEMIND_ACTOR`, then `human:<git config user.email>`.

## Next steps

- [Agent Capture](/guides/agent-capture/) — let your AI agents capture decisions automatically
- [MCP Setup](/guides/mcp-setup/) — connect Claude Desktop, Claude Code, or Cursor
- [Architecture](/concepts/architecture/) — understand the three-layer design
