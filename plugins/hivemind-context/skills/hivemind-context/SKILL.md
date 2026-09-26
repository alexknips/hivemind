---
name: hivemind-context
description: Consult, verify, and contest HiveMind organizational decisions from Claude Code or Codex by description — never by id. Use before changing a module with standing decisions, when unsure why something is the way it is, before proposing something that may already have been rejected, or to check whether an old decision still holds. Do not use to capture new decisions (see the hivemind-capture skill) or for chat logs, task tracking, or private scratch memory.
---

# HiveMind Context

## Consult Boundary

Query decision memory before you act on assumptions, not after something
breaks. HiveMind holds durable organizational decisions: what was decided,
why, by whom, which options were rejected, and whether it still holds. This
skill is the read/contest side; use `hivemind-capture` to record new
decisions.

Consult HiveMind when:

- You are about to change a module, file, or area that may carry standing
  decisions.
- You are unsure why something is built the way it is.
- You are about to propose a direction that may already have been
  considered and rejected.
- You want to rely on an old decision and need to know if it still holds.
- You disagree with a decision, or a decision needs to be replaced by a new
  one.
- A decision reads "nothing declared" for what it rests on, and you know what
  it rests on.

Do not use this skill for:

- Capturing new decisions (use `hivemind-capture`).
- Browsing decisions out of idle curiosity with no bearing on current work.
- Inventing a `decision_id` to skip resolution — there is no id-driven
  entry point in this plugin, by design.

## The Agent's Lens — No Ids, Ever

Every command in this plugin takes free text or the current working
situation as input, never a `decision_id`. This is not a shortcut around the
CLI's `--id` flags — they still exist as an escape hatch for scripts that
already have one — it is the primary interface, because an agent about to
touch code does not know a decision's id, only what it is about to change or
what it wants to ask.

- **Situational first** — you don't have to phrase a question. You are
  about to touch `src/api.rs`; the standing decisions that constrain that
  change should surface on their own:

  ```bash
  ${CLAUDE_PLUGIN_ROOT}/scripts/situational.sh
  # or explicitly:
  ${CLAUDE_PLUGIN_ROOT}/scripts/situational.sh --paths src/api.rs,src/mcp.rs
  ${CLAUDE_PLUGIN_ROOT}/scripts/situational.sh --since-branch-point
  ```

  With no arguments it reads the current git diff / staged set. Run this
  before editing an unfamiliar module, and again with `--since-branch-point`
  to catch what changed in HiveMind since you started working here.

- **Question second** — free text, not a decision id:

  ```bash
  ${CLAUDE_PLUGIN_ROOT}/scripts/recall.sh "what did we decide about bearer auth on postgres"
  ```

- **Follow-up by reference, not id** — once you have a decision in front of
  you (from `situational` or `recall`), ask about it by description:

  ```bash
  ${CLAUDE_PLUGIN_ROOT}/scripts/why.sh "bearer auth on postgres"
  ${CLAUDE_PLUGIN_ROOT}/scripts/verify.sh "bearer auth on postgres"
  ```

- **Verify before you rely on it** — an old decision may have been
  superseded, may rest on a since-refuted hypothesis, or may be contested.
  Check before you build on it:

  ```bash
  ${CLAUDE_PLUGIN_ROOT}/scripts/verify.sh "the decision you're about to depend on"
  ```

  Read `still_holds.held_up` and `still_holds.reasons`. A `false` here is a
  real signal, not noise — do not proceed as if the decision still stands
  without accounting for why it doesn't.

- **Push back or move on** — these are WRITE verbs. Never invent an id;
  resolve by description exactly like the read verbs above:

  ```bash
  ${CLAUDE_PLUGIN_ROOT}/scripts/disagree.sh "the decision you disagree with" \
    --reason "why you disagree"

  ${CLAUDE_PLUGIN_ROOT}/scripts/supersede.sh "the decision being replaced" \
    --title "the new decision" \
    --rationale "why the new direction" \
    --topic-keys same,topics --options a,b --chose a
  ```

