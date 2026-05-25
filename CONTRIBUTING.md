# Contributing to HiveMind

Welcome. HiveMind is a substrate for human governance of agentic
decision-making. Contributions are welcome from humans and from AI agents —
both are first-class actors in the project.

Before you start, please read:

- [`VISION.md`](VISION.md) — why HiveMind exists and what it is for
- [`PRINCIPLES.md`](PRINCIPLES.md) — the constraints HiveMind cannot trade away
- [`STRATEGY.md`](STRATEGY.md) — the active investment fronts that judge
  whether a change is on-target
- [`AGENTS.md`](AGENTS.md) — the standard of excellence every contributor
  is held to
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — the three-layer
  architecture and other architectural decisions

## How work is tracked

HiveMind uses [beads](https://github.com/) for all work tracking. Planning
lives in beads; the level-1 docs above are the compass beads are judged
against.

- `br ready` — list beads that have no blockers and are ready to be picked up
- `br show <id>` — read a bead's description, acceptance criteria, and deps
- `br ready --priority 1` — focus on the most-important work

## Picking up work

1. Pick a bead from `br ready` whose description you understand and whose
   acceptance criteria you can meet.
2. Read the bead's description fully, including its "Principles cross-check"
   and "Quality cross-check" sections.
3. Open a branch named `polecat/<bead-id>` from `master`.
4. Implement the change. Keep the diff scoped to the bead — file follow-up
   beads for anything bigger that surfaces.

## Quality gates (PRINCIPLES §8)

Every contribution must pass these gates locally before being submitted:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The CI workflow re-runs them on every push. A pull request that fails any
gate does not merge. Quality is not a P3 follow-up — it is part of the work
that made the bead claimable in the first place.

If your change touches surfaces (CLI, MCP, future HTTP API), implement once
in the internal `commands` / `queries` module and expose identically through
every surface — see [docs/ARCHITECTURE.md → Surface
Uniformity](docs/ARCHITECTURE.md#surface-uniformity).

## Tests

HiveMind has several test layers:

```bash
cargo test                              # full default test suite
cargo test --test golden                # golden query snapshots
cargo test --test golden -- --bless     # regenerate golden snapshots after intentional change
cargo test --test local_capture_demo -- --nocapture
cargo test --test slack_app -- --nocapture
cargo test --test seed -- --include-ignored
cargo test --test seed replay_smoke -- --nocapture
cargo test --test sqlite_wal_multiprocess shared_sqlite_ledger_accepts_concurrent_process_writes -- --nocapture
```

The Kuzu feature is optional and not part of the default suite. Only run it
when changing the Kuzu adapter:

```bash
cargo test --features graph-kuzu kuzu -- --nocapture
```

## Submitting a change

1. Push your branch.
2. Open a pull request against `master`. The pull request template lists the
   gate checklist — fill it in honestly.
3. Reference the bead id in the PR description and in the commit message
   trailer (e.g., `(hivemind-bead-id-XXXX)`).
4. The PR is reviewed; once gates pass on the rebased state and review is
   complete, it lands.

## License & sign-off

HiveMind is licensed under [AGPL-3.0-or-later](LICENSE). By contributing
you agree your contribution is provided under the same license.

We use the [Developer Certificate of Origin](https://developercertificate.org/)
to track provenance. Sign off every commit with `-s`:

```bash
git commit -s -m "your message"
```

This appends a `Signed-off-by: Your Name <your@email>` trailer, which is
your assertion that you wrote the patch or otherwise have the right to
submit it under the project's license. Commits without a sign-off will be
asked to amend before merge.

## Reporting issues

Use the issue templates in [`.github/ISSUE_TEMPLATE/`](.github/ISSUE_TEMPLATE/).
For security issues, please follow [`SECURITY.md`](SECURITY.md) — do not
open a public issue.

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By
participating, you agree to abide by it.
