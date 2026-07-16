# Architecture Note: Auth Token Caching Strategy
# G1: single document, implicit refutation (no "refuted"/"unsound" label)

**Status**: Accepted
**Date**: 2024-03-08
**Domain**: Backend / Security

## Decision

Cache authenticated user profile data (roles, permissions) in Redis for
15 minutes per session to avoid repeated database reads on every request.
The 15-minute window was chosen based on typical session lengths and the
low observed frequency of mid-session permission changes in our audit logs.

## Implementation

Deployed in auth-service v2.4. Cache key: user_id. Backend: Redis instance
auth-cache-01. Target hit rate: ≥90%.

## Operational Observations (2024-07-20)

Cache hit rate is running at 93–95%, meeting the performance target.

The security team reviewed two support tickets (#4471, #4489) where users
retained access to resources for up to 14 minutes after their roles were
revoked. Both cases involved contractor offboarding where role removal
happened during an active session. The incidents were identified through
daily audit log review, not automated alerting.

Immediate mitigation implemented: auth-service now flushes the cache entry
for a user when HR systems emit an offboarding event. A longer-term fix
to the TTL strategy is under review.
