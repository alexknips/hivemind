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
