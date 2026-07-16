# Source Document Bundle — Centralized Logging Stack
# G2: 2 documents — evaluation in doc 1, chosen solution in doc 2, no explicit link

---

## Document 1: Logging Platform Evaluation (2024-02-20)

The DevOps team evaluated three options for centralized log aggregation:

1. **ELK Stack (Elasticsearch + Logstash + Kibana)** — powerful full-text
   search and visualization; high resource overhead (~6 CPU cores, 24 GB RAM
   per node); operationally complex to tune and scale.

2. **Loki + Grafana** — lightweight label-based log indexing; integrates
   natively with our existing Grafana deployment; no full-text indexing by
   default (labels only); significantly lower resource requirements.

3. **Datadog Logs (managed SaaS)** — lowest operational overhead; estimated
   cost at current log volume: $4,200/month. Exceeds the infrastructure
   budget allocation.

Evaluation criteria: operational cost, resource overhead, integration with
existing tooling, monthly spend. Decision expected by end of February.

---

## Document 2: Infrastructure Setup — Logging Service (2024-03-05)

Logging infrastructure provisioned on infra-cluster-02 (3-node).

Installed: Loki v2.9 (single binary mode), Promtail log agent deployed
to all 47 service nodes, Grafana "Platform Logs" dashboard panel added.

All production services now shipping structured logs. Retention: 30 days
hot, 90 days cold archive. INFRA-412 closed.
