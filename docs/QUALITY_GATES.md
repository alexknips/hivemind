# Quality Gates

Quality gates are part of every HiveMind bead's definition of done. They are
not a retrospective cleanup step, and they are not optional when a change is
small. A branch is merge-ready only when the named gate set below has passed on
the exact state being submitted.

## Mandatory Gate Set

Run this full gate set before submitting a polecat branch to refinery:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --locked --bin generate-reference -- --check
"${GC_CITY:?GC_CITY must point at the city root}/assets/scripts/ubs-rig-scan.sh" "$(pwd)"
```

The UBS wrapper is the critical-finding gate. It must report zero criticals.
If `ubs` is unavailable, the wrapper reports that it skipped; that skip must be
named in the proof line instead of hidden.

UBS warnings are a baseline gate: warning count must not grow relative to the
current target branch. Capture a JSON report for the branch under review and
compare its total warning count with the baseline report from the target branch:

```bash
ubs . --format=json --ci --quiet --include-ext=rs --report-json "$REPORT"
jq '[.scanners[].warning // 0] | add // 0' "$REPORT"
```

`--include-ext=rs` works around hivemind-0qwf: UBS ships without
`modules/contract.json`, so its language prepass classifies zero files and
`ubs` reports "no supported languages" (exit 3) instead of scanning. Drop the
flag once upstream repairs the contract file.

Any warning-count growth blocks submission or merge unless the warning is fixed
or the baseline is explicitly updated by a separate bead that explains why the
new warning is acceptable.

## Workflow Gate (changes touching `.github/workflows/**`)

If the diff touches any file under `.github/workflows/**`, additionally run:

```bash
./scripts/lint-workflows.sh
```

This runs `actionlint`, which *integrates with* shellcheck for `run:` blocks
(it does not bundle it — it execs a `shellcheck` binary and, if none is
found, silently disables that rule) so it catches bad expressions, invalid
runner labels, malformed steps, and shell bugs embedded in workflow YAML —
none of which the other gates above check. The script installs pinned
releases of both `actionlint` and `shellcheck` on demand if they are not
already on `PATH`, and fails loudly if actionlint's shellcheck rule ends up
disabled anyway, so it works the same way locally and in CI (see the
`actionlint` job in `.github/workflows/ci.yml`).

## Polecat Contract

Polecats run the smallest meaningful test while developing, then run the full
mandatory gate set immediately before marking a bead merge-ready.

The `MERGE_READY` comment and BR notes must include one proof line per gate:

```text
fmt: PASS (cargo fmt --check)
clippy: PASS (cargo clippy --locked --all-targets -- -D warnings)
test: PASS (cargo test --locked)
reference-docs: PASS (cargo run --locked --bin generate-reference -- --check)
ubs-critical: PASS (<wrapper command>; 0 criticals)
ubs-warnings: PASS (baseline=<n>, branch=<n>, no growth)
```

Add `workflow-lint: PASS (./scripts/lint-workflows.sh)` to that list whenever
the diff touches `.github/workflows/**`; omit the line otherwise.

Do not submit `verified with <tests>` or another placeholder. If a gate is
skipped because a tool is unavailable, name the skipped gate and the reason.
If the failure is caused by the branch, fix it before submission. If the
failure is pre-existing, file or reference a bead and keep the current bead out
of merge-ready status until the failure is accounted for.

## Refinery Contract

Refinery re-runs the same mandatory gate set after applying the source branch
to the current target branch. The branch-local result is not sufficient; the
rebased state is the state that can land on `master`.

If any gate fails, refinery must not push. It rejects the BR issue back to the
polecat pool with the failing gate named in both the notes and `MERGE_FAILED`
comment. A clean rejection includes the source branch, target branch, failing
command, and the relevant failure summary.

On success, the close reason must include the same proof lines used by
polecats, with results from the rebased state.

## Baseline Rule

For polecats, the warning baseline is the target branch state used to start or
rebase the work. For refinery, the warning baseline is the fetched target
branch immediately before applying the source branch. This keeps warning growth
visible and prevents a stale polecat branch from masking regressions introduced
by rebasing.
