---
title: CLI Reference
description: Complete reference for the hivemind command-line interface.
---

## Global flags

| Flag | Description |
|------|-------------|
| `--actor <id>` | Actor making this request. Format: `human:<id>` or `agent:<tool>:<session>` |
| `--hivemind-dir <path>` | Ledger directory (default: `./hivemind/`). Created on first write. |
| `--json` | Emit structured JSON output |
| `--graph-backend <kuzu\|sqlite>` | Graph projection backend (default: `sqlite` in-memory) |

## Emit commands

All `emit` commands append an event to the ledger. They require `--actor`.

### `emit decision.proposed`

```
hivemind emit decision.proposed
  --title <text>
  --rationale <text>
  [--topic-keys <key,key,...>]
  [--options <opt,opt,...>]
  [--chose <option>]
  [--supersedes <decision-id>]
```

Prints the new decision ID on success. Add `--json` for a structured envelope.

### `emit decision.capture`

Noninteractive shorthand for agent use. Defaults actor to `agent:<tool>:<session>`,
records `source=agent`.

```
hivemind emit decision.capture
  --title <text>
  --rationale <text>
  [--topic-keys <key,...>]
  [--options <opt,...>]
  [--chose <option>]
```

### `emit decision.accepted`

```
hivemind emit decision.accepted --target <decision-id>
```

### `emit decision.rejected`

```
hivemind emit decision.rejected --target <decision-id> --reason <text>
```

### `emit decision.superseded`

```
hivemind emit decision.superseded --target <decision-id> --by <new-decision-id>
```

### `emit evidence.recorded`

```
hivemind emit evidence.recorded --content <text> [--url <url>] [--for <decision-id>]
```

### `emit hypothesis.recorded`

```
hivemind emit hypothesis.recorded --statement <text> [--for <decision-id>]
```

## Query commands

All `query` commands return JSON. They never write to the ledger.

### `query get_decision`

```
hivemind query get_decision --id <decision-id>
```

Returns the full decision node with current derived status, supersession chain tip,
and `hypothesis_refuted` flag if any assumed hypothesis has been refuted.

### `query search_decisions`

```
hivemind query search_decisions
  [--topic <key>]
  [--status <proposed|accepted|contested|superseded>]
  [--actor <pattern>]
  [--since <duration>]     # e.g., 7d, 30d, 1h
  [--limit <n>]            # default 20
  [--offset <n>]
```

Returns `{ decisions: [...], truncated: bool, total: n }`.

### `query recent`

```
hivemind query recent
  [--limit <n>]
  [--actor <pattern>]
  [--since <duration>]
  [--unreviewed-only]
```

### `query get_supersession_chain`

```
hivemind query get_supersession_chain --id <decision-id>
```

Returns the full chain from the given decision back to the original proposal.

### `query disagree`

```
hivemind query disagree [--since <duration>]
```

Returns all decisions with `contested` status.

## Other commands

### `quickstart`

```
hivemind --actor human:<id> quickstart
```

Creates an isolated temporary ledger, records a sample decision, queries it back,
and prints the result. No files are left behind.

### `review`

```
hivemind --actor human:<id> review
  [--actor <pattern>]      # filter by actor (e.g., 'agent:*')
  [--since <duration>]
  [--unreviewed-only]
```

Interactive terminal review flow. See [Human Review](/guides/human-review/).

### `mcp`

```
hivemind mcp [--session-id <id>]
```

Start the MCP stdio server. See [MCP Setup](/guides/mcp-setup/).

### `dump`

```
hivemind dump --format <dot|json>
```

Export the current projected graph as DOT (Graphviz) or JSON.

### `import documents`

```
hivemind --actor <id> import documents [--file <path> | <directory>]
  [--on-conflict <keep_existing|supersede|contest|add_context>]
  [--json]
```

Import structured decision notes from markdown or text files. Only explicit
`Decision:` blocks are imported. Re-importing identical input is a no-op.

### `suggest document-candidates`

```
hivemind suggest document-candidates
  --file <path>
  [--llm-response <path>]
```

Layer-3 command. Reads an unstructured document and optional LLM response to
produce pending-review decision candidates. Does not write ledger events.

### `tui`

```
hivemind tui [--q <query>] [--topic <key>] [--status <status>] [--dot-output <path>]
```

Read-only terminal UI for decision search and graph navigation.
Requires build with `--features tui`.

## Environment variables

| Variable | Description |
|----------|-------------|
| `HIVEMIND_DIR` | Default ledger directory |
| `HIVEMIND_ACTOR` | Default actor if `--actor` is omitted |
| `HIVEMIND_GRAPH_BACKEND` | Graph backend: `sqlite` (default) or `kuzu` |
| `HIVEMIND_VERSION` | Pin version for the installer script |
| `HIVEMIND_INSTALL_DIR` | Install destination for the installer script |
