# Quality Gates

Quality gates are part of every HiveMind bead's definition of done. They are
not a retrospective cleanup step, and they are not optional when a change is
small. A branch is merge-ready only when the gate set matching its diff scope
(below) has passed on the exact state being submitted.

## Diff Scope: Docs-Only vs Code

Classify the diff before running anything:

- **Docs-only diff**: no changed file is a `.rs` file, `Cargo.toml`,
  `Cargo.lock`, or a build/CI file (`.github/workflows/**`, `Dockerfile*`,
  `docker-compose*.yml`, `build.rs`, `rust-toolchain*`, `.cargo/**`, or a gate
  script under `scripts/**`). Markdown and other prose/config changes are
  docs-only.
- **Code diff**: everything else. A diff that touches even one `.rs` or build
  file is a code diff in full, even when it also touches docs.

This split exists because the UBS wrapper (see below) runs `cargo clippy
--all-features`, rebuilding feature combinations (kuzu, tui,
shared-backend-postgres) that most local dev flows never otherwise build —
cold, that can take hours. A 2-line docs fix (hivemind-7fe7) spent 3h10m
stuck in that build for a change that could not possibly touch Rust code.
This changes WHERE gates run for a docs-only diff, not WHAT passes: CI still
runs the full gate set (the `rust` and `ubs` jobs in
`.github/workflows/ci.yml`) on every push and PR regardless of diff scope, so
a code change smuggled into a diff mis-classified as docs-only is still
caught server-side. (hivemind-0ls8, Alex-approved 2026-09-19)

## Docs-Only Gate Set

There is no local gate for a docs-only diff. Skip `cargo fmt`, `cargo clippy`,
`cargo test`, and the UBS wrapper entirely — none of them can be broken by a
diff that touches no Rust or build file. The site and its docs no longer live
in this repo, so there is no site build or link check here either; see
[Reference Docs](#reference-docs-live-in-the-site-repo) for where the doc-code
sync check runs now.

## Reference Docs (Live in the Site Repo)

The site, the CLI and MCP reference pages, the MCP setup guide and the
homepage's tool count live in `alexknips/hivemind-site` (under its
`website/`), not in this repo. The generator stays here because it reads this
repo's clap definitions and `tool_definitions()`. It reads and writes
`website/...` relative to the directory it runs from, so run it from the root
of a site checkout, pointing cargo at this repo:

```bash
# cwd = root of alexknips/hivemind-site; this repo checked out at ../hivemind
cargo run --locked --manifest-path ../hivemind/Cargo.toml --bin generate-reference            # rewrite
cargo run --locked --manifest-path ../hivemind/Cargo.toml --bin generate-reference -- --check # verify
```

The site repo's `Reference docs in sync` workflow runs that check against
hivemind `master` on every push, pull request, and daily. It is not a merge
gate of this repo: a change here that leaves the reference stale turns the
site's workflow red, not this repo's CI. A bead that adds or changes a CLI
subcommand, a flag, or an MCP tool therefore also opens a matching pull
request on the site repo that regenerates the reference — merge the
hivemind change first, because the site's check reads hivemind `master`.

## Code Gate Set (Mandatory)

Run this full gate set before submitting a polecat branch with any code diff:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

