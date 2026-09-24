---
id: "org:security:decision:severity-critical"
title: "Escalate export issue to critical severity"
status: "accepted"
occurred_at: "2026-01-02T00:40:11+00:00"
topic_keys: ["org.security", "security", "legal"]
proposer: "human:security"
source: "api"
source_ref: "security-vulnerability-triage"
event_origin: 54
supersedes: "org:security:decision:severity-low"
superseded_by: null
---

# Escalate export issue to critical severity

Status: accepted

## Context

Topic keys: org.security, security, legal

Hypotheses premised on:
- The export endpoint still requires tenant authentication. (org:security:hypothesis:auth-required): **refuted**

## Options considered

- **org:security:option:critical** (chosen)
- org:security:option:medium

## Decision

**org:security:option:critical**

Exploit reproduction refutes the low-severity assumption.

## Rests on

- **Evidence:** Customer exposure logs show two enterprise tenants may be affected. (at capture)
- **Evidence:** Manual reproduction accesses another tenant's export without authentication. (at capture)
- **Assumption:** The export endpoint still requires tenant authentication. — REFUTED (at capture)

## Evidence

- Customer exposure logs show two enterprise tenants may be affected. (source: security-vulnerability-triage)
- Manual reproduction accesses another tenant's export without authentication. (source: security-vulnerability-triage)

## Outcome

Still holds: **no**
Reasons:
- Premised on refuted hypothesis org:security:hypothesis:auth-required

Supersedes: [Classify export issue as low severity](../../agent-maintainer/decisions/2026-01-02-classify-export-issue-as-low-severity-orgsecur.md)
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 54
- Source: api
- Source ref: security-vulnerability-triage
- Proposer: human:security
- Accepted by: human:legal
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:40:11+00:00
