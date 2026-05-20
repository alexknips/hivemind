# Milestone 2 Local Demo: Import Plus Weekly Diff

This walks through the end-to-end Milestone 2 flow on a single machine:

1. Import an "older" decision-note document.
2. Capture the latest ledger offset as the boundary.
3. Import a "newer" decision-note document.
4. Ask HiveMind which decisions were added to the graph after the boundary,
   including `since last week` resolution via a frozen clock.

The integration test `tests/local_import_weekly_diff_demo.rs` exercises the
same flow against deterministic fixtures
(`tests/fixtures/m2_weekly_diff_demo/`).

## Prerequisites

Build the CLI once:

```bash
cargo build --bin hivemind
```

Use `target/debug/hivemind` for the rest of this walkthrough. Pick a fresh
working directory so the demo does not collide with an existing graph:

```bash
HM_DIR=$(mktemp -d)/hivemind
```

## Step 1: Import the "older" document

```bash
target/debug/hivemind \
  --actor importer:last-week \
  --hivemind-dir "$HM_DIR" \
  --json \
  import documents tests/fixtures/m2_weekly_diff_demo/last_week
```

Expect `summary.blocks_imported == 1`. Record the latest ledger offset, which
will be the diff boundary:

```bash
target/debug/hivemind \
  --hivemind-dir "$HM_DIR" \
  --json \
  query get_recent_activity --limit 1 \
  | jq -r '.data.items[0].event_origin'
```

Store that value in `BOUNDARY` for the rest of the demo.

## Step 2: Import the "newer" document

```bash
target/debug/hivemind \
  --actor importer:this-week \
  --hivemind-dir "$HM_DIR" \
  --json \
  import documents tests/fixtures/m2_weekly_diff_demo/this_week
```

The reported `import_run_id` will appear in the query response below under
`creation.import_run_id` for every event emitted by this run.

## Step 3: Ask "what was added after the boundary?"

Using the deterministic ledger boundary:

```bash
target/debug/hivemind \
  --hivemind-dir "$HM_DIR" \
  --json \
  query get_decisions_added_since --since-offset "$BOUNDARY" --limit 50
```

Expect a response with:

- `data.total_added == 1`
- `data.total_changed_existing == 0`
- `data.added_decisions[0].decision_id` containing
  `decision:document:cache_eviction-…:weekly-cache-eviction`
- `data.added_decisions[0].creation.source == "document"`
- `data.added_decisions[0].creation.import_run_id` equal to the
  `import_run_id` reported in Step 2

The last-week decision must not appear in either `added_decisions` or
`changed_existing_decisions` — it sits outside the window by ledger offset.

## Step 4: Ask "since last week" with a frozen clock

The same query accepts relative phrases when the caller supplies a fixed
`--now` and a timezone, so the resolved window is reproducible regardless of
wall-clock skew:

```bash
target/debug/hivemind \
  --hivemind-dir "$HM_DIR" \
  --json \
  query get_decisions_added_since \
  --since "last week" \
  --timezone UTC \
  --now 2026-05-19T12:00:00Z \
  --source document
```

The response includes `data.resolved_since.timestamp` reflecting the start
of the previous ISO week (Monday 00:00:00 UTC) and
`data.resolved_until.timestamp` either `--now` or, when omitted, command-start.
Supported relative phrases in slice 1 are `last week`, `this week`, `today`,
`yesterday`, and `now`. Non-UTC timezones return an error in this slice.

## Running the test

```bash
cargo test --test local_import_weekly_diff_demo
```

The test imports both fixtures, records the boundary offset, and verifies the
diff query separates the two decisions correctly with full document
provenance.
