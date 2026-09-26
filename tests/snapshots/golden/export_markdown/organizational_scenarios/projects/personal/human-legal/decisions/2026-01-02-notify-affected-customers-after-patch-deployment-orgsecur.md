---
id: "org:security:decision:coordinated-notice"
title: "Notify affected customers after patch deployment"
status: "contested"
project: "personal:human:legal"
project_label: "legal's personal project"
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
Project: legal's personal project

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

## Rests on

- **Evidence:** Customer exposure logs show two enterprise tenants may be affected. (at capture)
- **Assumption:** A feature-flagged patch can close exposure without breaking exports. — supported (at capture)

## Evidence

- Customer exposure logs show two enterprise tenants may be affected. (source: security-vulnerability-triage)

## Outcome

Still holds: **no**
Reasons:
- Contested

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:security:option:notify-now`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:security:option:notify-now`)
- **Information** — solid
  - 1 evidence item counted: recorded before the decision or attached at capture (`org:security:evidence:customer-exposure`)
  - where it was observed is stated for 1 of 1 (`org:security:evidence:customer-exposure`)
  - 1 assumption counted: a stated premise, not yet checked (`org:security:hypothesis:flagged-patch-safe`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:security:decision:coordinated-notice`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:security:option:notify-now`)
  - no evidence that refutes something the decision rests on was on record before it
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 57
- Source: api
- Source ref: security-vulnerability-triage
- Proposer: human:legal
- Accepted by: human:legal
- Rejected by: human:customer-success
- Ledger timestamp: 2026-01-02T00:40:14+00:00
