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

UBS category 14 (dependency hygiene) counts every `cargo audit` output line
matching `Vulnerability|RUSTSEC` as one critical, regardless of `cargo audit`'s
own exit code or the allow-list in `.cargo/audit.toml`. On a host with
`cargo-audit` on `PATH`, that currently flags the informational advisories
`.cargo/audit.toml` already allows (`cargo audit` itself exits 0 there). Those
category-14 criticals are advisory noise, not merge-blocking: `cargo audit`'s
own exit status — run directly by the `dependency-audit` CI job in
`.github/workflows/ci.yml` — is the authoritative dependency-vulnerability
gate. If a UBS critical count is entirely explained by category-14 findings
that `.cargo/audit.toml` already allows, name that in the proof line instead
of rejecting, and confirm `cargo audit --locked` exits 0. A critical from any
other category, or an advisory `.cargo/audit.toml` does not allow, still
blocks as normal (hivemind-eako).

The CI `ubs` job installs a Rust toolchain and `cargo-audit` so its
dependency-hygiene scan sees what a local host sees, and suppresses the known
advisory finding via `--baseline=.github/ubs-baseline.json --new-only`
(hivemind-eako.3); its critical count is 0 on a clean tree. The shared
`ubs-rig-scan.sh` wrapper above does not install `cargo-audit` or apply that
baseline, so a host that already has `cargo-audit` on `PATH` can still see the
category-14 finding locally — treat it per the paragraph above rather than as
a rejection.

UBS warnings are a baseline gate: warning count must not grow relative to the
current target branch. **The number that reds CI is rust-only** —
`.github/workflows/ci.yml`'s `ubs` job (`Run UBS warning baseline scan` step)
runs the command below and compares `.totals.warning` against the baseline.
Reproduce that exact command, not a different-scoped scan:

```bash
UBS_MODULE_TIMEOUT=1200 ubs --ci --only=rust --include-ext=rs \
  --report-json "$REPORT" .
jq '.totals.warning' "$REPORT"
```

`--only=rust` scopes the scan to the rust module. **Do not drop it and sum
across `.scanners[]` instead** — that measures a different, larger quantity
(every language UBS detects in the tree, not just rust) and produced a false
"warnings grew" report during the v0.4.0 release merge (hivemind-o412): the
all-scanner total (318) was compared against the rust-only CI baseline (308)
as if the two numbers measured the same thing. They don't, and they move
independently.

`--include-ext=rs` works around hivemind-0qwf: UBS ships without
`modules/contract.json`, so its language prepass classifies zero files and
`ubs` reports "no supported languages" (exit 3) instead of scanning. Drop the
flag once upstream repairs the contract file, but keep `--only=rust` — without
it, a repaired contract file makes `ubs` scan every language the repo
contains, reopening the same mismatch.

`UBS_MODULE_TIMEOUT=1200` matches the CI job. The rust module runs `cargo
clippy --all-features`, a heavier build than the default-feature checks
elsewhere, and reliably exceeds UBS's 300s default even with a warm `target/`
cache. A timed-out module reports `"status":"partial"` with `critical` and
`warning` both `0` and a non-zero exit code — that looks like a clean pass if
you only check the counts. Check the exit code and the top-level `status`
field, and avoid a trailing `| tail` or `| jq` that would swallow the exit
code of the `ubs` invocation itself.

There is currently no working supplementary "all languages" check: running
`ubs` without `--include-ext=rs` hits the hivemind-0qwf contract-file gap
directly and exits 3 with zero scanners run, not a cross-language total.
Don't add one back without first confirming upstream has repaired the
contract file.

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
ubs-warnings: PASS (rust-only, --only=rust; baseline=<n>, branch=<n>, no growth)
```

Add `workflow-lint: PASS (./scripts/lint-workflows.sh)` to that list whenever
the diff touches `.github/workflows/**`; omit the line otherwise.

If the wrapper's critical count is nonzero but entirely explained by
category-14 (dependency hygiene) findings that `.cargo/audit.toml` already
allows, say so instead of rejecting, e.g.:

```text
ubs-critical: PASS (<wrapper command>; 0 criticals outside category 14;
  N category-14 findings covered by .cargo/audit.toml; cargo audit --locked: PASS)
```

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
