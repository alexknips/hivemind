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

This repository also ships a project-local Claude Code command:

```bash
/capture-decision --title "Keep capture in the commands layer" \
  --rationale "The write path should validate and append events without query-time inference" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

The command calls `.claude/scripts/capture-decision.sh`. By default it records
manual slash-command captures as `actor_id=human:<git-user>` with
`source=human`. Pass `--source agent` when Claude Code is recording an
autonomous agent decision; that uses `agent:claude:<session>` and
`source=agent`.

### Claude Code Distribution Bundle

This repository also ships a Claude Code marketplace at
`.claude-plugin/marketplace.json`. The marketplace exposes
`plugins/hivemind-capture` as the `hivemind-capture@hivemind` plugin.

Install from Claude Code:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

The repository-level `.claude/settings.json` advertises that marketplace and
enables `hivemind-capture@hivemind` so trusted checkouts prompt contributors to
install it. The plugin includes:

- `/hivemind-capture:capture-decision`, which defaults to
  `actor_id=agent:claude:<session>` and prints a one-line confirmation plus a
  query suggestion.
- `/hivemind-capture:query-decisions`, which runs bounded
  `query search_decisions` reads without ranking or summarizing.
- `.mcp.json`, which wires the `hivemind` MCP server to `hivemind mcp`.
- The `hivemind-capture` skill for durable decision boundaries and provenance
  rules.

The default backend is `${CLAUDE_PROJECT_DIR}/hivemind`. To switch to a shared
backend, set the plugin option `hivemind_dir`, export `HIVEMIND_DIR`, or pass
`--hivemind-dir` to the command.

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

### Codex Distribution Bundle

Codex exposes several extension surfaces relevant to HiveMind capture:

- `AGENTS.md` gives Codex repository and global instructions before work starts.
  This is useful for pointing contributors at HiveMind capture guidance, but it
  is not an installable transport. See
  <https://developers.openai.com/codex/guides/agents-md>.
- Skills package reusable instructions, resources, and optional scripts. Codex
  can invoke them explicitly or choose them by description, and it can read
  skills from repo, user, admin, and system locations. See
  <https://developers.openai.com/codex/skills>.
- Plugins are the installable distribution unit for reusable Codex workflows.
  They can bundle skills, apps, MCP servers, and lifecycle configuration. See
  <https://developers.openai.com/codex/plugins> and
  <https://developers.openai.com/codex/plugins/build>.
- Hooks can run deterministic scripts during the Codex lifecycle, but matching
  hooks can run concurrently, non-managed command hooks require trust review,
  and plugin hooks are off by default unless enabled. Hooks are therefore not
  the primary capture path. See <https://developers.openai.com/codex/hooks>.
- MCP connects Codex to third-party tools and context in the CLI and IDE
  extension. It is a good future interface for a shared HiveMind service, but
  local capture does not require MCP setup. See
  <https://developers.openai.com/codex/mcp>.

This repository ships `plugins/hivemind-capture`, exposed through
`.agents/plugins/marketplace.json`. The plugin bundles the
`$hivemind-capture` skill, which keeps the direct CLI as the write path and uses
the same actor-id convention as Claude: `agent:codex:<session>` and
`agent:claude:<session>`.

Install from a HiveMind checkout by starting Codex in the repository, opening
`/plugins`, choosing `HiveMind Plugins`, and installing `HiveMind Capture`.
Install from another machine by adding the repository marketplace first:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

For instruction-only use, copy
`plugins/hivemind-capture/skills/hivemind-capture` to `$HOME/.agents/skills/`
and invoke `$hivemind-capture`.

The skill is backend agnostic. Use the default local ledger with
`HIVEMIND_DIR=./hivemind`, or set `HIVEMIND_DIR`/`--hivemind-dir` to a shared
ledger path. The capture verb and query behavior stay the same.

## Reliability Tradeoffs

Direct CLI/API capture is the critical path because it is explicit, testable,
and writes to the ledger in one local process. Skills and instructions improve
discoverability, but they do not guarantee that an agent will call a tool.
Hooks are supplemental because they can be skipped, disabled, or misinstalled.
MCP is a good follow-up interface once the service boundary and auth model are
clear, but the ledger should not require MCP for local capture.
