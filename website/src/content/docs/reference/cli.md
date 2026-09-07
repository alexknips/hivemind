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

### `emit decision.scored`

Keyless scoring path: submit a pre-scored quality/importance assessment from a
plugin or edge session directly — no `ANTHROPIC_API_KEY` needed. The target
capture node is resolved from a prior `ingest.batch_classified` batch via
`--batch-id` + `--capture-index`, so callers never construct the
`capture:{event_id}:{idx}` node-id format themselves. Fails if the referenced
capture is not a `"decision"`. Scores are validated and clamped with the same
invariants as the server's own Haiku-backed scorer (`src/scorer.rs`).

```
hivemind emit decision.scored
  --batch-id <id>                 # batch_id from a prior emit ingest.batch_classified
  --capture-index <n>              # 0-based index of the decision capture in that batch
  --scores <path>                  # JSON file: {"quality_dims": {...}, "importance": {...}}
  [--scorer-model <name>]          # default: claude-haiku-4-5-20251001
  [--weight-version <tag>]         # default: v1
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

Alias: `chain`.

```
hivemind query get_supersession_chain (<free-text description> | --id <decision-id>)
  [--pick <n>]             # disambiguate when a description matches more than one decision
  [--topic <key>]          # narrow description resolution to this topic
```

Returns the full chain from the resolved decision back to the original proposal. A free-text
`<description>` resolves to a decision the same way `search`/`recall` do (deterministic term +
topic + recency ranking; see `docs/AGENT_FLUENT_QUERYING.md` in the repo for the full design); if
it matches more than one decision with no clear best match, the response is `{"outcome":
"ambiguous", "candidates": [...]}` instead — re-run with `--pick <n>`, a bare `#<n>` referring to
that candidate list, or `--id`. `--id` bypasses resolution entirely and is unchanged from before.

### `query compact-view`

Layer-3 signal/noise filter over a decision's subgraph.

```
hivemind query compact-view (<free-text description> | --id <decision-id>)
  [--pick <n>]
  [--topic <key>]
```

Same fluent resolution as `get_supersession_chain` above.

### `query get_decision_neighborhood`

Alias: `why`.

```
hivemind query get_decision_neighborhood (<free-text description> | --id <decision-id>)
  [--pick <n>]
  [--topic <key>]
  [--depth <n>]            # default 1
  [--relations <kind,...>]
  [--compact]              # return a compact-view instead of the raw neighborhood
```

Same fluent resolution as `get_supersession_chain` above.

### `query get_decision_outcome`

Alias: `verify`. "Did this decision hold up?" — leads with the decision, rationale, rejected
options, who decided, and whether it still holds (superseded / stale premises / contested /
thin structure), composing `get_decision` + `get_decision_context` + `get_decision_outcome`
into one `DecisionBrief` answer. Distinct from the bulk `review` command below, which reports
across many decisions rather than answering this about one.

```
hivemind query get_decision_outcome (<free-text description> | --id <decision-id>)
  [--pick <n>]
  [--topic <key>]
```

Same fluent resolution as `get_supersession_chain` above. IDs (`decision_id`, chosen/rejected
`option_id`) appear only as trailing handles for follow-up — never required reading to
understand the answer.

### `query get_relevant_decisions`

```
hivemind query get_relevant_decisions --topic <key> [--status <status>]
```

### `query situational`

"What should I know before I touch this?" — decisions bearing on the working
situation, no id or hand-typed question needed. Matches by exact `topic_keys`
membership and by deterministic term overlap against evidence content; each
result surfaces `get_decision_outcome`'s `held_up`/`reasons` verbatim.

```
hivemind query situational
  [--paths <file,dir,...>]     # defaults to the current git diff + staged set
  [--diff]                     # read a unified diff from stdin instead
  [--branch]                   # include the current branch name's tokens
  [--cwd]                      # include the current working directory's path segments
  [--since-offset <ledger-offset>]
  [--since-ts <timestamp>]
  [--since-branch-point]       # resolve --since to this branch's merge-base commit time
  [--base <ref>]               # base ref for --since-branch-point, default origin/master
  [--limit <n>]
```

With no flags at all, defaults to the current git diff/staged set in the
working directory — the "no question needed" mode. Requires a git repository
when relying on that default, `--branch`, or `--cwd`; pass `--paths`/`--diff`
explicitly otherwise.

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

### `query score_decision`

In-house explainable quality score for a single decision.

```
hivemind query score_decision --id <decision-id>
```

Returns a score, quality tier, list of reasons, and contributing node IDs.
No LLM involved; pure graph-signal heuristics.

### `query scan_decision_quality`

Bulk in-house quality scan: scores all decisions (or a filtered subset) using
the same explainable graph-signal engine as `score_decision`. Designed for
scheduled quality-scan loops. Precision-biased: use `--min-tier` to surface
only significant or high-concern decisions.

```
hivemind query scan_decision_quality
  [--since-event-origin <offset>]   # filter to decisions proposed at or after this ledger offset
  [--limit <n>]                     # 1–1000, default 25
  [--cursor <token>]                # pagination cursor from a previous response
  [--min-tier <tier>]               # clean|minor_concerns|significant_concerns|high_concern
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
