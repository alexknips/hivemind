---
id: "org:security:decision:severity-low"
title: "Classify export issue as low severity"
status: "superseded"
occurred_at: "2026-01-02T00:40:09+00:00"
topic_keys: ["org.security", "security"]
proposer: "agent:maintainer"
source: "api"
source_ref: "security-vulnerability-triage"
event_origin: 52
supersedes: null
superseded_by: "org:security:decision:severity-critical"
---

# Classify export issue as low severity

Status: superseded → [Escalate export issue to critical severity](../../human-security/decisions/2026-01-02-escalate-export-issue-to-critical-severity-orgsecur.md)

## Context

Topic keys: org.security, security

Hypotheses premised on:
- The export endpoint still requires tenant authentication. (org:security:hypothesis:auth-required): **refuted**

## Options considered

- **org:security:option:low** (chosen)
- org:security:option:critical

## Decision

**org:security:option:low**

The first read assumed authentication was still enforced.

## Rests on

- **Evidence:** Static scanner flagged an authorization bypass in export links. (at capture)
- **Assumption:** The export endpoint still requires tenant authentication. — REFUTED (at capture)

## Evidence

- Static scanner flagged an authorization bypass in export links. (source: security-vulnerability-triage)

## Outcome

Still holds: **no**
Reasons:
- Superseded by org:security:decision:severity-critical (4 ledger events later)
- Premised on refuted hypothesis org:security:hypothesis:auth-required

Supersedes: None recorded.
Superseded by: [Escalate export issue to critical severity](../../human-security/decisions/2026-01-02-escalate-export-issue-to-critical-severity-orgsecur.md)

Active blockers: None recorded.

## Provenance

- Event origin: 52
- Source: api
- Source ref: security-vulnerability-triage
- Proposer: agent:maintainer
- Accepted by: agent:maintainer
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:40:09+00:00
