# Source Document Bundle — Asynchronous Messaging Infrastructure
# G3: 3 documents, temporal gaps (Apr 2023→Dec 2023→Jun 2024),
#     terminology drift (backbone/message broker/broker cluster),
#     zero cross-references between documents

---

## Document 1: Platform Design Specification (2023-04-12)

**Component: Asynchronous Event Backbone**

The platform will use Apache Kafka as the core event backbone. The broker
cluster is provisioned at 2 GB/s aggregate ingestion throughput, representing
a 10× margin over the current measured peak of 200 MB/s. Based on observed
compound growth of roughly 30% per year, this margin is expected to remain
adequate through at least the end of 2027.

---

## Document 2: Infrastructure Capacity Review — Q4 2023 (2023-12-08)

**Topic: Message broker cluster utilization**

Consumer lag on the primary pipeline topic has grown from under one second
to an average of 8 minutes over the past six weeks. The growth is not
correlated with any service deployment or configuration change. Broker
throughput metrics are approaching configured limits.

The infrastructure team is investigating partition count and broker JVM
heap configuration. No root cause identified yet. Monitoring continues.

---

## Document 3: Post-Incident Review PIR-2024-031 (2024-06-20)

**Incident**: 4-hour telemetry processing outage affecting three enterprise
customers.

**Root cause**: Bursty correlated telemetry storms from large enterprise
accounts whose primary business hours overlap (North America + APAC time
zones) drove combined throughput to 2.1 GB/s, marginally exceeding the
cluster's rated ingestion capacity.

**Contributing factor**: The original capacity model assumed telemetry
events would distribute uniformly across the business day. Production
data shows enterprise customers exhibit bursty correlated load patterns
during overlapping business hours.

**Remediation**: Broker cluster capacity increased 3×. The original four-year
headroom estimate did not account for correlated enterprise load bursts.
