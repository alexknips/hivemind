---
id: "org:incident:decision:declare"
title: "Declare pricing-flag incident"
status: "accepted"
occurred_at: "2026-01-02T00:10:08+00:00"
topic_keys: ["org.incident", "operations"]
proposer: "agent:monitoring"
source: "api"
source_ref: "production-incident-response"
event_origin: 8
supersedes: null
superseded_by: null
---

# Declare pricing-flag incident

Status: accepted

## Context

Topic keys: org.incident, operations

Hypotheses premised on:
- The new pricing flag is causing the checkout regression. (org:incident:hypothesis:flag-regression): supported

## Options considered

- **org:incident:option:declare** (chosen)
- org:incident:option:watch

## Decision

**org:incident:option:declare**

Automated monitoring saw a sustained checkout failure spike.

## Evidence

- Checkout errors rose above the incident threshold for the new pricing flag. (source: production-incident-response)

## Outcome

Still holds: **yes**
Reasons: None recorded.

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 8
- Source: api
- Source ref: production-incident-response
- Proposer: agent:monitoring
- Accepted by: human:oncall
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:10:08+00:00
