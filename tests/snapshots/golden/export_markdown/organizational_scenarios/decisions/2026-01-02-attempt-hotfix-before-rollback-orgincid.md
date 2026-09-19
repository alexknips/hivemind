---
id: "org:incident:decision:hotfix"
title: "Attempt hotfix before rollback"
status: "superseded"
occurred_at: "2026-01-02T00:10:10+00:00"
topic_keys: ["org.incident", "operations"]
proposer: "human:oncall"
source: "api"
source_ref: "production-incident-response"
event_origin: 10
supersedes: null
superseded_by: "org:incident:decision:rollback"
---

# Attempt hotfix before rollback

Status: superseded → [Rollback pricing flag and queue remediation](2026-01-02-rollback-pricing-flag-and-queue-remediation-orgincid.md)

## Context

Topic keys: org.incident, operations

Hypotheses premised on:
- A hotfix can land before customer impact expands. (org:incident:hypothesis:hotfix-fast-enough): **refuted**

## Options considered

- **org:incident:option:hotfix** (chosen)
- org:incident:option:rollback

## Decision

**org:incident:option:hotfix**

A targeted patch might preserve the feature launch.

## Evidence

- Enterprise customers opened priority cases while the hotfix window slipped. (source: production-incident-response)
- Checkout errors rose above the incident threshold for the new pricing flag. (source: production-incident-response)

## Outcome

Still holds: **no**
Reasons:
- Superseded by org:incident:decision:rollback (5 ledger events later)
- Premised on refuted hypothesis org:incident:hypothesis:hotfix-fast-enough
- Contested

Supersedes: None recorded.
Superseded by: [Rollback pricing flag and queue remediation](2026-01-02-rollback-pricing-flag-and-queue-remediation-orgincid.md)

Active blockers: None recorded.

## Provenance

- Event origin: 10
- Source: api
- Source ref: production-incident-response
- Proposer: human:oncall
- Accepted by: human:product-owner
- Rejected by: agent:sre
- Ledger timestamp: 2026-01-02T00:10:10+00:00
