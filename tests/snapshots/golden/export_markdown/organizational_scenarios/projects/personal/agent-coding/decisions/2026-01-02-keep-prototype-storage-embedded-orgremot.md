---
id: "org:remote-db:decision:embedded-prototype"
title: "Keep prototype storage embedded"
status: "superseded"
project: "personal:agent:coding"
project_label: "coding agents' personal project"
occurred_at: "2026-01-02T00:20:08+00:00"
topic_keys: ["org.remote-db", "architecture"]
proposer: "agent:coding"
source: "api"
source_ref: "remote-db-architecture-choice"
event_origin: 23
supersedes: null
superseded_by: "org:remote-db:decision:service-owned-writes"
---

# Keep prototype storage embedded

Status: superseded → [Move writes behind a shared remote service](../../human-architect/decisions/2026-01-02-move-writes-behind-a-shared-remote-service-orgremot.md)
Project: coding agents' personal project

## Context

Topic keys: org.remote-db, architecture

Hypotheses premised on:
- A local embedded store is enough for the first shared pilot. (org:remote-db:hypothesis:embedded-sufficient): **refuted**

## Options considered

- **org:remote-db:option:embedded** (chosen)
- org:remote-db:option:remote-service

## Decision

**org:remote-db:option:embedded**

The local store is simplest while the data model is still moving.

## Rests on

- **Evidence:** Non-developer users need shared state across support shifts. (at capture)
- **Assumption:** A local embedded store is enough for the first shared pilot. — REFUTED (at capture)

## Evidence

- Non-developer users need shared state across support shifts. (source: remote-db-architecture-choice)

## Outcome

Still holds: **no**
Reasons:
- Superseded by org:remote-db:decision:service-owned-writes (4 ledger events later)
- Premised on refuted hypothesis org:remote-db:hypothesis:embedded-sufficient

Supersedes: None recorded.
Superseded by: [Move writes behind a shared remote service](../../human-architect/decisions/2026-01-02-move-writes-behind-a-shared-remote-service-orgremot.md)

Active blockers: None recorded.

## Provenance

- Event origin: 23
- Source: api
- Source ref: remote-db-architecture-choice
- Proposer: agent:coding
- Accepted by: agent:coding
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:20:08+00:00
