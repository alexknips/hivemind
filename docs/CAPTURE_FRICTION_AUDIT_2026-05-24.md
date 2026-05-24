# Capture Friction Audit 2026-05-24

## Summary

Sampled ten substantive Codex sessions in the HiveMind repo after the
`hivemind-dogfood-shared-ledger-via-mcp-8kj4` work was claimed on
2026-05-24. The shared-ledger bead was still open at audit time, so this is a
live post-claim window rather than a completed post-landing window.

Decision capture rate in the sample: **8.3% (1 of 12 decisions)**.

Session-level capture rate: **10% (1 of 10 sessions captured at least one
decision)**.

## Measurement Definition

A measured decision is a non-trivial architectural, design, implementation
strategy, integration, recovery, or scope choice made during a real repo
session where the agent selected a direction from plausible alternatives and
future work depended on that direction.

Excluded from the count:

- routine command sequencing, such as reading files before editing;
- Beads claim mechanics and branch naming unless they changed project behavior;
- ordinary test selection unless it committed to a new validation strategy;
- generated seed/demo ledger events not created by the audited agent session.

A captured decision is a matching HiveMind ledger `decision.proposed` event in
the repo or worktree `hivemind/ledger.sqlite` with a title/source that maps to
the session decision. Transcript mentions alone did not count as captured.

## Sample Window

The exact "after dogfood shared ledger lands" window was not available because
`hivemind-dogfood-shared-ledger-via-mcp-8kj4` was still `OPEN` in BR. To avoid
waiting on future work, this audit sampled the first ten non-current,
substantive Codex HiveMind sessions started after the dogfood work was claimed
on 2026-05-24.

Evidence used:

- Codex transcripts under `/home/ubuntu/.codex/sessions/2026/05/24/`.
- Repo/worktree ledgers under
  `/home/ubuntu/gc/.gc/worktrees/hivemind/**/hivemind/ledger.sqlite`.
- BR state from `/data/projects/hivemind`.

## Results

| Start UTC | Session | Work | Decisions | Captured | Notes |
| --- | --- | --- | ---: | ---: | --- |
| 06:08:19 | `019e5899-41e8-7e11-aa39-636512f31857` | `hivemind-agent-capture-on-decision-r4eg` | 1 | 1 | Captured "Derive Codex capture identity from session context". |
| 06:08:36 | `019e5899-853c-7de0-ad47-2ed2f31cbf6e` | `hivemind-disagree-and-supersede-verbs-ex6l` | 1 | 0 | Chose thin wrappers over existing accepted/rejected/superseded events with reason only on disagreement/rejection. |
| 06:09:26 | `019e589a-458d-75f2-bea7-e8178635f1ab` | `hivemind-dogfood-plugin-actor-convention-trax` | 1 | 0 | Chose a shared identity helper for CLI and MCP defaulting. |
| 06:09:51 | `019e589a-a902-7660-924f-25cc11678626` | `hivemind-dogfood-shared-ledger-via-mcp-8kj4` | 1 | 0 | Chose repo-local Codex MCP config after finding Claude already had repo settings. |
| 06:10:27 | `019e589b-34d8-7f00-a3d4-3cd624ee654a` | refinery recovery | 3 | 0 | Chose the recovery set, fresh worktree isolation, and manual conflict integration over blanket cleanup/ours merges. |
| 06:11:08 | `019e589b-d4b7-7052-9efa-281409f1baeb` | `hivemind-full-text-search-over-decisions-9dst` | 1 | 0 | Chose derived/rebuildable FTS rather than write-path state. |
| 06:11:12 | `019e589b-e1f4-7821-a827-cc9564c227a6` | `hivemind-investigate-multi-tenancy-model-pe6c` | 1 | 0 | Chose `tenant_id` as service-level isolation key with local SQLite as implicit single tenant. |
| 06:11:15 | `019e589b-f12e-7a91-ac86-146c0d5dd4ad` | `hivemind-investigate-search-design-6445` | 1 | 0 | Chose deterministic explicit-graph search for layer 2, leaving semantic inference to layer 3. |
| 06:11:22 | `019e589c-091a-7800-94a8-41a4e8ce85ff` | `hivemind-investigate-sqlite-wal-multiprocess-2okh` | 1 | 0 | Chose a contention reproduction around current WAL/single-insert behavior before deciding on code changes. |
| 06:11:56 | `019e589c-8fca-7093-81c9-e565bb50f081` | `hivemind-quickstart-five-minutes-to-first-decision-524h` | 1 | 0 | Chose a small `quickstart` command plus README path over only expanding prose docs. |

Ledger proof for the single captured decision:

- Path:
  `/home/ubuntu/gc/.gc/worktrees/hivemind/polecats/gastown.furiosa/hivemind/ledger.sqlite`
- Event: `decision.proposed` event 1
- Timestamp: `2026-05-24T06:18:57.110789146+00:00`
- Actor: `agent:codex:019e5899-41e8-7e11-aa39-636512f31857`
- Title: `Derive Codex capture identity from session context`

## Friction Reasons

1. **No automatic in-flow trigger**: 11 missed decisions. Agents regularly
   wrote "I am going to..." or "The key design tension is..." and proceeded
   directly into implementation or drafting. Only one session stopped to emit a
   ledger event.
2. **Decision-boundary ambiguity**: 5 missed decisions. Investigation and
   recovery sessions made durable design or process choices, but they looked
   like ordinary plan narration rather than capture-worthy HiveMind decisions.
3. **Capture path is operationally expensive**: 1 direct capture attempt showed
   high latency. The only successful capture invoked `cargo run --quiet` and
   then waited on compilation for minutes before editing. Several neighboring
   sessions were also waiting on cargo builds or locks, so capture competes with
   the same scarce build path as normal validation.

## Findings

The capture habit is present but not reliable. One session explicitly noticed a
capture boundary and recorded it before editing, proving the workflow can work.
The other nine sampled sessions made decisions of similar durability without a
ledger write.

The shared-ledger dogfood surface was not yet complete during this window. The
captured event landed in a worktree-local ledger, not an audited shared
`/data/projects/hivemind/hivemind/ledger.sqlite`. That means even the successful
capture is hard to discover globally unless the audit scans worktrees.

The current biggest opportunity is not schema expressiveness. It is getting a
low-latency, always-available capture path in front of agents at the moment they
state a durable choice, especially for design/investigation work where no code
edit immediately reinforces that a decision was made.

## Recommended Follow-Up

- Finish `hivemind-dogfood-shared-ledger-via-mcp-8kj4` and rerun the same audit
  on the first ten completed sessions after it closes.
- Add a cheap installed `hivemind` binary or MCP capture path to avoid
  per-capture cargo compilation.
- Add explicit capture-boundary language to the agent instructions for
  investigation/design beads, not only code changes.
