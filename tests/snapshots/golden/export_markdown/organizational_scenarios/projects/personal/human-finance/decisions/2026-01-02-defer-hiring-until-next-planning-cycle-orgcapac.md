---
id: "org:capacity:decision:defer-hire"
title: "Defer hiring until next planning cycle"
status: "rejected"
project: "personal:human:finance"
project_label: "finance's personal project"
occurred_at: "2026-01-02T00:50:08+00:00"
topic_keys: ["org.capacity", "planning"]
proposer: "human:finance"
source: "api"
source_ref: "hiring-capacity-planning"
event_origin: 67
supersedes: null
superseded_by: null
---

# Defer hiring until next planning cycle

Status: rejected
Project: finance's personal project

## Context

Topic keys: org.capacity, planning

Hypotheses premised on:
- A contractor can cover the next milestone without onboarding drag. (org:capacity:hypothesis:contractor-covers-gap): **refuted**

## Options considered

- **org:capacity:option:defer** (chosen)
- org:capacity:option:hire-now

## Decision

**org:capacity:option:defer**

Budget timing is tight and headcount can wait if scope shrinks.

## Rests on

- **Evidence:** Budget can support one full-time hire but not two contractors. (at capture)
- **Assumption:** A contractor can cover the next milestone without onboarding drag. — REFUTED (at capture)

## Evidence

- Budget can support one full-time hire but not two contractors. (source: hiring-capacity-planning)

## Outcome

Still holds: **no**
Reasons:
- Premised on refuted hypothesis org:capacity:hypothesis:contractor-covers-gap

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:capacity:option:hire-now`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:capacity:option:hire-now`)
- **Information** — solid
  - 1 evidence item counted: recorded before the decision or attached at capture (`org:capacity:evidence:budget-window`)
  - where it was observed is stated for 1 of 1 (`org:capacity:evidence:budget-window`)
  - 1 assumption counted: a stated premise, not yet checked (`org:capacity:hypothesis:contractor-covers-gap`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:capacity:decision:defer-hire`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:capacity:option:hire-now`)
  - 1 evidence item on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it) (`org:capacity:evidence:contractor-ramp`, `org:capacity:hypothesis:contractor-covers-gap`)
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 67
- Source: api
- Source ref: hiring-capacity-planning
- Proposer: human:finance
- Accepted by: None recorded.
- Rejected by: agent:team-lead
- Ledger timestamp: 2026-01-02T00:50:08+00:00
