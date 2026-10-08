# Agent Decision Capture

Status: shipped. Originally landed under bead `hivemind-claude-codex-agent-capture-tco7`.
Extended in M2 with HTTP API transcript capture (`/v1/ingest`) and a
server-side classifier. See the *HTTP API Capture* section below.

This document covers the write path only. For consulting, verifying, and
contesting decisions that already exist — including the CLI-only
`hivemind-context` plugin, a second agent interaction model alongside MCP —
see [`AGENT_DECISION_CONTEXT.md`](AGENT_DECISION_CONTEXT.md).

HiveMind exposes a noninteractive CLI path for Claude, Codex, and similar coding
agents to record a decision directly into the local ledger:

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options "Direct CLI,MCP server,Git hook" \
  --chose "Direct CLI" \
  --rests-on-assumption "Agents already have shell access to the local ledger"
```

`--options` splits on every comma. A label that contains a comma goes on its own
`--option` flag, which takes one label whole and can repeat; `--chose` repeats the
label exactly:

```bash
  --options "Rename now,Pause" \
  --option "Rename after the comparison, before the first listing" \
  --chose "Rename after the comparison, before the first listing"
```

An `--options` piece that is empty, or starts or ends with whitespace, is refused
with that hint rather than trimmed: the space after a comma is usually a comma
inside one label, and trimming would record its halves as two options. The same
two flags work on `decision.proposed` and `supersede`. MCP `capture_decision` and
REST `POST /v1/decisions` take options as an array, so a comma never splits them.

## What the decision rests on

Every `decision.capture` (and `supersede`) answers "what does this decision rest
on?" with at least one of:

- `--rests-on-decision <description | #N | decision-id>` — a decision already
  made. A description is resolved by the same deterministic resolver as the
  fluent verbs; if it matches more than one decision, or none, the capture is
  refused with the numbered candidates (re-run with `--rests-on-decision '#N'`)
  and nothing is written.
- `--rests-on-evidence <observation>` with `--evidence-source <where>` — something
  observed. Repeatable; sources are index-aligned with the evidence (one each, or
  none).
- `--rests-on-assumption <statement>` — something assumed. Repeatable.
- `--bet [statement]`, optionally `--would-change-if <text>` and
  `--check-by <date>` — nothing yet: a declared bet.

An existing evidence or hypothesis id (`--evidence`, `--hypotheses`) also counts.
`--confidence low|medium|high` records the decider's own stated confidence; omit
it otherwise. The decider's own words are not a grounding; they go in `--quote`.
A capture that names nothing exits 2 with the four ways to answer and writes
nothing. The reply lists `rests_on` (what was recorded) and `premise_stale` (named
decisions already superseded or rejected — the link is recorded, and the
staleness is visible). MCP `capture_decision` / `supersede_decision` and REST
`POST /v1/decisions` take the same answers as a required `grounding` array. Raw
`emit decision.proposed`, classifier ingest, document import and Slack capture do
not ask the question.

### How the capture plugins ask it

The `hivemind-capture` skill, `/hivemind-capture:capture` and
`/hivemind-capture:capture-decision`, the Claude `active-capture` skill, the
repo-local `/capture-decision`, and the Codex bundle all teach the same thing:
answer "what does this rest on?" before writing, and pass the answer as a flag.

| It rests on | Flag (MCP `grounding` kind) |
|---|---|
| a decision we already made, named as you would describe it, or by the `#N` / `decision-...` handle of the decision you consulted before acting | `--rests-on-decision` (`decision`) |
| something observed, with where it was seen: a URL, `file@commit`, a test run, a measurement | `--rests-on-evidence` with `--evidence-source` (`evidence`) |
| something we assume: the statement | `--rests-on-assumption` (`assumption`) |
| nothing yet: a declared bet, optionally what would change our mind and when to check | `--bet`, `--would-change-if`, `--check-by` (`bet`) |

The primary loop is **consult, decide, capture**: ask what is already decided
about the ground you are about to change (the `hivemind-context` plugin's
`situational`, or `recall`), decide, then capture with the consulted decision as
the premise. Consulting only finds what a decision rests on; it never decides
whether to capture. The answer is one open question, and the four kinds are the
common premises rather than a closed list: an answer that fits none of them is
recorded as an assumption with the text as given, never dropped and never forced
into evidence.

**The decider's own words are not a grounding.** A Slack message or chat reply
that drove the decision says who decided and what they said, not what the
decision rests on. Recording it as evidence (`--rests-on-evidence "Alex in Slack:
keep it at 3"`) files the decider as the decision's own evidence: the record then
says the decision holds because someone said so. The words go in `--quote`
(verbatim), paired with `--question` and `--decided-by`; then ask what those words
rest on (the observation behind them, a decision they follow from, an
assumption), and declare a `--bet` when nothing is known.

