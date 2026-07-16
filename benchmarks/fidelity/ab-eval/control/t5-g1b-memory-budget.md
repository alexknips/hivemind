# Technical Specification: Decision Graph In-Memory Index
# G1: single document, implicit refutation (no "refuted"/"unsound" label)

**Status**: Accepted
**Date**: 2024-01-22
**Domain**: Storage / Performance

## Decision

Maintain the decision graph index fully in memory. Per-node memory budget:
4 GB per server node, supporting up to 500,000 decision nodes. Sizing basis:
average of 8 KB per node (compact structured metadata, short evidence strings).

## Sizing Rationale

Typical organizational deployment: 50,000–100,000 decisions over a 10-year
horizon. 4 GB provides approximately 5× headroom over the expected maximum.

## Benchmark Results (2024-04-10)

In-memory index at 100,000 nodes: p50 read latency 1.2 ms, p99 4.1 ms.
Both within target. Index build time: 8 seconds from cold start.

## Production Report (2024-08-15)

Two enterprise customers exceeded 200,000 nodes within six months of
onboarding due to bulk historical data imports. Both nodes hit memory
exhaustion at approximately 210,000 nodes and restarted with degraded
service for 12–18 minutes each.

Post-mortem analysis: bulk imports include full document text in the
evidence fields, averaging 40 KB per node rather than the 8 KB compact
case. The bulk import code path does not enforce the structured-only
constraint assumed in the sizing model.
