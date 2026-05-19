# Agent Decision Capture

Status: slice-1 local prototype for `hivemind-claude-codex-agent-capture-tco7`.

HiveMind exposes a noninteractive CLI path for Claude, Codex, and similar coding
agents to record a decision directly into the local ledger:

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --agent-tool codex \
  --agent-session "$CODEX_SESSION_ID" \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options direct-cli,mcp,hook \
  --chose direct-cli
```

The command writes canonical ledger events. The decision proposal and its
fan-out relation events carry:

- `source=agent`
- `actor_id=agent:<tool>:<session>` unless `--actor-id` is provided
- `source_ref=<actor_id>` unless `--source-ref` is provided

Use `--evidence` and `--hypotheses` with existing evidence and hypothesis ids
when the decision depends on already captured context.

## Claude

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --agent-tool claude \
  --agent-session "${CLAUDE_SESSION_ID:-manual-session}" \
  --title "Keep capture in the commands layer" \
  --rationale "The write path should validate and append events without query-time inference" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

## Codex

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --agent-tool codex \
  --agent-session "${CODEX_SESSION_ID:-manual-session}" \
  --title "Prefer direct CLI capture before MCP" \
  --rationale "Codex can invoke the same local command in any checkout" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

## Reliability Tradeoffs

Direct CLI/API capture is the critical path because it is explicit, testable,
and writes to the ledger in one local process. Skills and instructions improve
discoverability, but they do not guarantee that an agent will call a tool.
Hooks are supplemental because they can be skipped, disabled, or misinstalled.
MCP is a good follow-up interface once the service boundary and auth model are
clear, but the ledger should not require MCP for local capture.
