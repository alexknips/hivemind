---
id: "org:remote-db:decision:embedded-prototype"
title: "Keep prototype storage embedded"
status: "superseded"
project: "personal:agent:coding"
project_label: "coding agents' personal project"
occurred_at: "2026-01-02T00:20:08+00:00"
topic_keys: ["org.remote-db", "architecture"]
proposer: "agent:coding"
source: "api"
source_ref: "remote-db-architecture-choice"
event_origin: 23
supersedes: null
superseded_by: "org:remote-db:decision:service-owned-writes"
---

# Keep prototype storage embedded

Status: superseded → [Move writes behind a shared remote service](../../human-architect/decisions/2026-01-02-move-writes-behind-a-shared-remote-service-orgremot.md)
Project: coding agents' personal project

## Context

Topic keys: org.remote-db, architecture

Hypotheses premised on:
- A local embedded store is enough for the first shared pilot. (org:remote-db:hypothesis:embedded-sufficient): **refuted**

## Options considered

- **org:remote-db:option:embedded** (chosen)
- org:remote-db:option:remote-service

## Decision

**org:remote-db:option:embedded**

The local store is simplest while the data model is still moving.

## Rests on

- **Evidence:** Non-developer users need shared state across support shifts. (at capture)
- **Assumption:** A local embedded store is enough for the first shared pilot. — REFUTED (at capture)

## Evidence

- Non-developer users need shared state across support shifts. (source: remote-db-architecture-choice)

## Outcome

Still holds: **no**
Reasons:
- Superseded by org:remote-db:decision:service-owned-writes (4 ledger events later)
- Premised on refuted hypothesis org:remote-db:hypothesis:embedded-sufficient

Supersedes: None recorded.
Superseded by: [Move writes behind a shared remote service](../../human-architect/decisions/2026-01-02-move-writes-behind-a-shared-remote-service-orgremot.md)

Active blockers: None recorded.

## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:remote-db:option:remote-service`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:remote-db:option:remote-service`)
- **Information** — solid
  - 1 evidence item counted: recorded before the decision or attached at capture (`org:remote-db:evidence:shared-state`)
  - where it was observed is stated for 1 of 1 (`org:remote-db:evidence:shared-state`)
  - 1 assumption counted: a stated premise, not yet checked (`org:remote-db:hypothesis:embedded-sufficient`)
- **Reasoning** — partial
  - a rationale is recorded (stated, not judged sound) (`org:remote-db:decision:embedded-prototype`)
  - whether the inference is sound is judged, not derived: no higher than partial without a model assessment
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.
- **Bias exposure** — partial
  - 1 counter-option recorded besides the chosen option: something was set against the option taken (`org:remote-db:option:remote-service`)
  - 1 evidence item on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it) (`org:remote-db:evidence:shared-state`, `org:remote-db:hypothesis:embedded-sufficient`)
  - whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment
- **Calibration** — not assessed: No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.

## Provenance

- Event origin: 23
- Source: api
- Source ref: remote-db-architecture-choice
- Proposer: agent:coding
- Accepted by: agent:coding
- Rejected by: None recorded.
- Ledger timestamp: 2026-01-02T00:20:08+00:00
