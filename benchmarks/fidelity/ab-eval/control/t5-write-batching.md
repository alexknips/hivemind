# ADR: Write Batching for Latency Reduction

**Status**: Accepted — hypothesis refuted; batching NOT added
**Domain**: Research / Performance

## Context

p99 write latency was a concern. We hypothesized that batching writes
would reduce p99 latency by reducing the number of fsync calls per unit
of data written.

## Hypothesis

"Batching writes will cut p99 latency."

## Decision

**Do not add write batching.** Revisit fsync strategy instead.

## Rationale

The benchmark refuted the hypothesis: batching reduced throughput cost
but left p99 latency **unchanged**. The actual bottleneck is fsync, not
the number of write calls. Batching would add implementation complexity
without the expected latency benefit.

## Evidence

- Benchmark result: write batching reduced per-operation CPU and I/O
  cost but p99 latency was unchanged. Root cause: the bottleneck is
  the fsync call at the end of each write transaction, which batching
  does not eliminate.

## Status of Hypothesis

**Refuted.** The hypothesis that batching writes would cut p99 latency
was proven false by the benchmark: p99 was unchanged because the true
bottleneck is fsync, not write call frequency.

## Consequences

- No write batching implemented.
- Engineering investigation redirected to fsync strategy optimization.
