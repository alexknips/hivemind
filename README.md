# HiveMind

[![CI](https://github.com/alexknips/hivemind/actions/workflows/ci.yml/badge.svg)](https://github.com/alexknips/hivemind/actions/workflows/ci.yml)

HiveMind is a Rust prototype for organizational decision memory. It records
what was decided, why it was decided, who acted, what options were considered,
and what evidence or hypotheses the decision depends on.

The project is deliberately a decision graph, not a chat archive, notes app, or
task tracker. Humans and agents are both represented as actors, disagreement is
preserved as first-class state, and decision status is derived from graph
relations instead of being silently overwritten.

## Slice-1 Scope

Slice 1 proves the write and read layers:

- A typed event ledger records decision, evidence, hypothesis, option, and
  relation events.
- A projector turns ledger events into a graph view with five node types:
  `Decision`, `Actor`, `Evidence`, `Option`, and `Hypothesis`.
- Query code derives decision status, including `contested` and `superseded`,
  from explicit graph edges.
- The CLI exposes `emit`, `query`, and `dump` commands over the same command and
  query modules used by tests.

The default CLI stores events in SQLite at `./hivemind/ledger.sqlite` or the
directory passed with `--hivemind-dir`. Queries and DOT dumps replay the ledger
into an in-memory graph view by default. A Kuzu-backed `GraphView` path is
available behind the optional `graph-kuzu` feature with `--graph-backend kuzu`
or `HIVEMIND_GRAPH_BACKEND=kuzu`; it rebuilds `./hivemind/graph.kuzu` from the
SQLite ledger before query/dump reads.

Not in slice 1: HTTP, signing, federation, compaction, similarity search,
ranking, recommendations, or any LLM-backed query behavior. Those belong to the
later agentic layer. The MCP server (see below) is a layer-1/2 transport
wrapper only — no smart behavior happens behind it.

## Install

Install the default CLI directly from Git:

```bash
cargo install --git https://github.com/alexknips/hivemind --locked hivemind
```

Tagged releases publish prebuilt tarballs for Linux and macOS on x86_64 and
ARM64. The installer selects the matching asset and verifies its SHA-256
checksum before copying `hivemind` into `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/alexknips/hivemind/master/scripts/install.sh | sh
```

Set `HIVEMIND_VERSION=v0.1.0` to install a specific release tag or
`HIVEMIND_INSTALL_DIR=/usr/local/bin` to choose another destination.

Verify the installed binary:

```bash
hivemind --version
hivemind --help
```

The release asset names are:

| Platform | Asset |
| --- | --- |
| Linux x86_64 | `hivemind-linux-x86_64.tar.gz` |
| Linux ARM64 | `hivemind-linux-arm64.tar.gz` |
| macOS x86_64 | `hivemind-macos-x86_64.tar.gz` |
| macOS ARM64 | `hivemind-macos-arm64.tar.gz` |

Kuzu support and the terminal UI are optional and not part of the default
binary:

```bash
cargo install --git https://github.com/alexknips/hivemind --locked --features graph-kuzu hivemind
cargo install --git https://github.com/alexknips/hivemind --locked --features tui hivemind
```

The crate is prepared for packaging but is not published to crates.io in this
slice; `Cargo.toml` keeps `publish = false` until the project explicitly
reserves the name and chooses a redistributable crate license.

## Build And Test

```bash
cargo build
cargo test
```

Kuzu support is optional and may compile bundled native C++ code:

```bash
cargo test --features graph-kuzu kuzu -- --nocapture
```

Only run the Kuzu feature tests when changing the Kuzu adapter or explicit Kuzu
CLI path. See `docs/ARCHITECTURE.md` for native dependency notes.

Additional slice-1 harnesses:

```bash
cargo test --test golden
cargo test --test golden -- --bless
cargo test --test local_capture_demo -- --nocapture
cargo test --test slack_app -- --nocapture
cargo test --test seed -- --include-ignored
cargo test --test seed replay_smoke -- --nocapture
```

`cargo test --test local_capture_demo -- --nocapture` demonstrates the local
Slack-style plus agent capture prototype against a temp ledger. It ingests a
fake Slack thread, records Codex and Claude decisions through `decision.capture`,
and verifies topic/status queries expose distinct Slack and agent provenance.

`cargo test --test slack_app -- --nocapture` exercises the real local-first
Slack app shell: manifest generation, workspace install state, queued thread
capture, queue drain into the HiveMind ledger, `/hivemind query`, and
`/hivemind show` responses with source citations.

`cargo test --test seed -- --include-ignored` writes a deterministic demo ledger
under `./hivemind/` unless `HIVEMIND_SEED_DIR` points somewhere else. The replay
smoke test is warning-only for query drift and exits non-zero only for setup,
ledger, projection, or query failures.

## CLI Shape

Use `--actor` to identify the human or agent taking the action. Use
`--hivemind-dir` to choose the ledger directory.

```bash
hivemind --actor alice emit evidence.recorded \
  --content "SQLite WAL is sufficient for slice-1 local writes"

hivemind --actor alice emit hypothesis.recorded \
  --statement "Embedded storage keeps onboarding under five minutes"

hivemind --actor alice emit decision.proposed \
  --title "Use embedded storage for slice 1" \
  --rationale "It keeps the prototype single-process and easy to replay" \
  --topic-keys architecture,storage \
  --options sqlite,postgres \
  --chose sqlite
```

Agents can use the noninteractive capture path. It defaults the actor and
provenance to `agent:<tool>:<session>` and writes events with `source=agent`:

```bash
hivemind --hivemind-dir ./hivemind/ emit decision.capture \
  --agent-tool codex \
  --agent-session "$CODEX_SESSION_ID" \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

### Codex Capture Plugin

The repository ships a Codex plugin marketplace at
`.agents/plugins/marketplace.json`. The `hivemind-capture` plugin packages a
Codex skill that tells agents when to preserve a decision and how to call the
same `decision.capture` CLI shown above. The skill keeps direct CLI capture as
the critical path; hooks and MCP are supplemental integration surfaces.

From a local HiveMind checkout, start Codex in this repository, open `/plugins`,
choose `HiveMind Plugins`, and install `HiveMind Capture`. From another
checkout or machine, add this repository as a marketplace first:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

Users who only want the instruction bundle can copy
`plugins/hivemind-capture/skills/hivemind-capture` into `$HOME/.agents/skills/`
and invoke `$hivemind-capture` directly. Local capture defaults to
`./hivemind/`; set `HIVEMIND_DIR` or pass `--hivemind-dir` to point Codex at a
shared ledger.

### MCP Server

Any MCP-aware client (Claude Desktop, Claude Code, Cursor, Codex with MCP
support, etc.) can capture and query decisions through the bundled MCP stdio
server. The server is a thin transport: capture tools call into the same
`Commands` API the CLI uses, query tools call into the same `queries` API.
Smart behavior stays out — see the slice scope above.

```bash
hivemind --hivemind-dir ./hivemind/ mcp
```

Once installed, point your MCP client at the same binary. Example config for
Claude Desktop (`~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS):

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "hivemind",
      "args": ["mcp"],
      "env": { "HIVEMIND_DIR": "/absolute/path/to/your/hivemind/dir" }
    }
  }
}
```

Cursor uses the same shape under `mcp.servers`. The server exposes seven tools:

| Tool | Layer | Wraps |
| --- | --- | --- |
| `capture_decision` | write | `emit decision.proposed` |
| `capture_evidence` | write | `emit evidence.recorded` |
| `capture_hypothesis` | write | `emit hypothesis.recorded` |
| `get_decision` | read | `query get_decision` |
| `get_relevant_decisions` | read | `query get_relevant_decisions` |
| `get_supersession_chain` | read | `query get_supersession_chain` |
| `dump_graph` | read | `dump --format dot` |

Every capture requires an explicit `actor_id`. Prefix it with the originating
tool, e.g. `agent:claude:<session>`, so provenance stays readable. The server
records `source=agent` plus a per-session `source_ref` for every write; pass
`--session-id` to override the generated default. Anonymous writes are
rejected because the underlying `Commands` API requires a non-empty actor.

`HIVEMIND_DIR` selects the ledger directory if `--hivemind-dir` is not set.
`HIVEMIND_GRAPH_BACKEND` is honored the same way the CLI honors it, so a
shared `kuzu` projection works under MCP when the `graph-kuzu` feature is
compiled in.

Local markdown or text decision notes can be imported without network access.
Only explicit `Decision:` blocks are imported, and re-importing identical input
is reported as a no-op:

```bash
hivemind --actor alice --json import documents --file ./notes/decision.md
hivemind --actor alice import documents ./notes/
```

The emit commands print the new entity id or event id. Add `--json` for a
structured output envelope.

Query commands return JSON:

```bash
hivemind query get_decision --id decision-001
hivemind query get_relevant_decisions --topic architecture
hivemind query get_relevant_decisions --topic architecture --status accepted
hivemind query get_supersession_chain --id decision-001
```

With a binary installed using `--features graph-kuzu`, use the persistent local
Kuzu projection explicitly:

```bash
hivemind --graph-backend kuzu query get_relevant_decisions \
  --topic architecture
