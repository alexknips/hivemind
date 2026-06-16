# HiveMind Seed Dataset

Run the deterministic slice-1 seed harness with:

```bash
cargo test --test seed -- --include-ignored
```

By default the ignored seed test populates `./hivemind/` and refuses to run if
that directory already exists. Set `HIVEMIND_SEED_DIR=/path/to/hivemind` to
write somewhere else.

The non-ignored tests verify that the canonical ledger event stream is
byte-identical across clean seed runs and that the dataset covers the slice-1
demo cases.

Replay smoke coverage runs with:

```bash
cargo test --test seed replay_smoke -- --nocapture
```

The replay smoke test seeds a fresh temporary ledger, captures all three query
outputs, captures them again from fresh read projections, and prints a warning
diff if they diverge after normalizing volatile `latency_ms`. It is
intentionally non-gating for output drift: it exits non-zero only when setup,
ledger IO, projection, or query execution fails. Slice 1 has no persistent
graph file, so the smoke test does not delete project data.

Golden query snapshots run with:

```bash
cargo test --test golden
```

Regenerate `tests/snapshots/golden/*.json` after intentional query output changes
with:

```bash
cargo test --test golden -- --bless
```

## Multi-tenant test fixtures

`tests/support/multi_tenant.rs` provides reusable helpers for multi-tenant code paths.

### Helpers

| Symbol | Purpose |
|---|---|
| `TenantDataset` | Named tenant with its generated IDs (decisions, evidence, hypothesis) |
| `seed_tenant(ledger, name)` | Seed 10 decisions + evidence + hypothesis for a named tenant on any `EventLedger` |
| `assert_tenant_isolation(ledger, scope, other)` | Assert events and graph for `scope` contain no data from `other` |
| `assert_tenant_completeness(ledger, dataset)` | Assert all of a tenant's decisions appear in its event stream |
| `assert_graph_completeness(ledger, dataset)` | Assert all of a tenant's decisions appear in its rebuilt graph |

### Usage

```rust
#[path = "support/multi_tenant.rs"]
mod multi_tenant;

use multi_tenant::{seed_tenant, assert_tenant_isolation, TenantDataset};
use hivemind::ledger::InMemoryEventLedger;

#[test]
fn my_multi_tenant_test() -> multi_tenant::TestResult<()> {
    let ledger = InMemoryEventLedger::new();
    let alpha = seed_tenant(&ledger, "alpha-corp")?;
    let beta  = seed_tenant(&ledger, "beta-startup")?;

    assert_tenant_isolation(&ledger, &alpha, &beta)?;
    Ok(())
}
```

The helpers work with any `EventLedger` implementation — `InMemoryEventLedger` for unit speed,
`SqliteEventLedger` for persistence, or `PostgresEventLedger` for the shared backend.

Run the multi-tenant integration suite with:

```bash
cargo test --locked --test multi_tenant
```
