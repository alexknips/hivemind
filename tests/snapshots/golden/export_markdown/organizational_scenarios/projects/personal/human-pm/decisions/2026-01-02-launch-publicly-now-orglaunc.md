---
id: "org:launch:decision:launch-now"
title: "Launch publicly now"
status: "rejected"
project: "personal:human:pm"
project_label: "pm's personal project"
occurred_at: "2026-01-02T00:30:11+00:00"
topic_keys: ["org.launch", "product"]
proposer: "human:pm"
source: "api"
source_ref: "product-launch-readiness"
event_origin: 40
supersedes: null
superseded_by: null
---

# Launch publicly now

Status: rejected
Project: pm's personal project

## Context

Topic keys: org.launch, product

Hypotheses premised on:
- The full public launch is ready this week. (org:launch:hypothesis:launch-ready): **refuted**

## Options considered

- **org:launch:option:public-now** (chosen)
- org:launch:option:delay

## Decision

**org:launch:option:public-now**

The launch window is available but quality and caveat evidence are still unresolved.

## Rests on

- **Evidence:** Launch copy needs a compliance caveat before broad availability. (at capture)
- **Evidence:** Regression automation still fails checkout and billing smoke tests. (at capture)
- **Assumption:** The full public launch is ready this week. — REFUTED (at capture)

## Evidence

- Launch copy needs a compliance caveat before broad availability. (source: product-launch-readiness)
- Regression automation still fails checkout and billing smoke tests. (source: product-launch-readiness)

## Outcome

Still holds: **no**
Reasons:
- Premised on refuted hypothesis org:launch:hypothesis:launch-ready

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:launch:option:delay`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:launch:option:delay`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:launch:evidence:legal-caveat`, `org:launch:evidence:qa-failures`)
  - where it was observed is stated for 2 of 2 (`org:launch:evidence:legal-caveat`, `org:launch:evidence:qa-failures`)
  - 1 assumption counted: a stated premise, not yet checked (`org:launch:hypothesis:launch-ready`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:launch:decision:launch-now`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:launch:option:delay`)
  - 1 evidence item on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it) (`org:launch:evidence:qa-failures`, `org:launch:hypothesis:launch-ready`)
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 40
- Source: api
- Source ref: product-launch-readiness
- Proposer: human:pm
- Accepted by: None recorded.
- Rejected by: human:legal
- Ledger timestamp: 2026-01-02T00:30:11+00:00
