# Shared SQLite Ledger WAL Safety

## Conclusion

Shared-ledger concurrent writes are safe for same-host HiveMind processes if every
writer opens the database through `SqliteEventLedger::open`.

The enforced conditions are:

- SQLite `journal_mode=WAL`.
- SQLite `synchronous=NORMAL`.
- A 30 second `busy_timeout` on every connection.
- Event appends are single-row autocommit `INSERT OR IGNORE` statements.
- Writers use local filesystem locking that SQLite can rely on.

Under those conditions, multiple `hivemind mcp` or CLI subprocesses can append
events to the same `ledger.sqlite` without an application-level writer
coordinator. SQLite serializes the writers, the `events.event_id` autoincrement
column defines the ledger order, and replay reads events back in that order.

## What Is Not Guaranteed

The command layer does not serialize across processes. Its `Mutex` only protects
in-process command state, such as option IDs created during one command call.
Cross-process safety relies on SQLite locking and the configured busy timeout.

Multi-event commands are event-safe but not grouped into one transaction. For
example, `decision.proposed` followed by its `relation.added` events may
interleave with events from another process. Consumers must use
`causation_event_id` and explicit payload IDs rather than assuming related
events are contiguous. A reader can also observe the prefix of a multi-event
command while that command is still appending its remaining events.

This result does not cover network filesystems, multi-machine coordination, or
filesystems with broken SQLite lock semantics. Those need a shared backend or a
single-writer service.

## Reproduction

`tests/sqlite_wal_multiprocess.rs` is the committed reproduction. It starts two
independent OS processes against one HiveMind directory. Each process proposes
1,000 decisions through the command layer. Each proposal writes one
`decision.proposed` event and one `relation.added` event, for 4,000 total ledger
events.

The test verifies:

- `latest_offset` is exactly 4,000.
- Event IDs are unique and strictly `1..=4000`.
- `replay_from(0)` returns the same monotonic event order.
- Every relation event points to an earlier decision event through
  `causation_event_id`.

Run it with:

```bash
cargo test --test sqlite_wal_multiprocess shared_sqlite_ledger_accepts_concurrent_process_writes -- --nocapture
```