`--confidence` comes only from the decider's own words ("pretty sure", "just a
guess"); an agent's own certainty is never recorded, and the flag is omitted
otherwise. Saying which question the decision answers is suggested, never
required: `--question`, one line (MCP `question`), with `--quote` when a person's
words answered it. See [Which question a decision answers](#which-question-a-decision-answers).

After a refusal (nothing named, an ambiguous premise, no match) the agent adds the
grounding and re-runs; it never drops the capture. An ambiguous premise is
re-run with `--rests-on-decision '#N'`; a miss means the decision is not
recorded, so the agent answers with what it does have. A named premise that has
since been superseded or rejected is recorded and reported as `premise_stale`, so
a replacement decision names what overturned the old one, not the decision it
replaces.

### Titles and rationale are for outside readers

A title is one plain sentence a stranger understands; the rationale is
readable without the tracker. A tracker or ticket id (a bead, Jira, or GitHub
issue) goes in `--evidence-source` or as linked evidence — never in the
title, and not as the rationale's subject. The `hivemind-capture` skill, the
capture commands, the active-capture skill, and the Codex bundle all teach
this the same way, and the classifier prompt (`src/classifier.rs`,
[`CAPTURE_CLASSIFIER.md`](CAPTURE_CLASSIFIER.md)) carries the same rule for
extracted captures.

### Grounding a decision after the fact

A decision captured without saying what it rests on reads `nothing declared`.
`hivemind ground "<decision description>"` adds it later, with the same grounding
flags as capture (`--rests-on-decision`, `--rests-on-evidence` with
`--evidence-source`, `--rests-on-assumption`, `--bet`; `--evidence` / `--hypotheses`
for existing nodes). The decision is resolved with the same ambiguity gate as
`supersede` (`--id ID` or `--pick N` to settle it), and nothing is written when the
decision or a premise is ambiguous or unmatched, when nothing is named, or when a
premise already rests on the decision being grounded (it would close a loop). The
grounding is append-only and attributed to whoever runs it (`--actor`), with no link
to the decision's proposal, so a reader can tell "added later" from "at capture".
`--confidence` is not accepted: it is the decider's own words at capture. MCP
`ground_decision` takes `description` or `decision_id` plus the same `grounding`
array and returns the same `{outcome: "ambiguous" | "not_found"}` shapes.

The command writes canonical ledger events. The decision proposal and its
fan-out relation events carry:

- `source=agent`
- `actor_id=agent:<tool>:<name>` unless `--actor-id` is provided. `<name>` is
  a stable identity, not a raw session id: Gas City's `GC_AGENT`/`GC_ALIAS`
  is checked before Claude or Codex session variables (which are freshly
  generated on every run and would otherwise make the same physical agent
  look like a different actor after every restart, hivemind-zdsh.9);
  `--agent-tool` and `--agent-session` are explicit overrides. `<tool>` is the
  `--agent-tool` flag, else `HIVEMIND_AGENT_TOOL`/`HIVEMIND_TOOL` or the Claude
  or Codex session variables, else `unknown` -- never a guessed `codex`. The MCP
  server (`hivemind mcp`) goes one step further when none of those names the
  agent: it takes the tool from the client's own `initialize` handshake
  (`clientInfo.name`: Claude Code -> `claude`, Codex -> `codex`, Cursor ->
  `cursor`, any other client under its own name, folded to the actor-id
  charset), so `claude mcp add hivemind -- hivemind mcp` files captures as
  `agent:claude:<name>` without the flag (hivemind-tiu9). Events already
  recorded are not renamed.
- `source_ref` set to the raw per-run session id (provenance) when one is
  available and no explicit `--agent-session`/`--actor-id` was given,
  otherwise `<actor_id>`, unless `--source-ref` is provided

A person running the command themselves is not an agent, and the command does
not record them as one. With none of `--source`, `--actor-id`, `--agent-tool`,
`--agent-session` or `--source-ref` given, the CLI's `--actor` is recorded as
the actor, with `source=human` for a `human:<name>` id and `source=agent` for an
`agent:<tool>:<name>` id, when either

- `--actor` was typed on the command line (`hivemind --actor human:alice emit
  decision.capture ...`), or
- the environment shows no agent at all (no tool or session variable, no Gas
  City identity), so the actor is the CLI's default: `HIVEMIND_ACTOR`, then
  `human:<git config user.email>`.

An agent stays an agent: with an agent in the environment and no `--actor`
typed, the `agent:<tool>:<name>` derivation above applies. The five provenance
flags are the fuller form the capture plugins pass and win over `--actor`.

Use `--evidence` and `--hypotheses` with existing evidence and hypothesis ids
when the decision depends on already captured context.

`--chose <option>` means the decision was already made: the command
self-accepts it immediately after proposing, from `--actor` (or from
`--decided-by <actor-id>` when the actual decider differs from the recording
actor). Pass `--still-proposed` to keep a genuine open recommendation at
`proposed` instead of self-accepting it.

`supersede` follows the same rule: `--chose <option>` on a replacement means
the replacement was already decided, so it is accepted from `--actor` right
after the supersession is recorded, and `--still-proposed` keeps it open. A
supersede that names no chosen option decides nothing and leaves the
replacement `proposed`. MCP `supersede_decision` and REST supersede take
`still_proposed` with the same meaning.

### Who decided: three cases

`--decided-by` and `--delegated-by` are how the ledger tells three different
situations apart (Alex's attribution ruling, hivemind-zdsh.3 / zdsh.6):

| Situation | Capture | What the record shows |
|---|---|---|
| The agent asked and a human chose | `--decided-by human:<name>` | the human decided; the agent is the recorder |
| A human delegated a scope and the agent decided within it | `--delegated-by human:<name>` | the agent decided, with the delegating human on the record |
| The agent decided alone | neither flag | the agent decided; nothing says a human sanctioned it |

`--delegated-by` is recorded on the agent's own `decision.accepted` event and
projected as a `delegated_by` property on the Decision node. It is not a new
node or edge kind, and it does not change the authorship or review shapes: an
agent deciding under a delegation still derives `agent_only` +
`self_accepted`, and `delegated_by` is the extra fact that separates it from an
agent deciding alone. `hivemind digest` prints it as a `Delegated by:` line
under `By:`; `query verify` prints `delegated by:` and returns
`decided_by.delegated_by`; `get_decision_context` returns `delegated_by`; the
Markdown decision-log export (`hivemind export --format markdown`) appends
`(Delegated by: human:<name>)` to the decider in the `Decided by` cell of each
`INDEX.md` row and adds a `Delegated by:` line to the decision's Provenance
section; and the failure-attribution report splits agent self-accepted
decisions into a `delegation` dimension (`delegated` vs `agent_alone`). A
decision with no marker has no `delegated_by` key at all, never a null, and
the export prints no `Delegated by` text for it.

Rules enforced at write time (CLI, MCP `capture_decision`, REST), each refused
before anything is appended:

- `--delegated-by` must name a `human:<name>` actor.
- The recording actor must be an `agent:<tool>:<name>` actor deciding for
  itself: a human recorder, or a `--decided-by` naming anyone but the recording
  agent, is refused (a human who decided is recorded with `--decided-by`).
- It requires `--chose` and conflicts with `--still-proposed`: a delegation
  qualifies a decision the agent already made, not an open recommendation.
- On the command layer, `Commands::accept_decision_delegated` additionally
  requires that the accepting agent is the decision's own proposer. A delegation
  never attaches to someone else's proposal; a human or peer accepting an
  agent's proposal is recorded plainly.

A standing delegation ("Alex delegated small dependency bumps to me") is not
a record of its own: the agent repeats the same `--delegated-by` value on every
capture within that scope. Old events without the field replay unchanged.

When the decision comes from a human's verbatim answer to a question you
asked, use `--quote` and `--question` together instead of folding the quote
into `--rationale`: `--quote "1a" --question "Should a personal project be
visible to the whole tenant?"`. `--quote` is the decider's own words,
self-contained; `--question` is what those words answer, spelled out in your
own words — not a bare reference like `"1a"` that only makes sense next to
the source conversation. A quote needs its question: a quote with no stated
question is unreadable once the source conversation is gone (hivemind-zdsh.13).
The question stands alone (hivemind-zdsh.16). `--rationale` still carries the self-contained summary of
why, independent of any quote.

`--rationale` is refused, on every capture surface (CLI, MCP, REST), unless
it stands on its own: at least 20 characters and 4 words, and free of a bare
reference into a numbered list that exists only in the source chat — the
same "1a"/"2. a" shape `--quote`/`--question` exist to carry instead
(hivemind-763i, follow-up to hivemind-zdsh.13). A figure is not a list
reference: a number followed by `k`, `m`, `s`, `h`, `d`, `w`, `y` or `x`
(`28k`, `$4.7k`, `167h`, `3d`, `10x`) passes (hivemind-ukd8). If the rationale
legitimately needs one of those tokens (e.g. quoting someone else's outline),
pair `--quote`/`--question` rather than folding it into `--rationale`.

## Which question a decision answers

`--question "<one line>"` (MCP `question`) names the question the decision
answers. It is never required, and it does not need a `--quote`. The text is
kept on the decision exactly as written, and it also names a **question node**:
two captures whose question is equal after lowercasing, collapsing whitespace
and dropping trailing punctuation share one node. The match is exact and
deterministic (no ranking, no model); a question worded differently is a
different question. The reply's `question_id` names the node.

A question someone asked with `hivemind ask` is a request that stays on the waiting list
(`hivemind query get_waiting_requests`) until a decision answers it. Answer it with
`--answers <request id>` (MCP `answers`; `capture.sh` forwards it) in place of `--question`: the
decision is linked to the request's question, the request leaves the list, and `why` shows
`asked_at` beside `answered_at`.

What sharing buys, in every answer:

- `also answered by: <title> [status]` under the question, one line per other
  decision that answers it (a superseded answer stays listed: it is the history
  of the question).
- Two **accepted**, not superseded decisions that answer one question and
  chose different options are flagged on both as `conflicting answer`. Neither
  is marked stale and nothing is resolved: it is a disagreement to settle, not
  a failure of either decision.
- `situational` shows a newer accepted answer to the question of a decision it
  matched.

A decision captured without a question, or before question nodes existed, is
linked afterwards with `hivemind ground "<decision>" --answers "<question>"`
(MCP `ground_decision` with `answers`), attributed to whoever runs it. A
decision answers one question: naming a different one than it already answers
is refused, and nothing is written.

## The session directive

An installed plugin does nothing for an agent that is never told to use it: with the
`hivemind-capture` plugin and its MCP server connected, agents given no instruction
recalled and captured 0 times in 31 headless benchmark sessions (hivemind-wdwg). So the
plugin's Claude Code `SessionStart` hook adds this to every session's context:

> HiveMind ledger: before you act, call the `hivemind` MCP tool `recall_decisions` (q =
> three or four key words about the task) and follow the earlier decisions that still
> hold. Call `capture_decision` (title, rationale = the why, options, grounding) for: a
> rule learned from something that failed or cost you (save it the first time, at once);
> a design or interface choice later work must follow; a reversal of an earlier decision
> (use `supersede_decision`). Not for routine status, session summaries, or plans that
> hold only for this session.

It says what is worth saving, not only to save: told to record "decisions", benchmark agents
saved a summary of each session and missed the rule that cost them something (hivemind-dd95).
It is a fixed text of about 540 characters (held under a 150-token budget by a test), it reads
no ledger, and it fails open: no `hivemind` binary means it prints nothing and the session
starts as usual. Turn it off with `HIVEMIND_DIRECTIVE_DISABLE=1` (in the environment, or under
`env` in `.claude/settings.json`).
The Codex package carries the same hook; Codex runs it only after you trust it. The text, the
Codex trust step and the settings are in the
[plugin README](../plugins/hivemind-capture/README.md#session-directive).

## Questions asked with AskUserQuestion

In Claude Code an agent puts a question to the person by calling the
`AskUserQuestion` tool, an act with a time. The `hivemind-capture` plugin hooks it
(hivemind-bbnw.5), so the ledger records when the question was asked and when it
was answered, with no model in the loop:

- **`PreToolUse`** writes one ask per question (`hivemind ask`, MCP
  `request_decision`): the question, the asking agent, and the time of the tool
  call. The request is `waiting` until a decision answers its question.
- **`PostToolUse`** writes one decision per answered question, recorded by the agent
  and **decided by the person** (`--decided-by human:<git email>`, or
  `HIVEMIND_HUMAN_ACTOR`). Options are the ones offered, chosen is the one picked;
  several picks become one combined option (`Search + Export`) and words of their
  own go in `--quote`. Words (or, when nothing was picked, the note) choose an
  offered option when they lead with its name and name no other offered option;
  words that name no single option are themselves the chosen option, with no offered
  option listed beside it, because the record lists an option only to say it was
  turned down. The placeholder Claude Code reports for a note-only answer
  (`(notes only)`) is never quoted as their words, and a note-only answer with no
  note records nothing. The
  rationale is the note the person wrote on their pick (the tool's
  `annotations[question].notes`), word for word; a note the write layer refuses as
  too short to read on its own goes in `--quote` instead, and the rationale says a
  note is attached. Only with neither a note nor words of their own does it say no
  reasons were given. It rests on a bet with nothing declared, because nothing was
  stated; `hivemind ground` adds what the answer rests on later.

The decision's topic is the slug of the question's header. A registered project
accepts only topic keys it declared (the vocabulary rule, under
[Which project a capture lands in](#which-project-a-capture-lands-in)), so when the
project has not declared the header's key the hook writes the answer once more under
the fixed key `claude-code-question` and declares only that key (MCP
`declare_topics`). It retries on that refusal alone and logs both attempts; every
other refusal is logged and not retried. From these hooks a project's vocabulary
grows by at most one key, never one per question. A personal or unregistered
checkout has no vocabulary, so the header's key is used and nothing is declared. The
person's answer is recorded in every checkout.

The decision names the same question as the ask (`--question`, not `--answers`: a
request id only resolves to its question's text, so the two hooks share no state),
which is what makes `hivemind why` show `asked_at` beside `answered_at` and takes
the request off the waiting list.

Only real asks are written. The tool call is the act and its time is the ask's
time: nothing is back-dated by guess, and an agent deciding alone, a person
deciding unprompted, or an import writes no ask. A question the person declines or
never answers has no answer step, so it stays `waiting`. Speed is that waiting
list plus each decision's own two times; nothing averages them and nothing is
figured per person. Agents therefore should not capture the same answer again with
`capture.sh`: a second decision on the question reads as `also answered by`. Setup,
settings (`HIVEMIND_API_URL` for a hosted server) and failure behaviour are in the
[plugin README](../plugins/hivemind-capture/README.md#askuserquestion-hooks).

## Which project a capture lands in

Every decision belongs to exactly one project inside its tenant (the model is in
[`MULTI_TENANCY.md`](MULTI_TENANCY.md#projects-inside-a-tenant)). A capture
says which project and how that was determined; HiveMind checks the handle and
records both, and never works the project out itself. Evidence and hypothesis
captures carry no project.

**Naming it.** `--project <handle>` on `emit decision.capture`,
`emit decision.proposed`, and `supersede`, and `project` on MCP
`capture_decision` and `supersede_decision`, name a registered project outright.
`--project-source` (MCP `project_source`) says how the caller got the handle:
`stated` (the default), `folder_marker`, `rig`, `current_project`, or `job`, and
it needs a `--project` to describe. A handle that is not registered is refused
before anything is written, so nothing is recorded (no decision, and no evidence or
assumption named on the same call), and the refusal carries the command that fixes
it:

```text
error: invariant violated: project not registered: nosuch -- register it first with `hivemind project register nosuch`
```

A caller may not state a `personal:` address, nor claim `personal_fallback`,
`moved` or `inherited` next to a handle: HiveMind records those three itself.

**Not naming it.** With no project, a capture never fails. It is saved to the
recorder's personal project (`personal:human:alex`, or `personal:agent:claude`
for an agent, one per tool and never per session), and every CLI and MCP reply
says so:

```text
saved to your personal project; pass a registered project handle to file it under a shared one
```

That sentence is the fallback notice: the CLI and MCP never file into a personal
project silently, and the decision can be moved later (see below). REST replies
do not carry it (see the HTTP paragraph).

**Working it out from where the agent is.** `--project-from-context` on the same
CLI verbs, and on the stdio server (`hivemind mcp --project-from-context`, which
the capture plugin's `.mcp.json` sets when the installed CLI lists the flag),
makes the client work the project out from its surroundings when none was named.
First match wins, and each rung records how:

| # | Source | `project_source` |
|---|---|---|
| 1 | `--project` (a named project always wins) | `stated` |
| 2 | the `.hivemind-project` markers of the files the uncommitted change touches (working-tree diff plus the staged set); when none of them sits under a marker, the nearest marker walking up from the working directory | `folder_marker` |
| 3 | the project anchored to the rig in `GC_RIG` (`hivemind project anchor --kind rig`) | `rig` |
| 4 | the actor's current project (`hivemind project use <handle>`) | `current_project` |
| 5 | none of these: the personal project, with a reminder (a session in a rig no project here is anchored to is refused instead; see below) | `personal_fallback` |

A marker is a one-line file holding one handle. A nested marker names a
sub-project and, being nearer, wins over the outer one. A marker that cannot be
read, is empty, or holds more than one word is refused, never skipped, so a
capture is not filed under some other project than the folder says. A marker
naming an unregistered handle is refused like any unregistered handle when it is
the one project the capture resolves to; inside a change that spans several
markers it shares a parent with nothing, so the decision lands in the personal
project and the reply names it. The flag is opt-in
and off by default, so a bare `hivemind emit` stays deterministic. It needs a
CLI that has it; an older one refuses the flag. The stdio server resolves from its
own working directory, the folder it was started in: an agent's later `cd` in its
shell does not change it.

The current project is a per-machine setting kept in `--hivemind-dir`
(`current-project.json`), keyed by tenant and by the CLI's `--actor`; it is not a
ledger fact.

**A change that spans projects.** When the files a change touches sit under
markers of several projects, the decision is made once, for the nearest project
they are all `part_of` (a project counts as part of itself, so a change across a
project and one of its sub-projects belongs to the outer one), recorded as
`folder_marker`, and the reply says so. It names handles, sorted:

```text
recorded for platform: this change spans auth and billing
```

With no project in common, the decision is saved to the recorder's personal
project (`personal_fallback`) and the reply names what it spans and how to fix
it. It is never refused and never dropped, because losing a decision is worse
than misfiling it and a move puts it right:

```text
this change spans auth and billing, which share no parent. Move it with hivemind move ..., or register a parent.
```

**When no folder is attached.** With `--project-from-context` and nothing found,
the personal fallback comes with one more line, so the way out is next to the
problem:

```text
this folder is not attached to a project yet; register the project (hivemind project register <handle>) and commit a .hivemind-project file holding the handle in this folder
```

The line is the whole recipe. Attaching a folder is two steps: register the
project (`hivemind project register billing`), then commit a `.hivemind-project`
file containing `billing` at the folder root. `hivemind project anchor --kind
folder` does not attach a folder: it only records a fact about the project,
nothing reads a folder anchor back and no command writes the marker. `--kind
rig` is the one anchor a capture does read, for a Gas City rig.

**A rig this ledger has no project for.** With `--project-from-context`, a session
that runs in a Gas City rig (`GC_RIG`) whose rig no project in the ledger is
anchored to is not an unattached folder: it is a capture heading for a ledger that
does not know the place it comes from. That is refused, not filed under the
personal project, and nothing is written. The refusal names the rig and the ledger
(the tenant and the local directory or shared database, never a credential) and
the three ways out:

```text
error: invalid command line input: this session runs in rig `beadline` (GC_RIG), but no project in tenant `hivemind` in the local ledger under ./hivemind is anchored to that rig, so this capture would be filed under your personal project in what looks like the wrong ledger. Write to the ledger that holds the rig's project (--tenant or HIVEMIND_TENANT, --hivemind-dir, or --database-url), anchor the rig here with `hivemind project anchor --handle <project> --kind rig --value beadline`, or name the project yourself with --project <handle>.
```

Only the personal fallback is refused: a folder marker, a rig anchor, or a current
project still wins first (the current project is kept per tenant, so it matches
this ledger). A session outside any rig is the ordinary unattached-folder case
above. A supersede is never refused for this: its project is the replaced
decision's, which is in this ledger by construction. HiveMind still never works
the project out; the client does, and only asks the registry what is anchored.

**Topic keys and the project's vocabulary.** A topic says what a decision is
about; without a vocabulary every capture invents its own keys and recall by topic
becomes a lottery. Keys are normalised to lowercase kebab (`Pricing Model` becomes
`pricing-model`). A registered project has a vocabulary: the keys someone declared
for it. A capture filed under a registered project may use only those, so a new
key has to be declared, and the capture says so:

```bash
hivemind emit decision.capture --project billing \
  --topic-keys pricing,seats --declare-topic seats ...
```

`--declare-topic` (MCP `declare_topics`) is on `emit decision.capture`,
`emit decision.proposed`, and `supersede` (MCP `capture_decision` and
`supersede_decision`). Each key must be one of the capture's own `--topic-keys`,
and the capture must be filed under a registered project. Each declared key is its
own recorded fact (`project.topic_declared`, by the capturing actor, just before
the decision), and the reply lists them: `declared_topics` in `--json` and MCP
replies, and in text mode on stderr next to the project line:

```text
project: billing (stated); declared topics for billing: seats
```

A key the project already has is not declared twice, and a capture that declares
nothing does not mention it. A capture that uses an undeclared key and does not
declare it is refused before anything is written, naming the keys, what the
project has declared, and how to declare:

```text
error: validation failed: topic `seats` is not declared for project billing (declared: pricing). To add it, say so in this capture (--declare-topic seats, or `declare_topics` over MCP) or declare it first with `hivemind project declare-topic billing seats`; otherwise use a declared topic.
```

A new project has an empty vocabulary, so its first capture declares every key it
uses. To declare keys without a capture, or to see them:

```bash
hivemind project declare-topic billing pricing seats
hivemind project show billing          # topics=pricing,seats
```

A project that already had decisions before it had a vocabulary adopts what they
use in one step: `hivemind project declare-topic billing --in-use` declares every
topic key the decisions now in that project carry, each as its own recorded fact.
Nothing removes a key. A personal project, and a capture that names no project,
has no vocabulary: any key is accepted there, and asking to declare one is refused.
A move never checks the destination's vocabulary, because correcting where a
decision lives must not be refused for the keys it was captured with.

**What the reply says.** Every capture reply names the project and how it was
determined. In text mode stdout stays the decision id (followed, as before, by a
`premise_stale:` line for each named premise that has since been superseded or
rejected) and the announcement goes to stderr, so `id=$(hivemind emit ...)` is
unchanged by projects:

```text
project: billing (stated)
decision-56e41007-81bd-4589-894e-f609b2a34f0b
```

`--json` and MCP replies carry it in-band: `project`, `project_source`, on
fallback `project_notice` (the sentence above), and, when the project was worked
out from context, `project_reminder` (the unattached-folder line or the
spanning sentence). `supersede` prints ` project=<handle> project_source=<how>`
on its existing key=value line. A superseding decision inherits the project the
decision it replaces is filed in now (after a move, the project it was moved to)
and records `project_source` `inherited`, unless a project is named or worked out
from context; when context finds nothing, or the change spans projects with no parent
in common, a supersede stays in that project instead of falling back to the
personal project. The one exception is a decision in a personal project: that
address belongs to one actor and cannot be stated or moved into by anyone else,
so its replacement is saved to the recording actor's own personal project, with
the usual notice.

**Over HTTP the project is an argument, or absent.** MCP-over-HTTP
`capture_decision` and `supersede_decision` take `project` and
`project_source`. The HTTP server never infers one, because it has no view of the
caller's working directory; only the CLI and the stdio server fill it in from
context. REST takes none: `POST /v1/decisions` records the decision in the
actor's personal project, and its reply carries no `project` or fallback notice.
REST supersede takes none either and inherits the project the replaced decision is
filed in now, so under a registered project its topic keys must already be declared
(REST takes no `declare_topics`; declare them with `hivemind project declare-topic`
first).
Name the project over MCP instead. A decision recorded through REST can be moved
afterwards. Decisions the classifier extracts from ingested transcripts are not
`decision.proposed` events, so `hivemind move` cannot move them yet.

**Fixing a wrong project.** `hivemind move "<description>" --to <handle>`, or
MCP `move_decision`, moves a decision by describing it, with the same ambiguity
gate as `supersede`. The move is recorded and reversible; see
[`AGENT_FLUENT_QUERYING.md`](AGENT_FLUENT_QUERYING.md). The decisions that
landed in a personal project and were never shared are listed by
`hivemind project decisions personal:<actor>`.

## Claude

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --title "Keep capture in the commands layer" \
  --rationale "The write path should validate and append events without query-time inference" \
  --topic-keys agents,capture \
  --options "Direct CLI,MCP server" \
  --chose "Direct CLI" \
  --rests-on-assumption "The write path stays deterministic without query-time inference"
```

This repository also ships a project-local Claude Code command:

```bash
/capture-decision --title "Keep capture in the commands layer" \
  --rationale "The write path should validate and append events without query-time inference" \
  --topic-keys agents,capture \
  --options "Direct CLI,MCP server" \
  --chose "Direct CLI" \
  --rests-on-assumption "The write path stays deterministic without query-time inference"
```

The command calls `.claude/scripts/capture-decision.sh`. By default it records
manual slash-command captures as `actor_id=human:<git-user>` with
`source=human`. Pass `--source agent` when Claude Code is recording an
autonomous agent decision; that uses `agent:claude:<name>` (a stable
identity, not a raw session id) and `source=agent`.

### Claude Code Distribution Bundle

This repository also ships a Claude Code marketplace at
`.claude-plugin/marketplace.json`. The marketplace exposes
`plugins/hivemind-capture` as the `hivemind-capture@hivemind` plugin.

Install from Claude Code:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

The repository-level `.claude/settings.json` advertises that marketplace and
enables `hivemind-capture@hivemind` so trusted checkouts prompt contributors to
install it. The plugin includes:

- `/hivemind-capture:capture-decision`, which defaults to
  `actor_id=agent:claude:<name>` (a stable identity, not a raw session id)
  and prints a one-line confirmation plus a query suggestion.
- `/hivemind-capture:query-decisions`, which answers "what did we decide
  about X?" via the fluent `query recall` verb — free text first, never a
  decision id. For single-decision follow-up (rationale, still-holds check,
  contest, supersede) see the `hivemind-context` plugin above.
- `.mcp.json`, which wires the `hivemind` MCP server to `hivemind mcp`.
- A `SessionStart` hook that tells every session to call `recall_decisions` before
  acting and `capture_decision` for a rule learned from a failure, a design choice later
  work must follow, or a reversal, never routine status (see
  [The session directive](#the-session-directive); off with
  `HIVEMIND_DIRECTIVE_DISABLE=1`).
- The `hivemind-capture` skill for durable decision boundaries, provenance
  rules, and the question every decision capture answers: what does it rest on.

The default backend is the project-local `./hivemind/` directory. The bundled
MCP descriptor pins that location for agents launched from this checkout. For
slash-command captures that need another ledger, set the plugin option
`hivemind_dir`, export `HIVEMIND_DIR`, or pass `--hivemind-dir` to the command.

## Codex

```bash
cargo run -- --hivemind-dir ./hivemind/ emit decision.capture \
  --title "Prefer direct CLI capture before MCP" \
  --rationale "Codex can invoke the same local command in any checkout" \
  --topic-keys agents,capture \
  --options "Direct CLI,MCP server" \
  --chose "Direct CLI" \
  --rests-on-assumption "Codex can run a local command in any checkout"
```

### Codex Distribution Bundle

Codex exposes several extension surfaces relevant to HiveMind capture:

- `AGENTS.md` gives Codex repository and global instructions before work starts.
  This is useful for pointing contributors at HiveMind capture guidance, but it
  is not an installable transport. See
  <https://developers.openai.com/codex/guides/agents-md>.
- Skills package reusable instructions, resources, and optional scripts. Codex
  can invoke them explicitly or choose them by description, and it can read
  skills from repo, user, admin, and system locations. See
  <https://developers.openai.com/codex/skills>.
- Plugins are the installable distribution unit for reusable Codex workflows.
  They can bundle skills, apps, MCP servers, and lifecycle configuration. See
  <https://developers.openai.com/codex/plugins> and
  <https://developers.openai.com/codex/plugins/build>.
- Hooks can run deterministic scripts during the Codex lifecycle, but matching
  hooks can run concurrently, non-managed command hooks require trust review,
  and plugin hooks are off by default unless enabled. Hooks are therefore not
  the primary capture path. See <https://developers.openai.com/codex/hooks>.
- MCP connects Codex to third-party tools and context in the CLI and IDE
  extension. It is a good future interface for a shared HiveMind service, but
  local capture does not require MCP setup. See
  <https://developers.openai.com/codex/mcp>.

This repository ships `plugins/hivemind-capture`, exposed through
`.agents/plugins/marketplace.json`. The plugin bundles the
`$hivemind-capture` skill, which keeps the direct CLI as the write path, asks the
same "what does this rest on?" question as the Claude skill, and uses the same
actor-id convention as Claude: `agent:codex:<name>` and `agent:claude:<name>` (a
stable identity, not a raw session id).

Install from a HiveMind checkout by starting Codex in the repository, opening
`/plugins`, choosing `HiveMind Plugins`, and installing `HiveMind Capture`.
Install from another machine by adding the repository marketplace first:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

For instruction-only use, copy
`plugins/hivemind-capture/skills/hivemind-capture` to `$HOME/.agents/skills/`
and invoke `$hivemind-capture`.

The skill is backend agnostic: it always execs the `hivemind` binary
directly, so it inherits whatever backend the calling environment selects.
`HIVEMIND_DIR`/`--hivemind-dir` only ever names a SQLite ledger path (the
default local `./hivemind`, or another directory on shared storage); it does
not select Postgres. To connect the skill's capture and query commands to a
shared Postgres backend instead, set `HIVEMIND_DATABASE_URL` (or
`--database-url`) plus `HIVEMIND_TENANT` in the environment the skill runs
in — every `hivemind` subcommand it shells out to honors those the same way
`hivemind serve` does. The capture verb and query behavior stay the same
either way.

## HTTP API Capture

Status: shipped in M2 as a second, parallel capture path alongside the
explicit CLI.

The HiveMind HTTP API exposes `POST /v1/ingest` to accept batches of agent
transcript turns. Two Python clients ship in `capture/`:

**Hook shipper** (`capture/hook_ship.py`): invoked by Claude Code
`PostToolUse` and `Stop` hooks. Reads the session JSONL transcript at the
cursor, ships new turns to `/v1/ingest`, and exits 0. Never blocks or raises
— hook failures must not affect the agent.

Example `.claude/settings.json` configuration:

```json
{
  "hooks": {
    "PostToolUse": [{"matcher": "", "hooks": [{"type": "command", "command": "python3 /path/to/capture/hook_ship.py"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "python3 /path/to/capture/hook_ship.py"}]}]
  }
}
```

**Sidecar daemon** (`capture/sidecar.py`): polls `~/.claude/projects/` for
JSONL mtime changes and ships new turns. Intended for hook-less harnesses
(Codex) and as a durability backup. Run as a background process:

```bash
python3 capture/sidecar.py &
```

Both clients configure the server URL and auth token via environment:

```
HIVEMIND_API_URL    Base URL of the HiveMind server (default: http://localhost:8080)
HIVEMIND_API_KEY    Bearer token hm_tk_... (optional in dev mode)
HIVEMIND_AGENT_TOOL Override agent_tool field (default: claude)
```

**Server-side classifier**: the server runs an optional background Layer-3
worker (`src/classifier.rs`) that reads `ingest.batch_received` events and
annotates them with `ingest.batch_classified` events via Haiku 4.5. The
worker exits immediately when `ANTHROPIC_API_KEY` is absent — the rest of
the system stays correct without it. See
[`CAPTURE_CLASSIFIER.md`](CAPTURE_CLASSIFIER.md) for the classifier design.

**Who decided** (hivemind-u70b): both classify workers, this one and the agent-seat
`classify-queue` command, fill `accepted_by` and `rejected_by` only for an actor the text
shows deciding, and `actor_id` only for who proposed, made or reported the item. A proposal,
a report of a call, a call relayed without its decider, or an agent carrying out its own
choice leaves no acceptor, so the decision reads `proposed` until a person is shown deciding
it. The rule and its fixtures are in [`CAPTURE_CLASSIFIER.md`](CAPTURE_CLASSIFIER.md).

**Restatements** (hivemind-83cj): every session that relays or re-reads a ruling
can classify it into a fresh decision. After extracting a decision, the worker
shows a model the closest recorded decisions (the same read `recall` answers
from) and asks which, if any, the new decision restates: the same choice, made
again or relayed once more. A yes puts that id in the capture's `restates_id`;
the model never names an id it was not shown, and any failure leaves the
capture as extracted. The agent-seat path (`classify-queue`) does the same with
`query recall` on each decision's title. The write layer judges nothing: it
checks `restates_id` names a recorded decision (refusing the classification
otherwise), records the capture as its own decision, and the projector links it
`SAME_AS` to the one it restates, so `recall` and `why` show one decision with
`also_recorded_as`. The one mechanical exception: a capture restating a decision
classified from turns with the very same newest source time (`ts`) is the same
moment seen again, a re-ingested transcript, and is not recorded a second time;
the reply's `restated` lists every such capture as `linked` or `deduplicated`.
Turns without a source time never count as the same moment. For decisions
recorded before this, `hivemind restatements propose` lists the links to
approve (a title-word overlap, with the shared words printed) and `restatements
apply` records the ones you confirm; nothing is deleted or rewritten. On a cell, where the
database password is not an agent's to hold, the same two steps run over HTTP with the caller's
token (hivemind-h4kr): `GET /v1/restatements/proposals[?project=][&limit=][&cursor=]` lists the
proposals a page at a time and writes nothing, and `POST /v1/decisions/{id}/restatements` with
`{"restates_id": "<the earlier decision>"}` records ONE link as the token's actor. A pair
already linked writes nothing, an id that is not a recorded decision is refused, and there is
no route that links every proposal: a wrong link cannot be undone for reads, so each one is
named.

**Source time and asks** (hivemind-bbnw.8): the classifier sees each turn's id and may name,
on a capture, the turn it came from (`source_turn_id`) and, on a decision or a
decision-request, the question in the words it was asked in (`question`). The write
layer refuses a turn the classified batches do not hold, reads that turn's own `ts` into
`source_ts` (never from the caller), and records a decision at that time instead of when it was
classified. A decision whose capture names no turn, or a turn with no `ts`, reads at the newest
turn time of its batch when the batch's turns carry times (hivemind-ohik; the graph reads it
from the received batch, so a session classified days later still reads as when it was said),
and at the classification's time only for a batch with no turn times at all. A
decision-request with a question and a dated turn writes one `question.asked` at that turn's
time, by the actor named on the request (with none named, an assistant's turn is credited to
the batch's submitter and a user's turn writes no ask); a decision stating the
same question (after the normalization `capture --question` uses) links to it with `ANSWERS`,
so `why` shows both times. An agent deciding alone writes no ask, and no earlier "first
raised" time is inferred. The same transcript ingested again writes its ask once.

**Agent-shaped tokens**: `POST /v1/users` (and `.../tokens`) always mint a
`human:<email>` token, since agents aren't users with an email or role. An
admin mints a token bound directly to an agent identity instead, with
`POST /v1/agent-tokens` (admin-gated, same as `/v1/users`):

```bash
curl -s -X POST "$HIVEMIND_API_URL/v1/agent-tokens" \
  -H "Authorization: Bearer $HIVEMIND_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"agent_tool": "claude", "agent_name": "crew-gastown", "label": "gastown crew pane"}'
```

Returns `{"actor_id": "agent:claude:crew-gastown", "token_id": ..., "token_secret": "hm_tk_..."}`.
Every write authenticated with that token — including `/v1/ingest` — is
recorded with `actor_id=agent:claude:crew-gastown`, never a `human:...`
identity (hivemind-zdsh.19). `actor_id` always comes from the resolved
token record, never the `X-HiveMind-Actor` header — a caller cannot spoof a
different actor by setting the header.

This is distinct from crediting *who answered within a transcript*: the
classifier's `actor_id`/`decided_by` extraction (only when a decider is
explicitly named in the text) still attributes individual decisions to a
human who spoke up inside an agent-token session, separately from the
session's own agent actor.

## Keyless classification (Worker A)

`ANTHROPIC_API_KEY` is optional. When it is absent, the server-side background
classifier (Worker B) does not start — the ingest path, ledger, and all queries
remain fully functional.

The `hivemind-capture` plugin ships **Worker A**: a
`/hivemind-capture:classify-queue` slash command that drains the same pending
classification queue using the agent's subscription seat. No API key is needed
on the server.

Install the plugin and run after a session:

```text
/hivemind-capture:classify-queue
```

Pass `--limit N` to cap the number of batches per run (default 20), and
`--session-id S` to scope listing to one ingest session — the run-once-per-
session-end cadence below relies on this so concurrent sessions never
redundantly process each other's queue. The queue persists across runs; large
backlogs drain across multiple invocations.

The server lists the oldest pending batches first and says when it stopped short:
`GET /v1/classify-queue` returns `pending_total` (how many batches match, whatever the limit)
and `truncated`. A runner that serves many sessions and has to choose which to classify first
does not need the queue to choose: `GET /v1/classify-queue/sessions[?limit=]` returns one row
per session with pending batches (`session_id`, `actor_id`, `batch_count`,
`oldest_submitted_at`, `newest_submitted_at`), most recently active first, with
`session_total`, `batch_total` and `truncated` — no turn text. It then fetches that
session's batches with `GET /v1/classify-queue?session_id=`. Both reads cost about the same
however much transcript the ledger holds: the turn text of a batch is read only for a batch
the reply carries.

`hivemind classify-queue list`/`submit` talk to a server over HTTP instead of
opening the local SQLite ledger directly when `HIVEMIND_API_URL` is set (same
`HIVEMIND_API_URL`/`HIVEMIND_API_KEY` variables as the Python capture clients
above) — the agent never needs a direct database credential on a shared cell.
`classify-queue submit --batch-id id1,id2` accepts more than one batch id to
submit a single classification covering several batches from the same session
in one write, matching Worker A's cadence: classify each session once at
session end, not per batch and not on an hourly sweep. A shared daily cap
(`HIVEMIND_CLASSIFY_DAILY_CAP`, default 150) protects against runaway spend
across every session in a city; once hit, `submit` refuses with an error and
the remaining batches stay pending for the next UTC day rather than being
dropped.

Both Worker A and Worker B write identical `IngestBatchClassified` events to the
same queue. Concurrent classification is idempotent — last writer wins per batch.
Setting `ANTHROPIC_API_KEY` later starts Worker B automatically; Worker A remains
available for on-demand draining.

See [`docs/KEYLESS_CAPTURE.md`](KEYLESS_CAPTURE.md) for a zero-to-first-decision
walkthrough that requires no API key.

## Reliability Tradeoffs

Direct CLI capture is the explicit, testable, write-once path for named
decisions. HTTP API ingest (`/v1/ingest`) is the passive background path for
capturing activity transcripts. Skills and instructions improve discoverability,
but they do not guarantee that an agent will call a tool. Hooks are supplemental
because they can be skipped, disabled, or misinstalled. The sidecar daemon is a
durability backstop for hook-less harnesses.

MCP (`src/mcp.rs`, native stdio and `POST /mcp` transports) exposes both write
tools (`capture_decision` and friends) and read/query tools directly against
the core commands/queries layer; see [`MCP_SERVICE_SPLIT.md`](MCP_SERVICE_SPLIT.md).
