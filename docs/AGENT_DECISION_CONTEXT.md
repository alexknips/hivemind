# Agent Decision Context

Status: shipped. Bead `hivemind-tenv.3`, parent epic `hivemind-tenv`
("Agent-fluent decision interaction").

`docs/AGENT_DECISION_CAPTURE.md` documents the write path: how an agent
records a new decision. This document covers the other half — how an agent
consults, verifies, and contests decisions that already exist — and is
honest about the fact that HiveMind ships **two different interaction
models** for that, not one:

1. **MCP** (`src/mcp.rs`, native stdio and `POST /mcp`) — tools like
   `get_relevant_decisions`, `get_decision_context`,
   `get_supersession_chain`, `disagree_decision`, `supersede_decision`.
   Every one of these still takes a `decision_id` (or a `topic`) as a
   required argument. It is the right surface when an agent's tool-calling
   runtime prefers structured function calls, or when an id is already at
   hand from a prior tool result.
2. **The `hivemind-context` Claude/Codex plugin** (this document,
   `plugins/hivemind-context/`) — thin `scripts/*.sh` wrappers around the
   `hivemind-tenv.1`/`hivemind-tenv.2` CLI verbs
   (`query situational`, `query recall`, `query why`, `query verify`,
   `disagree`, `supersede`). None of them require a `decision_id` as
   primary input; free text (or a numbered `#N` handle from a prior
   ambiguous result) is the interface. It ships **no MCP server** — every
   command execs the `hivemind` binary directly, on purpose, so it stays a
   genuinely different interaction model from MCP rather than an MCP
   client wearing a CLI costume.

Neither model is a strict subset of the other today. MCP exposes a broader
tool surface (scoring, compaction, quality scanning) that the CLI-only
plugin does not wrap; the CLI-only plugin's resolve-by-description ambiguity
gate (candidates returned as a value, `--pick N` / `#N` continuation) has no
MCP equivalent yet. Pick MCP when an id is already known or the runtime
wants structured tool calls; pick `hivemind-context` when the agent knows
only what it is about to change or what it wants to ask, not an id.

## The Agent's Lens

An agent about to change code does not know a decision's id. It knows:

- what it is about to touch (files, a diff, a branch) — **situational
  first**;
- a question in its own words — **recall**;
- a decision already in front of it that it wants to know more about —
  **why**, resolved by description;
- whether an old decision it is about to depend on still holds —
  **verify**, resolved by description;
- that it disagrees, or that a decision needs replacing — **disagree** /
  **supersede**, both resolved by description, both write verbs with a
  strict ambiguity gate (no write on an unresolved match).

`hivemind-context` maps each of these onto exactly one CLI verb. See
`docs/AGENT_FLUENT_QUERYING.md` for the resolver's design (deterministic
term-match + topic + recency ranking, no LLM — AGENTS.md Principles 1/7)
and `hivemind-tenv.2`'s situational-query design for the "what should I
know before I touch this" surface.

## Claude

This repository ships a Claude Code marketplace at
`.claude-plugin/marketplace.json` listing `hivemind-context@hivemind`
alongside `hivemind-capture@hivemind`.

Install from Claude Code:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-context@hivemind
/reload-plugins
```

The plugin includes:

- `/hivemind-context:situational`, `/hivemind-context:recall`,
  `/hivemind-context:why`, `/hivemind-context:verify`,
  `/hivemind-context:disagree`, `/hivemind-context:supersede` — one slash
  command per CLI verb.
- The `hivemind-context` skill, which teaches an agent when to consult
  HiveMind before changing code, how to read an ambiguous result, and
  forbids inventing a `decision_id`.

No `mcpServers` key is declared — this plugin intentionally does not also
wire the MCP server. An agent that wants both interaction models installs
`hivemind-capture` (which does wire MCP) alongside `hivemind-context`.

## Codex

This repository ships `plugins/hivemind-context`, exposed through
`.agents/plugins/marketplace.json` next to `hivemind-capture`. Install from
a HiveMind checkout by starting Codex in the repository, opening
`/plugins`, choosing `HiveMind Plugins`, and installing `HiveMind Context`.
Install from another machine by adding the repository marketplace first:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

For instruction-only use, copy `plugins/hivemind-context/skills/hivemind-context`
to `$HOME/.agents/skills/` and invoke `$hivemind-context`.

## Backend Selection

Both interaction models honor the same ledger resolution: the project-local
`./hivemind/` directory by default, or `HIVEMIND_DIR`/`--hivemind-dir`/the
plugin's `hivemind_dir` option for a shared ledger. Verb names, actor
format, and the ambiguity gate are unaffected by backend choice.
