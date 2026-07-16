# Sprint Planning Notes — Q4 2024 Week 3
# G1: single document, decision buried in prose (no "Decision:", "Options:", headings)

**Date**: 2024-10-14
**Attendees**: Engineering team leads, PM (Rosario)

## Engineering Items

**Async notification queue**: The notification service needs a durable queue
for background jobs. The platform team ran a spike last week comparing
three candidates: RabbitMQ, Apache Kafka, and AWS SQS. After weighing
message durability, operational simplicity, and the fact that we do not
need stream replay or multi-consumer fan-out at current scale, the team
agreed to move forward with RabbitMQ. Kafka was seen as over-engineered
for our throughput and retention needs; SQS adds AWS vendor lock-in the
team prefers to avoid. INFRA-339 opened for the setup work.

**CI pipeline**: Merged the parallel test runner. Build time down ~40%.

## Product Items

Mobile push notification schedule moved to end of Q4. No change to scope.

## Action Items

- INFRA-339: RabbitMQ setup — owner: Deepak, target: Oct 25
- CI monitoring dashboard: Jen to update thresholds by Oct 18
