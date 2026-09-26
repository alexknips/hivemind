---
id: "org:incident:decision:rollback"
title: "Rollback pricing flag and queue remediation"
status: "accepted"
project: "personal:agent:sre"
project_label: "sre agents' personal project"
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
Project: sre agents' personal project

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

Supersedes: [Attempt hotfix before rollback](../../human-oncall/decisions/2026-01-02-attempt-hotfix-before-rollback-orgincid.md)
Superseded by: None recorded.

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:incident:option:regional-patch`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:incident:option:regional-patch`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:incident:evidence:customer-impact`, `org:incident:evidence:rollback-health`)
  - where it was observed is stated for 2 of 2 (`org:incident:evidence:customer-impact`, `org:incident:evidence:rollback-health`)
  - 2 assumptions counted: a stated premise, not yet checked (`org:incident:hypothesis:flag-regression`, `org:incident:hypothesis:hotfix-fast-enough`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:incident:decision:rollback`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:incident:option:regional-patch`)
  - 1 evidence item on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it) (`org:incident:evidence:customer-impact`, `org:incident:hypothesis:hotfix-fast-enough`)
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 13
- Source: api
- Source ref: production-incident-response
- Proposer: agent:sre
- Accepted by: human:oncall
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:10:13+00:00
