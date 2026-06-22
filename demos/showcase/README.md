# HiveMind Showcase Corpus — Acme Payments

Synthetic decision history for a B2B embedded-payments startup.
Designed to exercise every visualisation feature of the SPA + 2-D spectral map.

## Load

```bash
cargo build --release --bin hivemind
HIVEMIND_DIR=./hivemind-demo/ TENANT=demo ./demos/showcase/load_showcase.sh
```

## What's in the corpus

| Feature | Count |
|---------|-------|
| Decisions | 25 |
| Evidence nodes | 9 |
| Hypothesis nodes | 3 (1 confirmed, 2 refuted) |
| Options (HAS_OPTION/CHOSE) | 40+ |
| Actors | 7 (5 human, 2 agent) |
| Topics | 5: infrastructure, payments, security, product, engineering |
| Supersession chains | 2 (auth D-003→D-010→D-011; DB D-001→D-012→D-013) |
| Contested decisions | 2 (D-008, D-023 — accepted_by and rejected_by split) |
| Cross-topic bridges | 6+ (security↔payments, product↔payments, infra↔security, etc.) |
| Decision requests | 1 |
| Blockers + resolutions | 2 |
| Semantic relations | 9 (SUPPORTS, ASSUMES, REFUTES) |
| Sources | cli + slack + document (bitemporal) |

## Scorer metrics

Authored importance/quality scores are defined in `corpus.yaml` and can be
emitted as `decision.scored` events once branch
`hivemind-m2-shared-backend-lives-uuq9.19` is merged:

```bash
EMIT_SCORES=true ./demos/showcase/load_showcase.sh
```

## Narrative

See `corpus.yaml` for the full Acme Payments story across 12 chapters:
Chapter 1 (Foundation) → Chapter 2 (Payments) → Chapter 3 (Security Incident) →
Chapter 4 (Supersession chains) → Chapter 5 (Product pivot) → Chapter 6 (Compliance).
