---
id: "org:incident:decision:hotfix"
title: "Attempt hotfix before rollback"
status: "superseded"
project: "personal:human:oncall"
project_label: "oncall's personal project"
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

Status: superseded → [Rollback pricing flag and queue remediation](../../agent-sre/decisions/2026-01-02-rollback-pricing-flag-and-queue-remediation-orgincid.md)
Project: oncall's personal project

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

## Rests on

- **Evidence:** Enterprise customers opened priority cases while the hotfix window slipped. (at capture)
- **Evidence:** Checkout errors rose above the incident threshold for the new pricing flag. (at capture)
- **Assumption:** A hotfix can land before customer impact expands. — REFUTED (at capture)

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
Superseded by: [Rollback pricing flag and queue remediation](../../agent-sre/decisions/2026-01-02-rollback-pricing-flag-and-queue-remediation-orgincid.md)

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:incident:option:rollback`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:incident:option:rollback`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:incident:evidence:customer-impact`, `org:incident:evidence:error-spike`)
  - where it was observed is stated for 2 of 2 (`org:incident:evidence:customer-impact`, `org:incident:evidence:error-spike`)
  - 1 assumption counted: a stated premise, not yet checked (`org:incident:hypothesis:hotfix-fast-enough`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:incident:decision:hotfix`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:incident:option:rollback`)
  - 1 evidence item on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it) (`org:incident:evidence:customer-impact`, `org:incident:hypothesis:hotfix-fast-enough`)
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 10
- Source: api
- Source ref: production-incident-response
- Proposer: human:oncall
- Accepted by: human:product-owner
- Rejected by: agent:sre
- Ledger timestamp: 2026-01-02T00:10:10+00:00
