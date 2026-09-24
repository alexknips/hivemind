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

## Provenance

- Event origin: 67
- Source: api
- Source ref: hiring-capacity-planning
- Proposer: human:finance
- Accepted by: None recorded.
- Rejected by: agent:team-lead
- Ledger timestamp: 2026-01-02T00:50:08+00:00
