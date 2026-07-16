# Source Document Bundle — Customer Database Sharding
# G2: 2 documents, no cross-references, different terminology for the same system

---

## Document 1: Architecture Decision — Database Partitioning (2023-11-10)

We will partition the customer database into 16 shards using user_id modulo
as the routing key. This strategy assumes that user account creation is
approximately uniformly distributed across ID ranges over the long run,
which will keep shard sizes balanced without rebalancing.

Alternative considered: time-based range partitioning. Rejected because
write-heavy periods create hot-shard risk under time-based schemes, whereas
modulo routing distributes writes evenly given uniform ID assignment.

---

## Document 2: Operations Incident Report INC-882 (2024-06-03)

**Severity**: P2 — Elevated Query Latency (Partition 3)

**Observed**: Partition 3 query response time elevated starting 2024-05-28;
p99 exceeded 2 seconds. No deployment or configuration changes identified.

**Root cause**: Partition 3 now holds 3.8× the row count of adjacent
partitions. Analysis: accounts created during the 2022 growth campaign
were assigned IDs in the 300–400K range. Users from that cohort generate
60–80% of total write throughput due to high engagement, concentrating
load on partition 3.

**Mitigation**: Provisioned two read replicas for partition 3. Latency
normalized within 6 hours.

**Long-term remediation**: Evaluate consistent hashing or virtual node
partitioning to tolerate non-uniform write distributions.
