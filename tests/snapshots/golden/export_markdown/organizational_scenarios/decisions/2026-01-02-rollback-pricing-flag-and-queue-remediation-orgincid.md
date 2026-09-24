---
id: "org:incident:decision:rollback"
title: "Rollback pricing flag and queue remediation"
status: "accepted"
occurred_at: "2026-01-02T00:10:13+00:00"
topic_keys: ["org.incident", "operations", "customer-comms"]
proposer: "agent:sre"
source: "api"
source_ref: "production-incident-response"
event_origin: 13
supersedes: "org:incident:decision:hotfix"
superseded_by: null
---

# Rollback pricing flag and queue remediation

Status: accepted

## Context

Topic keys: org.incident, operations, customer-comms

Hypotheses premised on:
- The new pricing flag is causing the checkout regression. (org:incident:hypothesis:flag-regression): supported
- A hotfix can land before customer impact expands. (org:incident:hypothesis:hotfix-fast-enough): **refuted**

## Options considered

- **org:incident:option:disable-flag** (chosen)
- org:incident:option:regional-patch

## Decision

**org:incident:option:disable-flag**

Rollback evidence resolves the immediate risk and preserves remediation context.

## Rests on

- **Evidence:** Enterprise customers opened priority cases while the hotfix window slipped. (at capture)
- **Evidence:** Canary rollback restored checkout success rate in the affected region. (at capture)
- **Assumption:** The new pricing flag is causing the checkout regression. — supported (at capture)
- **Assumption:** A hotfix can land before customer impact expands. — REFUTED (at capture)

## Evidence

- Enterprise customers opened priority cases while the hotfix window slipped. (source: production-incident-response)
- Canary rollback restored checkout success rate in the affected region. (source: production-incident-response)

## Outcome

Still holds: **no**
Reasons:
- Premised on refuted hypothesis org:incident:hypothesis:hotfix-fast-enough

Supersedes: [Attempt hotfix before rollback](2026-01-02-attempt-hotfix-before-rollback-orgincid.md)
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 13
- Source: api
- Source ref: production-incident-response
- Proposer: agent:sre
- Accepted by: human:oncall
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:10:13+00:00
