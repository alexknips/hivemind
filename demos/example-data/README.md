# Example data

A fresh, purpose-built HiveMind ledger, captured entirely through the real capture verbs
(`hivemind emit decision.capture`, `supersede`, `disagree`, `ground`, `relation.added`, ...) —
never hand-written fixture JSON injected into a database. This is the public demo's example
data (hivemind-hc-yds): nothing here is real, no city decisions, no real people, no real
projects. It exists to show what HiveMind is for.

Contrast `demos/showcase/`, which is an older, hand-authored fixture corpus for local SPA
testing. That approach hid a broken live capture path for three months (hu-0pf) before anyone
noticed, because nothing exercised the real capture verbs. This generator exists so that can't
happen again: it fails loudly if the CLI's argument shape drifts out from under it.

## What's in it

Three fictional projects, four stories, eight decisions:

| Project | Decisions | Story |
|---|---|---|
| TrailKeeper (a hiking-log app) | 2 | An SQLite → Postgres supersede, as a "one hiker, one phone" assumption stops holding once people use the app on their phone and the web dashboard. |
| Ridewell API + Ridewell Mobile (a driver-dispatch backend and its mobile client; Mobile depends on API) | 4 | An agent decides a retry count on its own, grounded as a declared bet with a check-by date. A cache keyed by a driver's numeric id rests on the assumption "ids never change" — a downstream **mobile** decision follows from it, crossing the project boundary. A platform migration refutes that assumption; the API decision is superseded, and the mobile decision (never itself touched) starts reading `still_holds.held_up: false` because its premise went stale. |
| 12 Angry Men (a jury's deliberation) | 2 | A first verdict is contested the moment it's captured — one juror disagrees, on the record, beside everyone who accepted it. New evidence surfaces; the verdict is superseded, and the disagreement stays on the first verdict's record, never silently resolved. The replacement verdict is accepted by every juror, the last holdout included. |

## Files

- `snapshot/graph.json` — the body of `GET /v1/graph`, verbatim.
- `snapshot/briefs.json` — one JSON object mapping each decision id in `graph.json` to the body
  of `GET /v1/decisions/verify?id=<that id>`, verbatim (`{"data": {...}}` per entry).

These are the two files the read-only public-demo UI reads (see hivemind-2cde's originating
comment from the hivemind-ui side). Decision ids and timestamps are freshly generated on every
regeneration — real capture, not a fixed fixture — so there is no byte-stable diff to preserve
across runs; what's committed is simply the most recent blessed run.

## Regenerating

The whole ledger and both snapshot files come from one test binary,
`tests/example_data_generator.rs`:

```bash
# Check only: builds the ledger in a temp dir, calls the real API in-process, and asserts
# the snapshot's shape (decision/project counts, that a superseded decision survives and the
# first jury verdict's disagreement is still on its record). Writes nothing. This is what CI runs on every PR — no extra CI job needed,
# it's an ordinary `cargo test` target.
cargo test --test example_data_generator

# Regenerate: does the same build, then overwrites demos/example-data/snapshot/{graph,briefs}.json.
cargo test --test example_data_generator -- --bless
```

Regenerate after changing the story in `tests/example_data_generator.rs`, and commit the
resulting `snapshot/*.json` alongside the code change.
