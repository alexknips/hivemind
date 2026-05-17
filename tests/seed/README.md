# HiveMind Seed Dataset

Run the deterministic slice-1 seed harness with:

```bash
cargo test --test seed -- --include-ignored
```

By default the ignored seed test resets and populates `./hivemind/`. Set
`HIVEMIND_SEED_DIR=/path/to/hivemind` to write somewhere else.

The non-ignored tests verify that the canonical ledger event stream is
byte-identical across clean seed runs and that the dataset covers the slice-1
demo cases.
