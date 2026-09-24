# Agent Decision Context

Status: shipped. Bead `hivemind-tenv.3`, parent epic `hivemind-tenv`
("Agent-fluent decision interaction").

`docs/AGENT_DECISION_CAPTURE.md` documents the write path: how an agent
records a new decision. This document covers the other half — how an agent
consults, verifies, and contests decisions that already exist — and is
honest about the fact that HiveMind ships **two different interaction
models** for that, not one:

1. **MCP** (`src/mcp.rs`, native stdio and `POST /mcp`) — tools like
   `get_relevant_decisions`, `get_decision_context`, `get_supersession_chain`,
   `disagree_decision`, `supersede_decision`. As of `hivemind-ot72.5`–
   `hivemind-ot72.8` (2026-09-21), `get_decision_neighborhood` (why),
   `get_decision_outcome` (verify), `disagree_decision`, and
   `supersede_decision` also accept a free-text `description` (+ optional
   `topic`) instead of an id — the same resolve-by-description primitive the
   CLI uses (see `docs/AGENT_FLUENT_QUERYING.md` §3.4). `get_decision_context`
   and `get_supersession_chain` still require `decision_id` (the latter has
   an open bead, `hivemind-ot72.9`, to add fluent resolution; the former is
   not planned to change — it is meant to be called once a decision is
   already resolved). MCP has no `--pick`/`#N` continuation (stateless per
   call): an ambiguous or not-found description comes back as a normal
   success reply with `data.outcome` set to `"ambiguous"` or `"not_found"`,
   never a JSON-RPC error, and the caller re-calls with `decision_id` once it
   has one. Pick MCP when an id is already known, the runtime prefers
   structured tool calls, or the verb is one of the still-id-only ones above.
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

Neither model is a strict subset of the other. MCP exposes a broader tool
surface (scoring, compaction, quality scanning) that the CLI-only plugin does
not wrap, and two verbs (`get_supersession_chain`, `compact-view`) that
still require an id over MCP but resolve fluently over the CLI. The
CLI-only plugin's `--pick N` / `#N` cross-invocation continuation has no MCP
equivalent (MCP's ambiguous replies still carry the full candidate list in
the same call — there is just no shorthand to re-select one in a later
call). Pick MCP when an id is already known or the runtime wants structured
tool calls; pick `hivemind-context` when the agent knows only what it is
about to change or what it wants to ask, not an id, or wants `--pick`/`#N`
continuation across calls.

The HTTP REST API sits between the two: `GET /v1/decisions/{situational,
recall,why,verify}` are fluent (id-optional, `hivemind-ot72.4`) the same way
`why`/`verify` are over MCP, but `POST /v1/decisions/{id}/disagreements` and
`/{id}/supersessions` are still `decision_id`-path-only — HTTP has no fluent
write path yet, unlike MCP. See `docs/DEPLOYMENT.md`/`docs/SELF_HOSTING.md`
for the HTTP route list.

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
  strict ambiguity gate (no write on an unresolved match);
- what an existing decision rests on, when it reads `nothing declared` —
  **ground**, resolved by description, a write verb with the same strict
  ambiguity gate, attributed to whoever adds it.

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
  `/hivemind-context:disagree`, `/hivemind-context:supersede`,
  `/hivemind-context:ground` — one slash command per CLI verb.
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

Both interaction models — MCP and the `hivemind-context` plugin's CLI
wrappers — read two independent settings from whatever environment launches
the agent; neither model sets either itself:

- **Ledger location** (SQLite mode): `./hivemind/` by default, or
  `HIVEMIND_DIR`/`--hivemind-dir`/the plugin's `hivemind_dir` option for
  another directory, including one on shared storage.
- **Backend** (SQLite vs. Postgres): unset `HIVEMIND_DATABASE_URL` uses the
  SQLite ledger above; setting it (with `HIVEMIND_TENANT` to pick the
  tenant) makes `hivemind mcp` and every `hivemind query`/`disagree`/
  `supersede` invocation the plugin shells out to connect directly to that
  Postgres backend instead — `--hivemind-dir`/`hivemind_dir` is then
  ignored. Verb names, actor format, and the ambiguity gate are unaffected
  by this choice.
