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
| `--graph-backend <memory\|kuzu>` | Graph projection backend (default: `memory`) |

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

### `emit ingest.batch_classified`

Keyless batch path: submit pre-classified captures from a plugin or edge session
directly — no `ANTHROPIC_API_KEY` needed. The server classifier skips this batch
because no companion `IngestBatchReceived` event exists for the batch ID.

```
hivemind emit ingest.batch_classified
  --captures <path>              # JSON file: array of CaptureItem objects
  [--classifier-model <name>]    # default: claude-haiku-4-5-20251001
  [--schema-version <n>]         # must be "2"; default: 2
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

To list all `contested` decisions (note: `disagree` is a top-level write command,
not a query subcommand):

```
hivemind query search_decisions --status contested
```

### `query recall`

Layer-3 search + summarise in one call. Answers "what was decided about X?".

```
hivemind query recall [<free-text query>]
  [--topic <key,...>]
  [--status <status,...>]
  [--since <duration>]
  [--limit <n>]          # default 5
```

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

### `query compact-view`

Layer-3 signal/noise filter over a decision's subgraph.

```
hivemind query compact-view --id <decision-id>
```

### `query get_decision_neighborhood`

```
hivemind query get_decision_neighborhood --id <decision-id>
  [--depth <n>]            # default 1
  [--relations <kind,...>]
  [--compact]              # return a compact-view instead of the raw neighborhood
```

### `query get_relevant_decisions`

```
hivemind query get_relevant_decisions --topic <key> [--status <status>]
```

### `query get_active_decision_blockers`

```
hivemind query get_active_decision_blockers
  [--decision-id <id,...>]
  [--topic <key,...>]
  [--owner <actor-id,...>]
  [--blocked-actor <actor-id,...>]
  [--priority <level,...>]
  [--limit <n>]
```

### `query get_recent_activity`

```
hivemind query get_recent_activity
  [--actor-id <id,...>]
  [--topic <key,...>]
  [--status <status,...>]
  [--limit <n>]
```

### `query get_decisions_changed_since`

```
hivemind query get_decisions_changed_since
  [--since-offset <ledger-offset>]
  [--since-ts <timestamp>]
  [--until-offset <ledger-offset>]
  [--until-ts <timestamp>]
  [--limit <n>]
```

### `query get_decisions_added_since`

```
hivemind query get_decisions_added_since
  [--since <duration>]
  [--since-offset <ledger-offset>]
  [--since-ts <timestamp>]
  [--limit <n>]
```

### `query export_read_only_summary`

```
hivemind query export_read_only_summary
```

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
  [--extractor-command <command>]
  [--extractor-arg <arg>]
  [--llm-response <path>]
  [--json]
```

Import decisions from markdown or text files. All imported decisions land in
the ledger immediately as **unreviewed** (proposed, not auto-accepted) and flow
into the review step (`hivemind review --unreviewed-only`).

Auto-detection: if a file contains explicit `Decision:` blocks, the
deterministic block parser runs. If it does not, the file is treated as prose
and the extractor configured via `--extractor-command` (or a pre-computed
`--llm-response`) is used to extract candidate decisions. Prose extraction
requires one of those two flags; without them a prose file produces no output.

Re-importing identical input is a no-op. Changed same-id re-imports report
conflicts by default; resolve with `--on-conflict`.

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
| `HIVEMIND_GRAPH_BACKEND` | Graph backend: `memory` (default) or `kuzu` |
| `HIVEMIND_VERSION` | Pin version for the installer script |
| `HIVEMIND_INSTALL_DIR` | Install destination for the installer script |
