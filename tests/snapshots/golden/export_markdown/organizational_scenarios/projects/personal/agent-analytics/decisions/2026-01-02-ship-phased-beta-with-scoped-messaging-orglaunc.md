---
id: "org:launch:decision:phased-rollout"
title: "Ship phased beta with scoped messaging"
status: "accepted"
project: "personal:agent:analytics"
project_label: "analytics agents' personal project"
occurred_at: "2026-01-02T00:30:13+00:00"
topic_keys: ["org.launch", "product", "support"]
proposer: "agent:analytics"
source: "api"
source_ref: "product-launch-readiness"
event_origin: 42
supersedes: null
superseded_by: null
---

# Ship phased beta with scoped messaging

Status: accepted
Project: analytics agents' personal project

## Context

Topic keys: org.launch, product, support

Hypotheses premised on:
- A phased beta can capture value while containing support and compliance risk. (org:launch:hypothesis:phased-rollout): supported

## Options considered

- **org:launch:option:phased-beta** (chosen)
- org:launch:option:full-delay

## Decision

**org:launch:option:phased-beta**

Cutting bulk import and adding the caveat preserves value without hiding risk.

## Rests on

- **Evidence:** Launch copy needs a compliance caveat before broad availability. (at capture)
- **Evidence:** Beta cohorts complete activation when the risky bulk import is hidden. (at capture)
- **Assumption:** A phased beta can capture value while containing support and compliance risk. — supported (at capture)

## Evidence

- Launch copy needs a compliance caveat before broad availability. (source: product-launch-readiness)
- Beta cohorts complete activation when the risky bulk import is hidden. (source: product-launch-readiness)

## Outcome

Still holds: **yes**
Reasons: None recorded.

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:launch:option:full-delay`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:launch:option:full-delay`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:launch:evidence:legal-caveat`, `org:launch:evidence:user-research`)
  - where it was observed is stated for 2 of 2 (`org:launch:evidence:legal-caveat`, `org:launch:evidence:user-research`)
  - 1 assumption counted: a stated premise, not yet checked (`org:launch:hypothesis:phased-rollout`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:launch:decision:phased-rollout`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:launch:option:full-delay`)
  - no evidence that refutes something the decision rests on was on record before it
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 42
- Source: api
- Source ref: product-launch-readiness
- Proposer: agent:analytics
- Accepted by: human:support
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:30:13+00:00
