# Deployment Strategy — Platform Services
# G1: single document, supersession implicit (no "Supersedes:" / "Superseded by:" labels)

**Domain**: DevOps / Reliability

## History

### Phase 1 (Q2 2023): Blue-Green Deployment

Production releases deployed using blue-green: a complete parallel environment
spun up, traffic switched atomically, old environment torn down post-validation.
Rollbacks required switching traffic back to the prior environment.

### Phase 2 (Q1 2024): Canary Rollout

Following three production incidents in H2 2023 where full blue-green
environment switches exposed environment configuration drift (secrets mismatch,
service discovery lag), the platform team moved to a canary release model.

Under the current process, a release ships to 5% of traffic for a 30-minute
observation window. If the error rate stays within the baseline band, traffic
expands to 25% then 100%. Rollbacks target only the canary slice, limiting
blast radius.

The team observed approximately 60% fewer production incidents in the first
two quarters after adopting canary releases.

## Current Status

Canary releases are the standard deployment method as of 2024-Q1.
Blue-green environment switching is retired for normal production releases.
