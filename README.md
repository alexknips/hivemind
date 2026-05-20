# HiveMind

[![CI](https://github.com/alexknips/hivemind/actions/workflows/ci.yml/badge.svg)](https://github.com/alexknips/hivemind/actions/workflows/ci.yml)

HiveMind is the substrate for human governance of agentic decision-making.
It records what was decided, why it was decided, who acted (human or agent),
what options were considered, and what evidence or hypotheses the decision
depends on — so humans can see, query, and contest every decision their
agents make, instead of relinquishing oversight as agents take on more work.

The project is deliberately a decision graph, not a chat archive, notes app,
or task tracker. Humans and agents are both represented as actors,
disagreement is preserved as first-class state, and decision status is
derived from graph relations instead of being silently overwritten.

See [VISION.md](VISION.md) for the full positioning,
[PRINCIPLES.md](PRINCIPLES.md) for the load-bearing constraints, and
[STRATEGY.md](STRATEGY.md) for the active investment fronts.

## Quickstart

The fastest first run uses an isolated temporary ledger, captures one decision,
and immediately queries it back:

```bash
hivemind --actor human:alice quickstart
```

From a fresh clone before installing the binary:

```bash
cargo run -- --actor human:alice quickstart
```

The `--actor` value is mandatory provenance. Use a stable human id such as
`human:alice` or an agent id such as `agent:codex:<session>`. If `--actor` is
omitted, the CLI falls back to `HIVEMIND_ACTOR`, then `USER`.

To capture and query the first real decision in the current directory:

```bash
hivemind --actor human:alice --hivemind-dir ./hivemind emit decision.proposed \
  --title "Use HiveMind for architecture decisions" \
  --rationale "We need a durable record of what was decided, why, and by whom" \
  --topic-keys onboarding,architecture \
  --options hivemind,notes \
  --chose hivemind

hivemind --hivemind-dir ./hivemind query search_decisions \
  --topic onboarding \
  --limit 5
```

## What HiveMind Does Today

- **Write.** A typed event ledger records decision, evidence, hypothesis,
  option, and relation events. The `commands` module validates invariants and
  appends to the ledger; both CLI and MCP write through the same internal
  functions.
- **Project.** Ledger events are projected into a graph view with five node
  types: `Decision`, `Actor`, `Evidence`, `Option`, and `Hypothesis`. Status
  (`proposed`, `accepted`, `contested`, `superseded`) is derived from explicit
  graph edges, not stored.
- **Read.** Query code derives status, walks supersession chains, and surfaces
  contested decisions through deterministic graph reads. Search includes
  topic/status filters and FTS-backed full-text search; `recent`, `disagree`,
  and `supersede` are first-class verbs on both CLI and MCP.
- **Capture from agents.** Installable plugins for Claude Code and Codex,
  plus the bundled MCP stdio server, let agents capture decisions in-flow
  with auto-populated actor and provenance defaults.

The default CLI stores events in SQLite at `./hivemind/ledger.sqlite` or the
directory passed with `--hivemind-dir`. Queries and DOT dumps replay the
ledger into an in-memory graph view by default. A Kuzu-backed projection is
available behind the optional `graph-kuzu` feature.

SQLite is a deliberate short-term choice; the long-term storage backend is
open. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the current state
model and what is marked temporary.

Out of scope today: compactification, similarity search, semantic ranking,
LLM-backed query, multi-tenant shared backend, federation. These belong to
the agentic layer above the ledger and to the shared-backend front —
[`STRATEGY.md`](STRATEGY.md) tracks where each one stands.

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

The crate is prepared for packaging but is not yet published to crates.io;
`Cargo.toml` keeps `publish = false` until the project explicitly reserves
the name and chooses a redistributable crate license.

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

Additional test harnesses:

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

Use `--actor` to override the human or agent taking the action. Bare terminal
writes default to `human:<git config user.email>` with `source=human`. Use
`--hivemind-dir` to choose the ledger directory.

```bash
hivemind --actor alice emit evidence.recorded \
  --content "SQLite WAL is sufficient for current local writes"

hivemind --actor alice emit hypothesis.recorded \
  --statement "Embedded storage keeps onboarding under five minutes"

hivemind emit decision.proposed \
  --title "Use embedded storage for the local prototype" \
  --rationale "It keeps the local install single-process and easy to replay" \
  --topic-keys architecture,storage \
  --options sqlite,postgres \
  --chose sqlite
```

Agents can use the noninteractive capture path. It defaults the actor and
provenance to `agent:<tool>:<session>` and writes events with `source=agent`:

```bash
hivemind --hivemind-dir ./hivemind/ emit decision.capture \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

### Agent Capture Plugins

The repository ships installable capture bundles for Claude Code and Codex. Both
packages teach agents when to preserve a decision and how to call the same
`decision.capture` CLI shown above. The direct CLI remains the critical write
path; MCP is bundled as a supplemental tool transport.

Claude Code uses the marketplace at `.claude-plugin/marketplace.json` and the
plugin in `plugins/hivemind-capture`. Install it from Claude Code:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

In this repository, `.claude/settings.json` advertises the marketplace and
enables `hivemind-capture@hivemind` so trusted Claude Code sessions are prompted
to install it. The plugin provides `/hivemind-capture:capture-decision`,
`/hivemind-capture:query-decisions`, and a `hivemind` MCP server wired to
`hivemind mcp`. See `plugins/hivemind-capture/README.md` for uninstall and
verification steps.

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
Smart behavior stays out — see the "What HiveMind Does Today" section above.

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
is reported as a no-op. Changed same-id re-imports report conflicts by default;
reviewers can resolve them explicitly with `--on-conflict keep_existing`,
`supersede`, `contest`, or `add_context`:

```bash
hivemind --actor alice --json import documents --file ./notes/decision.md
hivemind --actor alice --json import documents --on-conflict supersede ./notes/
hivemind --actor alice import documents ./notes/
```

PDFs and OCR-backed text are prepared before import. The preparation command
does not write ledger events; it materializes reviewable text with source/page
metadata, then the reviewed output can be imported through the same document
importer:

```bash
hivemind --json import prepare-documents ./sources/decision.pdf \
  --output-dir ./prepared
hivemind --actor alice --json import documents ./prepared
```

OCR text files such as `scan.ocr.txt` are marked `review_required`, and that
uncertainty is preserved in the final imported source reference.

LLM-assisted document extraction stays in the layer-3 suggestion path. It can
read an unstructured local document and an external extractor response without
writing ledger events:

```bash
hivemind --json suggest document-candidates \
  --file ./notes/memo.txt \
  --llm-response ./llm-candidates.json > candidates.json
```

After review, materialize selected candidates into ordinary `Decision:` blocks,
then import that reviewed file explicitly:

```bash
hivemind --actor alice --json suggest materialize-document-candidates \
  --input candidates.json \
  --candidate-id candidate:document:abc123 \
  --output reviewed.md
hivemind --actor alice --json import documents --file reviewed.md
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

Level-1 guidance (read these first):

- [`VISION.md`](VISION.md) — why HiveMind exists, who it's for, and the bet on
  human governance of agentic decision-making.
- [`PRINCIPLES.md`](PRINCIPLES.md) — the constraints HiveMind cannot trade
  away.
- [`STRATEGY.md`](STRATEGY.md) — active investment fronts; the filter a bead
  is judged against.

Architecture and surfaces:

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — three-layer architecture,
  surface uniformity, state model, storage, and other architectural decisions
  (some marked temporary).
- [`docs/MULTI_TENANCY.md`](docs/MULTI_TENANCY.md) — multi-tenant model for
  the MCP/API surface.
- [`docs/SEARCH_DESIGN.md`](docs/SEARCH_DESIGN.md) — storage-agnostic search
  surface design.
- [`docs/REMOTE_DB.md`](docs/REMOTE_DB.md) — long-term shared-backend
  direction.

Per-feature docs:

- [`docs/AGENT_DECISION_CAPTURE.md`](docs/AGENT_DECISION_CAPTURE.md) — the
  Claude/Codex capture path.
- [`docs/LOCAL_CAPTURE_DEMO.md`](docs/LOCAL_CAPTURE_DEMO.md) — local Slack plus
  agent capture demo.
- [`docs/SLACK_APP.md`](docs/SLACK_APP.md) — local-first Slack app install,
  queue, and query surface.
- [`docs/TEXT_IMPORT_AND_DIFF_SEMANTICS.md`](docs/TEXT_IMPORT_AND_DIFF_SEMANTICS.md)
  — local document import and temporal decision diff semantics.
- [`docs/M2_WEEKLY_DIFF_DEMO.md`](docs/M2_WEEKLY_DIFF_DEMO.md) — import
  documents locally, then ask which decisions were added since last week.
- [`AGENTS.md`](AGENTS.md) — project standards and non-goals for
  contributors.
- [`tests/seed/README.md`](tests/seed/README.md) — deterministic seed
  dataset, replay smoke test, and golden snapshot workflow.
