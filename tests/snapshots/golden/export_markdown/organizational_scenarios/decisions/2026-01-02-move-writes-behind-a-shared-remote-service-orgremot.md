---
id: "org:remote-db:decision:service-owned-writes"
title: "Move writes behind a shared remote service"
status: "accepted"
occurred_at: "2026-01-02T00:20:10+00:00"
topic_keys: ["org.remote-db", "architecture", "security"]
proposer: "human:architect"
source: "api"
source_ref: "remote-db-architecture-choice"
event_origin: 25
supersedes: "org:remote-db:decision:embedded-prototype"
superseded_by: null
---

# Move writes behind a shared remote service

Status: accepted

## Context

Topic keys: org.remote-db, architecture, security

Hypotheses premised on:
- A service-owned write path can meet security and operational constraints. (org:remote-db:hypothesis:service-owned-writes): supported

## Options considered

- **org:remote-db:option:service-owned-writes** (chosen)
- org:remote-db:option:direct-clients

## Decision

**org:remote-db:option:service-owned-writes**

Shared state and security review both require service ownership.

## Evidence

- Security review rejects direct database writes from every client. (source: remote-db-architecture-choice)
- Non-developer users need shared state across support shifts. (source: remote-db-architecture-choice)

## Outcome

Still holds: **yes**
Reasons: None recorded.

Supersedes: [Keep prototype storage embedded](2026-01-02-keep-prototype-storage-embedded-orgremot.md)
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 25
- Source: api
- Source ref: remote-db-architecture-choice
- Proposer: human:architect
- Accepted by: human:security-reviewer
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:20:10+00:00
