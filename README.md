# HiveMind

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

Not in slice 1: HTTP, MCP, signing, federation, compaction, similarity search,
ranking, recommendations, or any LLM-backed query behavior. Those belong to the
later agentic layer.

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
CLI path. See `docs/ARCHITECTURE.md` for the current native-link blocker.

Additional slice-1 harnesses:

```bash
cargo test --test golden
cargo test --test golden -- --bless
cargo test --test seed -- --include-ignored
cargo test --test seed replay_smoke -- --nocapture
```

`cargo test --test seed -- --include-ignored` writes a deterministic demo ledger
under `./hivemind/` unless `HIVEMIND_SEED_DIR` points somewhere else. The replay
smoke test is warning-only for query drift and exits non-zero only for setup,
ledger, projection, or query failures.

## CLI Shape

Use `--actor` to identify the human or agent taking the action. Use
`--hivemind-dir` to choose the ledger directory.

```bash
cargo run -- --actor alice emit evidence.recorded \
  --content "SQLite WAL is sufficient for slice-1 local writes"

cargo run -- --actor alice emit hypothesis.recorded \
  --statement "Embedded storage keeps onboarding under five minutes"

cargo run -- --actor alice emit decision.proposed \
  --title "Use embedded storage for slice 1" \
  --rationale "It keeps the prototype single-process and easy to replay" \
  --topic-keys architecture,storage \
  --options sqlite,postgres \
  --chose sqlite
```

Agents can use the noninteractive capture path. It defaults the actor and
provenance to `agent:<tool>:<session>` and writes events with `source=agent`:

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --agent-tool codex \
  --agent-session "$CODEX_SESSION_ID" \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

The emit commands print the new entity id or event id. Add `--json` for a
structured output envelope.

Query commands return JSON:

```bash
cargo run -- query get_decision --id decision-001
cargo run -- query get_relevant_decisions --topic architecture
cargo run -- query get_relevant_decisions --topic architecture --status accepted
cargo run -- query get_supersession_chain --id decision-001
```

Use the persistent local Kuzu projection explicitly:

```bash
cargo run --features graph-kuzu -- --graph-backend kuzu query get_relevant_decisions \
  --topic architecture
```

Export the current projected graph as deterministic Graphviz DOT:

```bash
cargo run -- dump --format dot > graph.dot
```

## Read More

- `docs/ARCHITECTURE.md` is the concise architecture summary for reviewers.
- `docs/AGENT_DECISION_CAPTURE.md` documents the Claude/Codex capture path.
- `PLAN.md` explains the slice-1 architecture and what is intentionally deferred.
- `AGENTS.md` defines the project standards and non-goals for contributors.
- `tests/seed/README.md` documents the deterministic seed dataset, replay smoke
  test, and golden snapshot workflow.
