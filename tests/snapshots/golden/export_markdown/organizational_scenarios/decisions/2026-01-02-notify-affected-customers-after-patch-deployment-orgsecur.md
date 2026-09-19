---
id: "org:security:decision:coordinated-notice"
title: "Notify affected customers after patch deployment"
status: "contested"
occurred_at: "2026-01-02T00:40:14+00:00"
topic_keys: ["org.security", "legal", "customer-comms"]
proposer: "human:legal"
source: "api"
source_ref: "security-vulnerability-triage"
event_origin: 57
supersedes: null
superseded_by: null
---

# Notify affected customers after patch deployment

Status: contested — accepted by: human:legal; rejected by: human:customer-success

## Context

Topic keys: org.security, legal, customer-comms

Hypotheses premised on:
- A feature-flagged patch can close exposure without breaking exports. (org:security:hypothesis:flagged-patch-safe): supported

## Options considered

- **org:security:option:notify-after-patch** (chosen)
- org:security:option:notify-now

## Decision

**org:security:option:notify-after-patch**

Legal and customer success need a consistent disclosure package.

## Evidence

- Customer exposure logs show two enterprise tenants may be affected. (source: security-vulnerability-triage)

## Outcome

Still holds: **no**
Reasons:
- Contested

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 57
- Source: api
- Source ref: security-vulnerability-triage
- Proposer: human:legal
- Accepted by: human:legal
- Rejected by: human:customer-success
- Ledger timestamp: 2026-01-02T00:40:14+00:00
