---
id: "org:remote-db:decision:postgres-pilot"
title: "Use hosted Postgres for the short-run backend"
status: "accepted"
occurred_at: "2026-01-02T00:20:13+00:00"
topic_keys: ["org.remote-db", "architecture", "migration"]
proposer: "agent:ops"
source: "api"
source_ref: "remote-db-architecture-choice"
event_origin: 28
supersedes: null
superseded_by: null
---

# Use hosted Postgres for the short-run backend

Status: accepted

## Context

Topic keys: org.remote-db, architecture, migration

Hypotheses premised on:
- A service-owned write path can meet security and operational constraints. (org:remote-db:hypothesis:service-owned-writes): supported

## Options considered

- **org:remote-db:option:postgres** (chosen)
- org:remote-db:option:mysql

## Decision

**org:remote-db:option:postgres**

It satisfies the shared-service migration plan without introducing graph storage early.

## Evidence

- Hosted Postgres meets the latency and backup constraints for the pilot. (source: remote-db-architecture-choice)

## Outcome

Still holds: **yes**
Reasons: None recorded.

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 28
- Source: api
- Source ref: remote-db-architecture-choice
- Proposer: agent:ops
- Accepted by: human:architect
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:20:13+00:00
