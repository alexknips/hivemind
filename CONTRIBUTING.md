# Contributing

## Golden Query Snapshots

Run the golden snapshot harness with:

```bash
cargo test --test golden
```

Regenerate the committed query snapshots after an intentional query response
change with:

```bash
cargo test --test golden -- --bless
```

The harness seeds a fresh temporary HiveMind directory, runs all three query
subcommands, normalizes volatile `latency_ms`, and compares the JSON outputs
against `snapshots/golden/*.json`. Mismatches print a unified diff.
