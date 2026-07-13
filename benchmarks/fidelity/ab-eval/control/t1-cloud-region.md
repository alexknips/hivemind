# ADR: EU Customer Data Region Selection

**Status**: Accepted
**Date**: 2024-09-12
**Domain**: Infrastructure / Compliance

## Context

We are expanding to European enterprise customers. We need a dedicated AWS
region for EU customer data to satisfy GDPR data residency requirements.
Three regions were evaluated.

## Options Considered

1. **eu-central-1 (Frankfurt)** — German data center; low latency to our
   primary EU customer cohort (measured via RUM dashboard).
2. **eu-west-1 (Ireland)** — Also GDPR-compliant; historically our first choice
   for EU expansion, but RUM data showed higher latency to the bulk of our EU
   cohort than eu-central-1.
3. **us-east-1 (N. Virginia)** — Existing primary region; cannot satisfy GDPR
   data residency for EU customer data.

## Decision

**Chosen: eu-central-1 (Frankfurt)**

Host all EU customer data in eu-central-1.

## Rationale

- **GDPR compliance**: eu-central-1 satisfies GDPR data residency; us-east-1
  does not.
- **Latency**: RUM dashboard shows eu-central-1 has lower latency to our
  primary EU customer cohort than eu-west-1.

## Evidence

1. GDPR data residency requirement — EU customer data must be hosted inside
   the EU. us-east-1 is disqualified.
2. RUM dashboard — real-user measurements show eu-central-1 has lower latency
   to our EU cohort than eu-west-1.

## Consequences

- All EU tenant data stored in eu-central-1 from this date forward.
- EU-specific infrastructure deployment pipeline required.
- Compliance review complete for this region choice.
