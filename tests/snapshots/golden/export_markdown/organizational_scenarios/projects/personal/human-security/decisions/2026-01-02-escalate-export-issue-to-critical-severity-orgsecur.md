---
id: "org:security:decision:severity-critical"
title: "Escalate export issue to critical severity"
status: "accepted"
project: "personal:human:security"
project_label: "security's personal project"
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
Project: security's personal project

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

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:security:option:medium`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:security:option:medium`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:security:evidence:customer-exposure`, `org:security:evidence:exploit-repro`)
  - where it was observed is stated for 2 of 2 (`org:security:evidence:customer-exposure`, `org:security:evidence:exploit-repro`)
  - 1 assumption counted: a stated premise, not yet checked (`org:security:hypothesis:auth-required`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:security:decision:severity-critical`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:security:option:medium`)
  - 1 evidence item on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it) (`org:security:evidence:exploit-repro`, `org:security:hypothesis:auth-required`)
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 54
- Source: api
- Source ref: security-vulnerability-triage
- Proposer: human:security
- Accepted by: human:legal
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:40:11+00:00