```

Export the current projected graph as deterministic Graphviz DOT:

```bash
hivemind dump --format dot > graph.dot
```

With a binary installed using `--features tui`, run the read-only decision
search TUI:

```bash
hivemind tui --q queue --topic architecture \
  --status accepted --dot-output focused-neighborhood.dot
```

The TUI uses the same query APIs as the CLI, supports keyboard search and graph
navigation, and exports the focused one-hop neighborhood as DOT with `x`.

## Read More

- `docs/ARCHITECTURE.md` is the concise architecture summary for reviewers.
- `docs/AGENT_DECISION_CAPTURE.md` documents the Claude/Codex capture path.
- `docs/LOCAL_CAPTURE_DEMO.md` documents the local Slack plus agent capture demo.
- `docs/SLACK_APP.md` documents the local-first Slack app install, queue, and
  query surface.
- `docs/TEXT_IMPORT_AND_DIFF_SEMANTICS.md` defines local document import and
  temporal decision diff semantics for Milestone 2.
- `docs/M2_WEEKLY_DIFF_DEMO.md` walks through the Milestone 2 end-to-end flow:
  import documents locally, then ask which decisions were added since last week.
- `PLAN.md` explains the slice-1 architecture and what is intentionally deferred.
- `AGENTS.md` defines the project standards and non-goals for contributors.
- `tests/seed/README.md` documents the deterministic seed dataset, replay smoke
  test, and golden snapshot workflow.
