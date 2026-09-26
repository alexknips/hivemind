---
id: "org:remote-db:decision:service-owned-writes"
title: "Move writes behind a shared remote service"
status: "accepted"
project: "personal:human:architect"
project_label: "architect's personal project"
occurred_at: "2026-01-02T00:20:10+00:00"
topic_keys: ["org.remote-db", "architecture", "security"]
proposer: "human:architect"
source: "api"
source_ref: "remote-db-architecture-choice"
event_origin: 25
supersedes: "org:remote-db:decision:embedded-prototype"
superseded_by: null
---

# Move writes behind a shared remote service

Status: accepted
Project: architect's personal project

## Context

Topic keys: org.remote-db, architecture, security

Hypotheses premised on:
- A service-owned write path can meet security and operational constraints. (org:remote-db:hypothesis:service-owned-writes): supported

## Options considered

- **org:remote-db:option:service-owned-writes** (chosen)
- org:remote-db:option:direct-clients

## Decision

**org:remote-db:option:service-owned-writes**

Shared state and security review both require service ownership.

## Rests on

- **Evidence:** Security review rejects direct database writes from every client. (at capture)
- **Evidence:** Non-developer users need shared state across support shifts. (at capture)
- **Assumption:** A service-owned write path can meet security and operational constraints. — supported (at capture)

## Evidence

- Security review rejects direct database writes from every client. (source: remote-db-architecture-choice)
- Non-developer users need shared state across support shifts. (source: remote-db-architecture-choice)

## Outcome

Still holds: **yes**
Reasons: None recorded.

Supersedes: [Keep prototype storage embedded](../../agent-coding/decisions/2026-01-02-keep-prototype-storage-embedded-orgremot.md)
Superseded by: None recorded.

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:remote-db:option:direct-clients`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:remote-db:option:direct-clients`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:remote-db:evidence:security-review`, `org:remote-db:evidence:shared-state`)
  - where it was observed is stated for 2 of 2 (`org:remote-db:evidence:security-review`, `org:remote-db:evidence:shared-state`)
  - 1 assumption counted: a stated premise, not yet checked (`org:remote-db:hypothesis:service-owned-writes`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:remote-db:decision:service-owned-writes`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:remote-db:option:direct-clients`)
  - no evidence that refutes something the decision rests on was on record before it
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 25
- Source: api
- Source ref: remote-db-architecture-choice
- Proposer: human:architect
- Accepted by: human:security-reviewer
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:20:10+00:00
