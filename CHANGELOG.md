# Changelog

All notable changes to HiveMind are documented here.
This project adheres to [Semantic Versioning](https://semver.org/).

## v0.1.0 — 2026-06-15 — M1: Dogfood loop ships

First milestone release. HiveMind — an event-sourced, graph-projected decision
ledger for human governance of agentic decision-making — is now usable end to
end inside its own repository.

### Added
- **Event-sourced decision ledger.** Typed events (decision, option, evidence,
  hypothesis, relation) appended to an authoritative SQLite ledger and projected
  into a graph (Decision, Actor, Evidence, Option, Hypothesis). Status
  (proposed / accepted / contested / superseded) is derived from edges, never
  stored; the ledger is the trust boundary and smart behavior stays out of the
  write path.
- **Capture surfaces.** CLI (`hivemind emit decision.capture`), an MCP server,
  and Claude Code / Codex capture plugins — all thin wrappers over the same
  internal commands, carrying actor provenance (`agent:<tool>:<session>` /
  `human:<id>`) and `source=agent|human`.
- **Deterministic query layer.** `search_decisions` with filters (topic, status,
  actor, source, time, evidence, supersession), full-text search, bounded pages,
  and explicit rank basis. Semantic/vector ranking is intentionally kept out of
  the query path (a layer-3 concern, above the ledger).
- **Dogfood loop (M1).** Concurrent multi-agent writes to a shared ledger,
  repo-local MCP config, plugin actor-prefix conventions, the `docs/DOGFOOD.md`
  operations guide, and foundational VISION / PRINCIPLES / STRATEGY decisions
  seeded into the ledger.
- **Verification recorded in the ledger itself.** The dogfood loop is marked
  operational by two distinct agents capturing real decisions, verified end to
  end — HiveMind used by the team building it.

### Notes
- Storage is local-first SQLite today. A Postgres-backed multi-tenant service is
  the M2 direction (see `docs/REMOTE_DB.md`, `docs/MULTI_TENANCY.md`).