`cargo clippy` here is default-features only. It does not exercise the
feature-gated surfaces: CI's `tui` and `shared-backend-postgres` legs cover
those two, and the [Kuzu nightly](#kuzu-nightly) covers `graph-kuzu`. A clean
local run is not proof those configurations are clean.

ubs-critical and ubs-warnings run in CI only — see the next section. Do not
run the local UBS wrapper as part of this checklist; it is no longer a
mandated local step for any diff.

## UBS Gates (CI Only)

The `ubs` job in `.github/workflows/ci.yml` is the critical-finding and
warning-baseline gate. Gate evidence is the green `ubs` job on the draft
validation PR for the exact commit head being submitted — see the Refinery
Contract below for how that evidence is produced. It must report zero
criticals. If you need to reproduce or debug a red `ubs` job locally, the
wrapper below runs the same scan:

```bash
"${GC_CITY:?GC_CITY must point at the city root}/assets/scripts/ubs-rig-scan.sh" "$(pwd)"
```

If `ubs` is unavailable, the wrapper reports that it skipped; that skip must
still be named in the CI job's log instead of hidden.

UBS category 14 (dependency hygiene) counts every `cargo audit` output line
matching `Vulnerability|RUSTSEC` as one critical, regardless of `cargo audit`'s
own exit code or the allow-list in `.cargo/audit.toml`. On a host with
`cargo-audit` on `PATH`, that currently flags the informational advisories
`.cargo/audit.toml` already allows (`cargo audit` itself exits 0 there). Those
category-14 criticals are advisory noise, not merge-blocking: `cargo audit`'s
own exit status — run directly by the `dependency-audit` CI job in
`.github/workflows/ci.yml` — is the authoritative dependency-vulnerability
gate. If a UBS critical count is entirely explained by category-14 findings
that `.cargo/audit.toml` already allows, name that in the gate-report proof
line instead of rejecting, and confirm `cargo audit --locked` exits 0. A
critical from any other category, or an advisory `.cargo/audit.toml` does not
allow, still blocks as normal (hivemind-eako).

The CI `ubs` job installs a Rust toolchain and `cargo-audit` so its
dependency-hygiene scan sees what a local host sees, and suppresses the known
advisory finding via `--baseline=.github/ubs-baseline.json --new-only`
(hivemind-eako.3); its critical count is 0 on a clean tree. The shared
`ubs-rig-scan.sh` wrapper above does not install `cargo-audit` or apply that
baseline, so reproducing locally on a host that already has `cargo-audit` on
`PATH` can still surface the category-14 finding — treat it per the
paragraph above rather than as a real failure.

UBS warnings are a baseline gate: warning count must not grow relative to the
current target branch. **The number that reds CI is rust-only** —
`.github/workflows/ci.yml`'s `ubs` job (`Run UBS warning baseline scan` step)
runs the command below and compares `.totals.warning` against the baseline.
Reproduce that exact command if debugging, not a different-scoped scan:

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

## Kuzu (Nightly)

Kuzu (`graph-kuzu`) compiles a bundled C++ database. A cold build takes about
100 minutes on a loaded host, more than a merge gate can spend, so no per-change
gate builds it: not the polecat, not the refinery, and not the pull-request CI,
whose `rust` legs run default features, `tui`, and
`shared-backend-postgres` only. The evidence for the Kuzu backend is
`.github/workflows/kuzu-nightly.yml`. Every night on `master`, and on demand, it
builds `hivemind` with `--features graph-kuzu` and runs the library tests with
the feature on. Those include the Kuzu adapter's parity tests against the other
backends and the `--graph-backend kuzu` CLI path. The Kuzu build is cached
between runs.

The nightly is not a merge gate: it is not a required check, and a red run
never blocks a merge. It shows on the Actions page and in the README badge. The
accepted cost is that a change that breaks Kuzu can reach `master` and is caught
the next night, so a red nightly is a defect in `master`: file a bug for each
distinct failure.

To get the evidence before the next scheduled run, dispatch it:
`gh workflow run kuzu-nightly.yml --ref master` (or a pushed branch).

## Workflow Gate (changes touching `.github/workflows/**`)

A diff touching `.github/workflows/**` is a build file change and therefore a
code diff by definition (see Diff Scope above). In addition to the Code Gate
Set, run:

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

Polecats run the smallest meaningful test while developing, then run the
full gate set matching the diff's scope immediately before marking a bead
merge-ready.

The `MERGE_READY` comment and BR notes must include one proof line per gate.
For a code diff:

```text
fmt: PASS (cargo fmt --check)
clippy: PASS (cargo clippy --locked --all-targets -- -D warnings)
test: PASS (cargo test --locked)
ubs: CI (draft validation PR <url>, `ubs` job green @ <commit sha>)
```

For a docs-only diff there is no gate to report; say `docs-only: no local gate`.

Add `workflow-lint: PASS (./scripts/lint-workflows.sh)` to the code-diff list
whenever the diff touches `.github/workflows/**`; omit the line otherwise.

Add `kuzu: covered by nightly <run url> after merge` to the code-diff list
whenever the diff changes the Kuzu backend (the adapter in `src/projector/kuzu*`,
the `graph-kuzu` paths in `src/cli/`, or the `kuzu` / `cxx-build` lines in
`Cargo.toml`); omit the line otherwise. Do not build Kuzu to fill it in. The
run that carries the evidence is the first nightly (or dispatch) after the
merge, so until it exists link the workflow's run list,
`https://github.com/alexknips/hivemind/actions/workflows/kuzu-nightly.yml`.

If the `ubs` job's critical count is nonzero but entirely explained by
category-14 (dependency hygiene) findings that `.cargo/audit.toml` already
allows, say so instead of treating it as a failure, e.g.:

```text
ubs: CI (draft validation PR <url>, `ubs` job green @ <sha>; 0 criticals
  outside category 14; N category-14 findings covered by .cargo/audit.toml;
  cargo audit --locked: PASS)
```

Do not submit `verified with <tests>` or another placeholder. If a gate is
skipped because a tool is unavailable, name the skipped gate and the reason.
If the failure is caused by the branch, fix it before submission. If the
failure is pre-existing, file or reference a bead and keep the current bead out
of merge-ready status until the failure is accounted for.

## Refinery Contract

Refinery classifies the rebased diff the same way (docs-only vs code). The
rebased state is what actually lands on `master`, so the evidence is CI on the
rebased head: never the polecat's pre-rebase head, and never a run from before
`master` moved.

- Code diff: refinery pushes the rebased branch, opens (or reuses) a draft
  validation PR, and waits for the CI run for that exact head. Green CI for that
  head is the whole gate: every job in `.github/workflows/ci.yml`, among them
  fmt, clippy and test on each `rust` leg, `ubs`, `dependency-audit`, and the e2e
  legs. Refinery does not re-run the Code Gate Set locally and does not build
  Kuzu (see [Kuzu (Nightly)](#kuzu-nightly)). It then fast-forwards `master`
  onto the verified commit.
- If `master` moves before the fast-forward, refinery rebases again and waits
  for CI on the new head; the earlier run no longer counts.
- Docs-only rebased diffs are unchanged: there is no local gate to re-run for
  them, exactly as for polecats.

If CI is red for the rebased head, refinery must not fast-forward `master`. It
rejects the BR issue back to the polecat pool with the failing job named in both
the notes and `MERGE_FAILED` comment. A clean rejection includes the source
branch, target branch, failing job or command, and the relevant failure summary.

On success, the close reason names the draft validation PR and the rebased head
it verified (`ci: draft validation PR <url>, all checks green @ <sha>`),
alongside the polecat's proof lines.

## Baseline Rule

For polecats, the warning baseline is the target branch state used to start or
rebase the work. For refinery, the baseline is the one committed in
`.github/workflows/ci.yml` on the rebased head, which the `ubs` job enforces.
This keeps warning growth visible and prevents a stale polecat branch from
masking regressions introduced by rebasing.
