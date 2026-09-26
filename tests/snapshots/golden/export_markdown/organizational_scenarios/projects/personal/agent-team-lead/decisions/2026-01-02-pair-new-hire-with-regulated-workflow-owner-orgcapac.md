---
id: "org:capacity:decision:onboarding-plan"
title: "Pair new hire with regulated workflow owner"
status: "accepted"
project: "personal:agent:team-lead"
project_label: "team-lead agents' personal project"
occurred_at: "2026-01-02T00:50:15+00:00"
topic_keys: ["org.capacity", "hiring"]
proposer: "agent:team-lead"
source: "api"
source_ref: "hiring-capacity-planning"
event_origin: 74
supersedes: null
superseded_by: null
---

# Pair new hire with regulated workflow owner

Status: accepted
Project: team-lead agents' personal project

## Context

Topic keys: org.capacity, hiring

Hypotheses premised on:
- A full-time backend hire pays off after the first onboarding month. (org:capacity:hypothesis:fulltime-payoff): supported

## Options considered

- **org:capacity:option:paired-onboarding** (chosen)
- org:capacity:option:self-serve-onboarding

## Decision

**org:capacity:option:paired-onboarding**

The onboarding plan addresses the capacity risk without losing domain context.

## Rests on

- **Evidence:** Recent contractors took longer to ramp on regulated workflows. (at capture)
- **Assumption:** A full-time backend hire pays off after the first onboarding month. — supported (at capture)

## Evidence

- Recent contractors took longer to ramp on regulated workflows. (source: hiring-capacity-planning)

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
  - 1 alternative recorded besides the chosen option (`org:capacity:option:self-serve-onboarding`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:capacity:option:self-serve-onboarding`)
- **Information** — solid
  - 1 evidence item counted: recorded before the decision or attached at capture (`org:capacity:evidence:contractor-ramp`)
  - where it was observed is stated for 1 of 1 (`org:capacity:evidence:contractor-ramp`)
  - 1 assumption counted: a stated premise, not yet checked (`org:capacity:hypothesis:fulltime-payoff`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:capacity:decision:onboarding-plan`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:capacity:option:self-serve-onboarding`)
  - no evidence that refutes something the decision rests on was on record before it
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 74
- Source: api
- Source ref: hiring-capacity-planning
- Proposer: agent:team-lead
- Accepted by: human:recruiting
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:50:15+00:00
