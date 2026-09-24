---
id: "org:capacity:decision:contractor"
title: "Use a contractor for backend capacity"
status: "contested"
project: "personal:human:manager"
project_label: "manager's personal project"
occurred_at: "2026-01-02T00:50:10+00:00"
topic_keys: ["org.capacity", "planning"]
proposer: "human:manager"
source: "api"
source_ref: "hiring-capacity-planning"
event_origin: 69
supersedes: null
superseded_by: null
---

# Use a contractor for backend capacity

Status: contested — accepted by: human:manager; rejected by: agent:team-lead
Project: manager's personal project

## Context

Topic keys: org.capacity, planning

Hypotheses premised on:
- A contractor can cover the next milestone without onboarding drag. (org:capacity:hypothesis:contractor-covers-gap): **refuted**

## Options considered

- **org:capacity:option:contractor** (chosen)
- org:capacity:option:fulltime

## Decision

**org:capacity:option:contractor**

Contracting keeps the team flexible while recruiting starts.

## Rests on

- **Evidence:** Recent contractors took longer to ramp on regulated workflows. (at capture)
- **Evidence:** Roadmap forecast shows two quarters of backend capacity shortfall. (at capture)
- **Assumption:** A contractor can cover the next milestone without onboarding drag. — REFUTED (at capture)

## Evidence

- Recent contractors took longer to ramp on regulated workflows. (source: hiring-capacity-planning)
- Roadmap forecast shows two quarters of backend capacity shortfall. (source: hiring-capacity-planning)

## Outcome

Still holds: **no**
Reasons:
- Premised on refuted hypothesis org:capacity:hypothesis:contractor-covers-gap
- Contested

Supersedes: None recorded.
Superseded by: None recorded.

Active blockers: None recorded.

## Provenance

- Event origin: 69
- Source: api
- Source ref: hiring-capacity-planning
- Proposer: human:manager
- Accepted by: human:manager
- Rejected by: agent:team-lead
- Ledger timestamp: 2026-01-02T00:50:10+00:00