- **Say what it rests on** — a decision captured without saying reads
  `nothing declared`. If you know what it rests on, add it after the fact.
  Name a decision it follows from, something observed and where, something
  assumed, or — when there is nothing yet — a bet. It is append-only and
  attributed to you, not to whoever captured the decision, so a reader sees
  it was added later:

  ```bash
  ${CLAUDE_PLUGIN_ROOT}/scripts/ground.sh "the decision that rests on nothing declared" \
    --rests-on-decision "the earlier decision it follows from"

  ${CLAUDE_PLUGIN_ROOT}/scripts/ground.sh "the decision that rests on nothing declared" \
    --rests-on-evidence "what was observed" --evidence-source "where: URL, file@commit, test run"
  ```

  The decider's own words are not a grounding, and `--confidence` is not
  accepted here: it is the decider's words at capture. A premise that already
  rests on this decision is refused — it would close a loop.

  A decision captured without saying which question it answers takes
  `--answers "<the question, one line>"`: it links to the question node whose
  text matches (decisions that answer the same question share one), and it may
  stand alone. `verify` then shows `answers:` and `also answered by:` for the
  other decisions that answer it, and flags two accepted answers that chose
  differently.

## The Ambiguity Gate — Never Guess

Every command above resolves your description deterministically (term match
+ topic + recency — no LLM, no similarity, no ranking model; see
`docs/AGENT_FLUENT_QUERYING.md`). If more than one decision matches equally
well, the CLI does not pick one for you. It returns a numbered candidate
list and, for the write verbs (`disagree`, `supersede`, `ground`), performs
**no write**.

When you see an ambiguous result:

1. Read the candidate list; it is the full answer, not a partial one.
2. Re-invoke the same command with `--pick N` for the candidate you mean,
   or a bare `#N` (referring to the most recent ambiguous output), or
   `--id <decision_id>` / `--decision <decision_id>` / `--old <decision_id>`
   if you already know the id. (`ground`'s `--rests-on-decision` premises
   resolve the same way and refuse, with their own candidate list, when they
   are ambiguous.)
3. Never fabricate a `decision_id` to force a resolution. If nothing
   matches (`NotFound`), that is itself the answer — the decision does not
   exist yet, or your description needs different terms.

This mirrors AGENTS.md's honesty standard: no invented confidence. A wrong
guess on a write verb is worse than asking again.

## Quality Rules

- Use the plugin scripts (or the CLI directly) as the read/write transport.
  Do not query the ledger through any other path from this skill.
- Preserve disagreement and staleness in what you report back. Do not
  summarize away `contested`, `superseded`, or `held_up: false` because it
  complicates the answer — that is the signal this plugin exists to surface.
- Never invent a `decision_id`, `evidence_id`, or `hypothesis_id`. If you
  need one, resolve it through a query first.
- `situational` is a ranked list, not a single answer — there is no
  ambiguity gate on it, because there is no single target to disambiguate.
  Read the `matched_via` field on each result to see why it surfaced
  (topic-key match vs. evidence-content overlap) rather than trusting the
  ranking blindly.

## Backend Selection

The default local backend is whatever `--hivemind-dir` points at, normally
`./hivemind/`. To switch to a shared backend, set `HIVEMIND_DIR` to the
shared ledger mount or service-managed directory before running the same
commands, or set the plugin's `hivemind_dir` option. The verbs, actor
format, and ambiguity-gate behavior stay unchanged.

**These scripts only reach a local directory or a database the CLI process
can open directly** — they have no HTTP-client mode and cannot reach a
remote cell's `/v1` API or `/mcp` endpoint (`docs/SELF_HOSTING.md`'s "Using
the CLI / MCP from local agents"). If the cell is reachable only over HTTP,
use the equivalent `mcp__hivemind__*` tool directly instead of the script:

| Script | MCP tool |
|---|---|
| `situational.sh` | `get_situational_decisions` |
| `recall.sh` | `recall_decisions` |
| `why.sh` | `get_decision_neighborhood` |
| `verify.sh` | `get_decision_outcome` |
| `disagree.sh` | `disagree_decision` |
| `supersede.sh` | `supersede_decision` |
| `ground.sh` | `ground_decision` |

The first six are registered on the HTTP transport (`hivemind-ot72.6`
through `.12`) and `ground_decision` is registered on both transports, so this
substitution works with no further product change.
The MCP tools take the same free-text/description arguments as the scripts
— still never a `decision_id` you invented. For the three write verbs,
`actor_id` follows the same rule as `hivemind-capture`'s MCP-over-HTTP
section: pass it explicitly when the server's token is shared across more
than one session, so disagreement/supersession/grounding provenance doesn't collapse
onto the token's own identity or an unstable per-connection session id.
