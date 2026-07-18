# Agent Decision Capture

Status: shipped. Originally landed under bead `hivemind-claude-codex-agent-capture-tco7`.
Extended in M2 with HTTP API transcript capture (`/v1/ingest`) and a
server-side classifier. See the *HTTP API Capture* section below.

HiveMind exposes a noninteractive CLI path for Claude, Codex, and similar coding
agents to record a decision directly into the local ledger:

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options direct-cli,mcp,hook \
  --chose direct-cli
```

The command writes canonical ledger events. The decision proposal and its
fan-out relation events carry:

- `source=agent`
- `actor_id=agent:<tool>:<session>` unless `--actor-id` is provided. Tool and
  session default from `HIVEMIND_AGENT_TOOL`, Claude session variables, or
  Codex session variables; `--agent-tool` and `--agent-session` are explicit
  overrides.
- `source_ref=<actor_id>` unless `--source-ref` is provided

Use `--evidence` and `--hypotheses` with existing evidence and hypothesis ids
when the decision depends on already captured context.

## Claude

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
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

The default backend is the project-local `./hivemind/` directory. The bundled
MCP descriptor pins that location for agents launched from this checkout. For
slash-command captures that need another ledger, set the plugin option
`hivemind_dir`, export `HIVEMIND_DIR`, or pass `--hivemind-dir` to the command.

## Codex

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
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

## HTTP API Capture

Status: shipped in M2 as a second, parallel capture path alongside the
explicit CLI.

The HiveMind HTTP API exposes `POST /v1/ingest` to accept batches of agent
transcript turns. Two Python clients ship in `capture/`:

**Hook shipper** (`capture/hook_ship.py`): invoked by Claude Code
`PostToolUse` and `Stop` hooks. Reads the session JSONL transcript at the
cursor, ships new turns to `/v1/ingest`, and exits 0. Never blocks or raises
— hook failures must not affect the agent.

Example `.claude/settings.json` configuration:

```json
{
  "hooks": {
    "PostToolUse": [{"matcher": "", "hooks": [{"type": "command", "command": "python3 /path/to/capture/hook_ship.py"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "python3 /path/to/capture/hook_ship.py"}]}]
  }
}
```

**Sidecar daemon** (`capture/sidecar.py`): polls `~/.claude/projects/` for
JSONL mtime changes and ships new turns. Intended for hook-less harnesses
(Codex) and as a durability backup. Run as a background process:

```bash
python3 capture/sidecar.py &
```

Both clients configure the server URL and auth token via environment:

```
HIVEMIND_API_URL    Base URL of the HiveMind server (default: http://localhost:8080)
HIVEMIND_API_KEY    Bearer token hm_sk_live_... (optional in dev mode)
HIVEMIND_AGENT_TOOL Override agent_tool field (default: claude)
```

**Server-side classifier**: the server runs an optional background Layer-3
worker (`src/classifier.rs`) that reads `ingest.batch_received` events and
annotates them with `ingest.batch_classified` events via Haiku 4.5. The
worker exits immediately when `ANTHROPIC_API_KEY` is absent — the rest of
the system stays correct without it. See
[`CAPTURE_CLASSIFIER.md`](CAPTURE_CLASSIFIER.md) for the classifier design.

## Keyless classification (Worker A)

`ANTHROPIC_API_KEY` is optional. When it is absent, the server-side background
classifier (Worker B) does not start — the ingest path, ledger, and all queries
remain fully functional.

The `hivemind-capture` plugin ships **Worker A**: a
`/hivemind-capture:classify-queue` slash command that drains the same pending
classification queue using the agent's subscription seat. No API key is needed
on the server.

Install the plugin and run after a session:

```text
/hivemind-capture:classify-queue
```

Pass `--limit N` to cap the number of batches per run (default 20). The queue
persists across runs; large backlogs drain across multiple invocations.

Both Worker A and Worker B write identical `IngestBatchClassified` events to the
same queue. Concurrent classification is idempotent — last writer wins per batch.
Setting `ANTHROPIC_API_KEY` later starts Worker B automatically; Worker A remains
available for on-demand draining.

See [`docs/KEYLESS_CAPTURE.md`](KEYLESS_CAPTURE.md) for a zero-to-first-decision
walkthrough that requires no API key.

## Reliability Tradeoffs

Direct CLI capture is the explicit, testable, write-once path for named
decisions. HTTP API ingest (`/v1/ingest`) is the passive background path for
capturing activity transcripts. Skills and instructions improve discoverability,
but they do not guarantee that an agent will call a tool. Hooks are supplemental
because they can be skipped, disabled, or misinstalled. The sidecar daemon is a
durability backstop for hook-less harnesses.

MCP via the TypeScript gateway (`clients/mcp-gateway/`) is a read path over
the HTTP API; see [`MCP_SERVICE_SPLIT.md`](MCP_SERVICE_SPLIT.md).
