---
id: "org:remote-db:decision:postgres-pilot"
title: "Use hosted Postgres for the short-run backend"
status: "accepted"
project: "personal:agent:ops"
project_label: "ops agents' personal project"
occurred_at: "2026-01-02T00:20:13+00:00"
topic_keys: ["org.remote-db", "architecture", "migration"]
proposer: "agent:ops"
source: "api"
source_ref: "remote-db-architecture-choice"
event_origin: 28
supersedes: null
superseded_by: null
---

# Use hosted Postgres for the short-run backend

Status: accepted
Project: ops agents' personal project

## Context

Topic keys: org.remote-db, architecture, migration

Hypotheses premised on:
- A service-owned write path can meet security and operational constraints. (org:remote-db:hypothesis:service-owned-writes): supported

## Options considered

- **org:remote-db:option:postgres** (chosen)
- org:remote-db:option:mysql

## Decision

**org:remote-db:option:postgres**

It satisfies the shared-service migration plan without introducing graph storage early.

## Rests on

- **Evidence:** Hosted Postgres meets the latency and backup constraints for the pilot. (at capture)
- **Assumption:** A service-owned write path can meet security and operational constraints. — supported (at capture)

## Evidence

- Hosted Postgres meets the latency and backup constraints for the pilot. (source: remote-db-architecture-choice)

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
  - 1 alternative recorded besides the chosen option (`org:remote-db:option:mysql`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:remote-db:option:mysql`)
- **Information** — solid
  - 1 evidence item counted: recorded before the decision or attached at capture (`org:remote-db:evidence:ops-latency`)
  - where it was observed is stated for 1 of 1 (`org:remote-db:evidence:ops-latency`)
  - 1 assumption counted: a stated premise, not yet checked (`org:remote-db:hypothesis:service-owned-writes`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:remote-db:decision:postgres-pilot`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:remote-db:option:mysql`)
  - no evidence that refutes something the decision rests on was on record before it
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 28
- Source: api
- Source ref: remote-db-architecture-choice
- Proposer: agent:ops
- Accepted by: human:architect
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:20:13+00:00
