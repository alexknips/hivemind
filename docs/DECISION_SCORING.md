# Decision Scoring

> **Status: shipped as a quality *profile* and a list of *attention findings*. There is
> no score.** `score_decision` returns the profile of one decision,
> `scan_decision_quality` returns one page of attention findings and `get_suggestions`
> returns the same page without the findings someone has acknowledged, on the stdio MCP
> server, the HTTP MCP endpoint and the CLI. All are read on demand from what the
> ledger states, with no model and no network. No response carries a composite number,
> a tier or a grade. A model's assessment of a decision has a record, a write path, a
> place beside the floors ([below](#a-models-assessment-beside-the-floors)), and two
> optional producers ([below](#producers-of-a-model-assessment)): the background scorer
> (`ANTHROPIC_API_KEY`) and the keyless `emit decision.scored` path. **Deferred, not
> built:** Composite, Confidence, Reputation, Importance (see [Deferred](#deferred-not-built)).

HiveMind records *what* was decided, by whom, with what options and evidence
([`ARCHITECTURE.md`](ARCHITECTURE.md)). The quality profile adds a separate, derived
view: *what the record supports* about how the decision was made, along seven
dimensions, computed after the fact and never mixed into the decision record itself.
It reports what is written down, never that it is sound, and it says "not assessed"
where nothing can be said.

The research basis is [`DECISION_QUALITY_LITERATURE.md`](DECISION_QUALITY_LITERATURE.md).

## Architectural placement (and why it stays in bounds)

The profile is agentic analysis. It lives strictly in **Layer 3**, above the write and
query paths, per [`PRINCIPLES.md` §7](../PRINCIPLES.md) and
[`ARCHITECTURE.md` → Layer Boundary](ARCHITECTURE.md): `src/quality_profile.rs` (the
floors), `src/quality_profile/findings.rs` (attention findings),
`src/quality_profile/report.rs` (what the tools return) and
`src/quality_profile/failure_modes.rs` (the failure-mode analysis by dimension). Four
properties keep it
compliant and trustworthy:

1. **Ex ante.** A level uses **only what was knowable at decision time, never the
   outcome.** It measures how the decision was made, not the luck. Something recorded
   after the decision is shown as `later` and never raises a level; what happened next
   has its own view ([below](#where-the-retired-deductions-went)).
2. **Computed, not reported.** The decider supplies *evidence and artifacts*; the
   profile is derived from them on every read. Clients never report a level, which
   removes the obvious gaming path.
3. **Outside write and query.** Layer 2 "does not call LLMs, rank, cluster,
   summarize, or invent confidence" ([`ARCHITECTURE.md`](ARCHITECTURE.md)). Nothing in
   `src/queries/` or `src/commands/` imports the profile (a test holds that line), so
   queries stay pure and the profile can be replaced without touching ingest or
   queries. It reads the graph through the same read interface as everything else and
   writes nothing.
4. **No model, no network.** The floors run self-hosted with no API key. A model's
   assessment stands beside a floor, stored as an append-only annotation and never as
   an edit to the decision ([below](#a-models-assessment-beside-the-floors)); the floors
   read nothing of it, so a profile with none is complete on floors alone.

## The seven dimensions

Each dimension stands alone. There is no composite over them.

| # | Dimension | What it assesses |
| --- | --- | --- |
| 1 | **Framing** | Was the right problem/question framed? |
| 2 | **Alternatives** | Were genuine alternatives generated and considered? |
| 3 | **Information** | Was the relevant information gathered and used? |
| 4 | **Reasoning** | Is the inference from information to choice sound? |
| 5 | **Values / Tradeoffs** | Were the values and tradeoffs made explicit and weighed? |
| 6 | **Bias exposure** | Exposure to **non-calibration** cognitive distortions: anchoring, confirmation, sunk-cost, framing, motivated reasoning. |
| 7 | **Calibration** | Matching confidence to evidence; acknowledging unknowns; avoiding over- and under-confidence. |

Bias and Calibration are separate by design: Bias covers distortions *other than*
confidence/evidence mismatch; Calibration covers that mismatch. Reversibility is not a
quality dimension: a reversible decision is not lower *quality*, it simply matters less
(see [Importance](#deferred-not-built)).

## Floors: what each dimension can say without a model

Implemented in `src/quality_profile.rs`. A **floor** is the part of a dimension that can be computed from facts the record
already states, deterministically, self-hosted, with no model and no network. A
floor says what is written down, never that it is sound. The profile lists all
seven dimensions for a decision. Each one is either

- **assessed**: an ordinal `level` (`none`, `partial`, `solid`), the `reasons`
  behind it and the `node_ids` they rest on; or
- **not assessed**: a `why`, and no level. A dimension with no basis says so
  rather than guessing.

There is no composite number, no tier and no grade in the profile: each
dimension stands alone, and the `attention` list beside them names what deserves a
second look. `none` means the record states nothing toward the
dimension, not that the decision was bad. Every profile records the
`floor_version` of the rules that produced it; the version moves whenever a rule
changes the level a record gets. Version 1 read evidence, the rationale, the
question and the options. Version 2 also reads the prior decisions, assumptions
and declared bets a decision rests on and the confidence declared at capture, so
Calibration and Bias exposure have floors.

| Dimension | Floor from | Levels | Judged part (needs a model) |
| --- | --- | --- | --- |
| **Framing** | the question the decision answers was recorded | `none` no question recorded · `partial` a question is recorded. Never `solid` without a model. | whether it is the right question |
| **Alternatives** | options recorded, and whether each rejected option carries a description of its own | `none` fewer than two options recorded · `partial` some alternative has no description of its own · `solid` every alternative has one | whether the alternatives are genuine |
| **Information** | what the decision rests on: evidence, a prior decision it follows from, an assumption, a declared bet; and whether counted evidence says where it was observed | `none` nothing counts · `partial` something counts (a prior decision, an assumption and a bet count as information on record, not as observations) · `solid` a counted evidence item says where it was observed | whether it is the relevant information |
| **Reasoning** | a rationale is recorded, or a prior decision it follows from (stated, not judged sound) | `none` neither · `partial` either. Never `solid` without a model. | whether the inference is sound |
| **Values / Tradeoffs** | none: judged only | not assessed | whether the values and tradeoffs were made explicit and weighed |
| **Bias exposure** | whether a counter-option or counter-evidence is on record; the age of the prior decisions it rests on is reported beside it, never scored | `none` neither is on record · `partial` either is. Never `solid` without a model. | whether a distortion shaped the choice |
| **Calibration** | the confidence the decider declared at capture, and what the decision rests on | not assessed when no confidence was declared · `none` declared, and nothing counted to compare it with · `partial` declared, and something counted to compare it with. Never `solid` without a model. | whether the confidence matches what it rests on |

**Alternatives.** The alternatives are the options other than the chosen one (all
of them while none is chosen). A description counts only if it is the author's
own. Text a capture surface fills in when none was given does not, and neither
does a description that only repeats the option's label:

| Written by | Text (`<label>` is the option's label) |
| --- | --- |
| MCP `capture_decision` | `Option generated from MCP value '<label>'` |
| CLI `emit decision.proposed` | `Option generated from CLI value '<label>'` |
| `supersede` | `Option generated from supersede value '<label>'` |
| HTTP capture | `Option '<label>'` |
| Slack ingest | `Slack option '<label>' captured from <source>` |
| Document import | `Option imported from document block <block>` |

A description that merely begins with one of these but goes on (an author who
kept the generated text and added their reason) is the author's own.

**Ex ante.** Something counts toward a floor only if it was recorded before the
decision or attached at capture. Evidence, a prior decision, an assumption or a
bet recorded after the decision, then linked to it, is reported as `later` and
never raises a level. One that was recorded before the decision counts even when
it was linked to the decision afterwards (the backfill of an older decision).
"Before" is the ledger offset of the recording event, so it does not depend on a
clock. An offset that is not known shows nothing, so it does not count.

**What the decision rests on.** Information counts the four kinds of grounding
side by side: an observation (evidence), a prior decision (`FOLLOWS_FROM`), an
assumption and a declared bet (both `ASSUMES`). Only an observation that says
where it was seen can lift Information to `solid`, because only it can be checked
again against the world. Reasoning counts a counted prior decision as stated
reasoning: it is a link, not a judged inference, so it stops at `partial` like a
rationale does.

**A prior decision that was superseded.** It is shown in Information as a fact,
`since superseded (later)` when the superseding event came after the decision,
`already superseded when this decision was recorded` when it came before, and
`not recorded` when the order is unknown. No level moves: that is what happened
next, which the outcome view shows, not how well the decision was made.

**Calibration.** The confidence is the decider's own capture-time words (`low`,
`medium` or `high`), never system-computed and never inferred from the rationale
or from how the decision is grounded. No declared confidence, or a value outside
that vocabulary, means not assessed, with the reason. The level depends on what
is on record to compare the confidence with, never on the confidence itself: a
`high` and a `low` over the same grounding get the same level. High confidence over
a declared bet alone (`high_confidence_over_bet`), or with nothing counted on
record (`high_confidence_over_nothing`), is an attention line on the profile, with
the node ids it rests on. It is not a deduction. A decision that is grounded on an
observation, a prior decision or an assumption as well as a bet is grounded, and
raises no line.

**Bias exposure.** A counter-option is an option set against the one taken (the
same options the Alternatives floor reads). Counter-evidence is evidence that
refutes an assumption or bet the decision rests on and was recorded before the
decision (as for any evidence, the link saying so may have been added afterwards;
evidence recorded after the decision is what happened next, not exposure). Both
are reported as facts, as is
the age in whole days of each counted prior decision when the decision was
recorded, from the two records' own timestamps. The age is shown and never
scored: an old premise is not a lower level.

**Read-only and bounded.** A profile reads one decision and its direct options,
evidence, prior decisions and assumptions or bets with anchored lookups, never a
scan of the graph. The same graph
gives the same profile, with reasons and ids in a fixed order, on the in-memory,
Postgres and Kuzu projections.

## A model's assessment, beside the floors

A floor says what the record states. A model can say more, but only where it can point:
its assessment of a decision is one `decision.scored` event of **schema version 2**, and
every dimension it rates `partial` or `solid` quotes the passage of the decision's own
text it rests on (a `none` answer may, but need not: an absence cannot be quoted). It is
shown beside the floor and never replaces it: no floor rule reads it, so a floor's level
is the same with or without one.

```json
{
  "schema_version": 2,
  "decision_id": "decision-…",
  "model": "claude-haiku-4-5-20251001",
  "prompt_version": "assessment-v1",
  "dimensions": {
    "framing":          { "status": "assessed", "level": "solid", "explanation": "…", "quote": "Which store should hold the ledger?" },
    "alternatives":     { "status": "assessed", "level": "partial", "explanation": "…", "quote": "A file per tenant; rejected, no cross-tenant queries" },
    "information":      { "status": "not_assessed", "reason": "…" },
    "reasoning":        { "status": "assessed", "level": "partial", "explanation": "…", "quote": "…" },
    "values_tradeoffs": { "status": "assessed", "level": "partial", "explanation": "…", "quote": "…" },
    "bias_exposure":    { "status": "assessed", "level": "none", "explanation": "Nothing in the record bears on a distortion" },
    "calibration":      { "status": "not_assessed", "reason": "…" }
  },
  "importance": { … }
}
```

- **All seven dimensions are present.** Each is `assessed` (a `level` of `none`, `partial`
  or `solid`, an `explanation` and a `quote`) or `not_assessed` (a `reason`, and no
  level). A dimension cannot be left out, and none can carry a placeholder number: a
  model that cannot assess a dimension says so, with why. `importance` (stakes,
  irreversibility, actionability, each with its explanation) is a separate axis, optional,
  and not part of the profile. `supersedes_score_id` optionally names the earlier
  `decision.scored` event a re-assessment replaces.
- **A `partial` or `solid` answer must quote; a `none` answer may leave the quote out.**
  A `none` judgement is usually about something the record lacks, and an absence cannot
  be quoted; forcing a quote would invite a tangential one that passes the check but
  grounds nothing. So at level `none` the `explanation` is still required and says what
  is missing, and the `quote` is optional (left out, or `null`; it is absent from what the
  ledger stores). A `partial` or `solid` answer with no quote is refused. A quote that is
  given is held to the same rules at every level: non-blank and verbatim.
- **Keyed by the decision's id**, whether it was proposed or is a classified capture
  (`capture:<batch event>:<index>`). A decision that is recorded nowhere is refused.
- **Every quote given must occur verbatim in the decision's own recorded text.** The
  write path (`Commands::record_decision_assessed`) checks it with a plain substring test:
  exact, case-sensitive, no whitespace repair, no model. For a proposed decision the text
  is its title, question, quote, rationale and each option's label and description; for a
  classified capture its title, rationale, options and chosen option. What the decision
  merely cites (evidence, assumptions, prior decisions) is not part of it. One quote that is
  not found refuses the whole event, names the dimension, and appends nothing. A blank
  quote, explanation or reason, a `partial` or `solid` answer without a quote, a missing
  dimension and importance out of range are refused the same way (a row that got into the
  ledger anyway is skipped on replay, below). The substring check needs the ledger, so it
  runs on the write path only.
- **The newest assessment of a decision is the one the graph shows.** Earlier ones stay in
  the ledger; nothing is overwritten there. It is stored on the decision node as
  `model_assessment` (the model, prompt version and the seven answers, as JSON) and
  `model_assessment_origin` (the ledger offset of the event that recorded it), and touches
  nothing else on the node: the capture's `event_origin`, `source`, `source_ref`, `tenant_id`
  and text stay as the capture wrote them. The projection is the same on the in-memory,
  Postgres and Kuzu backends.
- **Where it shows.** `score_decision` adds a `model_assessment` object beside the
  floors, present only when a model assessed the decision:

  ```json
  "model_assessment": {
    "model": "claude-haiku-4-5-20251001",
    "prompt_version": "assessment-v1",
    "event_origin": 42,
    "dimensions": { "framing": { "status": "assessed", "level": "solid", "explanation": "…", "quote": "…" }, … }
  }
  ```

  The CLI text summary prints a `model_assessment` line and, right after each dimension's
  floor line, a `model_dimension` line with the answer and its quote when one was given
  (or why it is not assessed). The Markdown export's Quality profile section names the
  model and prompt and adds one nested `model` bullet under each dimension. Neither
  invents a quote for a `none` answer that gave none. Scan findings and ticket bodies are
  about floors and facts and do not carry it.
- **An assessment that cannot be read is skipped, never fatal.** The ledger is append-only
  and shared, so a buggy or old producer, a partial import or a hand edit can leave a
  `decision.scored` row whose payload fails the checks above; the write path refuses such an
  event, but nothing stops one that is already there. Every reader (the projector, the
  history and search queries) treats it as "assessment unavailable for this decision": the
  row is skipped, the decision keeps its floors and any readable assessment (before or
  after it), and nothing the row names is created. This holds for a `decision.scored` row of
  either version and for `decision.metadata_derived`; a malformed event of any other kind
  is what the graph is built from and still fails the read. The skip is said out loud: CLI
  `query`, `digest`, `export` and `quality-scan`, and every MCP read tool (stdio and
  `/mcp`), carry a one-line `notice` (`1 assessment row could not be read and was skipped
  (ledger event 57: …); answers leave it out`) naming the ledger events, as a `notice` key
  in JSON and a final `notice:` line in `--summary` output, and a projection logs one
  warning with the count and offsets. The background scorer does not count such a row as an
  assessment, so the decision is still picked up and assessed. A ledger with nothing unreadable
  in it says nothing extra.
- **Version-1 scores stay as they are.** Every `decision.scored` event written before
  schema version 2 (no `schema_version`) keeps its shape: 0 to 1 floats per dimension,
  keyed by a classifier capture node. The ledger is immutable, so those events keep
  validating and replaying and keep projecting their float properties onto the capture
  node. They are never shown in the profile: they carry no quoted basis, and the prompt
  that wrote them asked for a 0.5 when a dimension could not be assessed. Nothing writes
  that shape any more (below): both producers write schema version 2 only.

## Producers of a model assessment

Two producers write a version-2 assessment, both optional and both going through the
same write path (`Commands::record_decision_assessed`), so a plugin/edge call is
checked exactly as strictly as the server's own:

- **The background scorer** (`src/scorer.rs`). Runs only with `ANTHROPIC_API_KEY` set;
  absent, it logs once and never starts, and the rest of the system — including every
  floor — stays fully correct without it. Every 30 seconds it scans the ledger for
  decisions with no version-2 assessment yet: both classifier-extracted captures
  (`ingest.batch_classified`) and decisions captured directly (`decision.proposed`, which
  underlies `decision.capture` too). For each it sends the decision's own recorded text to
  a model (`claude-haiku-4-5-20251001` by default, `HIVEMIND_SCORER_MODEL` to override)
  with a prompt that asks it to assess Framing and Values / Tradeoffs — the two dimensions
  with no floor beyond "a question was recorded" — and to enrich any of the other five only
  where it has real grounds, leaving the rest `not_assessed`. A malformed or unfound-quote
  answer is refused by the write path and logged; the decision stays unassessed and is
  retried on the next pass.
- **The keyless `emit decision.scored` CLI path.** Lets a plugin or edge session (no
  server-held key) submit an assessment it produced itself — typically by spawning its own
  Haiku subagent with the same prompt, off the server's metered key. The scores file is
  `{"dimensions": {...}}` in the wire shape [above](#a-models-assessment-beside-the-floors)
  (optionally `"importance"` too), and the target decision is named either by
  `--decision-id` (a decision proposed or captured directly) or by `--batch-id` +
  `--capture-index` (a classifier-extracted capture, resolved to its `capture:<event>:<index>`
  node internally — a caller never constructs that id itself).

## Attention findings: what needs a look

Implemented in `src/quality_profile/findings.rs`. The roll-up of the profile is not a grade. It is a list of decisions that need a
look now, each with the reason in words and the nodes it rests on. A finding is
derived from what the graph already states; nothing is scored, ranked or
inferred, and the list works self-hosted with no model and no network. The order
is by decision id so that a page can be resumed; a consumer that wants a priority
applies its own.

| Kind | The decision is flagged when | Basis time (`basis_at`) |
| --- | --- | --- |
| `bet_past_check_date` | it rests on a declared bet whose check date has passed and nothing supports or refutes it | the check date |
| `premise_superseded` | a decision it follows from was superseded | when the earliest superseding decision was made |
| `premise_rejected` | a decision it follows from was rejected and nobody accepted it | none: the graph does not record when a decision was rejected |
| `assumption_refuted` | an assumption it rests on was refuted | when the earliest refuting evidence was recorded |
| `bet_failed` | a bet it rests on was refuted | when the earliest refuting evidence was recorded |
| `evidence_not_rechecked` | the newest evidence linked to it was recorded more than the window ago | when that evidence was recorded |

The first kind is the list of bets past their check date, the four in the middle
are the list of decisions whose premise changed, the last is the list of evidence
not re-checked. A request can ask for any subset of the six.

**Evidence not re-checked.** The window is 90 days by default (one quarter) and is
configurable (`AttentionConfig::evidence_window_days`). "Re-checked" means newer
evidence is linked to the decision, whether it was attached at capture or
afterwards; the graph has no notion of one observation re-checking another, so
the finding is about the decision's newest evidence, not about each item. Evidence
recorded exactly one window ago is not yet old; one second earlier is. A decision
with no evidence, or none whose record states a time, is never flagged here: its
Information floor already says it rests on nothing. "High-leverage premises not
re-examined" needs the re-examine state and is built with `hivemind-ottw`, not here.

**Standing decisions, one hop.** Only a decision that has not been superseded or
rejected is flagged: a decision that was replaced needs no look. A decision is
flagged for its own premises only. Carrying a change on to the decisions that rest
on the flagged one is the re-examine walk (`hivemind-ottw`). A contested premise
is a disagreement and is shown as such, not flagged as a change. A decision
superseded by more than one decision names the earliest as the basis, and the
first refutation is the basis of a refuted assumption or bet. A finding names the
decision that needs the look, the premise, hypothesis or evidence it is about and,
where a time is taken from another node (the superseding decision, the refuting
evidence), that node too: `node_ids` is sorted, distinct and always includes the
decision.

**Named in words, not just ids.** `reason` reads as a sentence a stranger can act on
without a lookup: the flagged decision's own title is on `decision_title`, and every
other node the reason names is worded too -- a prior decision by its title, an
assumption or bet by its statement, an evidence item by its content -- clipped to 200
characters and falling back to the id only when the record states no title,
statement or content. The ids (`decision_id`, `node_ids`) stay in the finding as
handles for a follow-up call (`score_decision`, `get_decision`), never as the only
way to say what the finding is about.

**Stable ids.** `finding_id` is `finding-` and the first 16 bytes (32 hex digits)
of the SHA-256 of the kind, the node ids and the basis time (written as UTC with
nanoseconds; a missing time is `-`), so a consumer can dedupe across scans. The
same graph gives the same ids on every run, on the in-memory, Postgres and Kuzu
projections, and an id does not depend on the clock. It changes when the basis
does: newer evidence is linked to the decision, another decision supersedes the
premise earlier, a bet is replaced by one with another check date.

**Paging and cost.** A page holds at most `limit` findings (default 50, never more
than 1000). When more follow, `truncated` is true and `next_cursor` resumes after
the last finding returned. The cursor is a position in the order, not an offset:
findings that appear or vanish between two pages never make a consumer skip or
repeat one that stood throughout. Findings are never dropped silently, and no
request returns the whole graph.

A request may list findings to leave out, by `finding_id`. They are left out before
the page is cut, not after: the page is filled from the findings that remain, so it
holds `limit` of them whenever that many remain, and `truncated` is true only when
another finding that was not left out follows.

A page issues 12 bulk reads (`GROUNDING_FACT_READS`: who superseded, accepted or
rejected which decision; the `FOLLOWS_FROM`, `BASED_ON`, `PREMISED_ON_DIRECT`,
`PREMISED_ON` and `CHOSE` links; the hypothesis and evidence rows; what refutes or
supports a hypothesis), however many decisions there are, plus the anchored reads
that word each finding: one per distinct decision the finding names (the decision
itself, and -- for `premise_superseded` and `premise_rejected` -- the premise),
which gives its title and, for a decision that superseded a premise, also states
when in the same read; and one per distinct hypothesis or evidence item the finding
names. That is a small, fixed multiple of `limit` more (at most three per finding:
the decision, its subject and its basis), never a scan. When findings are left out,
the same anchored reads are made for each one stepped over on the way to a full
page: the extra cost grows with the findings left out that sort before the end of
the page, and stops there. The rows those reads return grow with the grounding
links, hypotheses and evidence, not with decisions times a per-decision cost.
Nothing walks the premise graph, so a `FOLLOWS_FROM` cycle cannot loop and costs
nothing extra.

## What the tools return

One core, `src/quality_profile/report.rs`, builds every answer. The stdio server, the
HTTP endpoint and the CLI each parse their own arguments, call it and serialize what it
returns, so the three cannot drift (`transport_parity` tests run the same ledger through
all three). Responses use the usual envelope: `result_count`, `truncated`, `latency_ms`
and `data`.

| | MCP (stdio and HTTP) | CLI (JSON by default, `--summary` for text) |
| --- | --- | --- |
| Profile of one decision | `score_decision {decision_id}` | `hivemind query score_decision --id <id>` |
| A page of findings | `scan_decision_quality {kinds?, evidence_window_days?, limit?, cursor?}` | `hivemind query scan_decision_quality [--kind a,b] [--evidence-window-days N] [--limit N] [--cursor C]` |
| A page of findings not yet acknowledged | `get_suggestions {kinds?, evidence_window_days?, exclude_acknowledged?, limit?, cursor?}` | `hivemind query get_suggestions [--kind a,b] [--evidence-window-days N] [--exclude-acknowledged BOOL] [--limit N] [--cursor C]` |
| Acknowledge a finding | `acknowledge_suggestion {finding_id, decision_id, action?, channel?, actor_id?}` | |
| One Linear ticket per finding | | `hivemind quality-scan [--kind a,b] [--limit N] [--dry-run]` |

### `score_decision`

`data` is `null` when the decision does not exist. Otherwise it is the profile of the
decision, whichever way it was captured:

```json
{
  "decision_id": "decision-…",
  "floor_version": 2,
  "framing":          { "status": "assessed", "level": "partial", "reasons": [ { "kind": "question_recorded", "text": "…", "node_ids": ["decision-…"] } ], "node_ids": ["decision-…"] },
  "alternatives":     { "status": "assessed", "level": "solid", "reasons": [ … ], "node_ids": [ … ] },
  "information":      { "status": "assessed", "level": "partial", "reasons": [ … ], "node_ids": [ … ] },
  "reasoning":        { "status": "assessed", "level": "partial", "reasons": [ … ], "node_ids": [ … ] },
  "values_tradeoffs": { "status": "not_assessed", "why": "Judged only: …" },
  "bias_exposure":    { "status": "assessed", "level": "partial", "reasons": [ … ], "node_ids": [ … ] },
  "calibration":      { "status": "not_assessed", "why": "No confidence was declared at capture, …" },
  "attention": [ { "kind": "high_confidence_over_bet", "dimension": "calibration", "text": "…", "node_ids": [ … ] } ],
  "model_assessment": { "model": "…", "prompt_version": "…", "event_origin": 42, "dimensions": { … } },
  "provenance": { "authorship": "agent_only", "review": "self_accepted", "line": "not yet reviewed by a human" }
}
```

Every dimension is either `assessed` (an ordinal `level`, its `reasons` and the
`node_ids` they rest on) or `not_assessed` (a `why`, and no `level` key). The text
summary shows one line per dimension: the level, the ids and the reasons, or why it was
not assessed. `model_assessment` is present only when a model assessed the decision, and
sits [beside the floors](#a-models-assessment-beside-the-floors) without changing them.

**Provenance.** `authorship` and `review` are the decision's existing authorship and
review shapes. `line` is `not yet reviewed by a human`, present exactly when an agent
authored the decision and no human accepted it (whatever else happened to it), and
absent when someone rejected it: the review shape does not say whether that someone was
a human, and the line makes a negative claim. It is a provenance label shown beside the
profile, never a deduction from it.

### `scan_decision_quality`

`data` is one page of [attention findings](#attention-findings-what-needs-a-look), in
order of decision id. Each carries the dimensions it bears on, each with its level and
reasons:

```json
{
  "as_of": "2026-09-26T00:00:00Z",
  "evidence_window_days": 90,
  "findings": [
    {
      "finding_id": "finding-…",
      "kind": "bet_past_check_date",
      "decision_id": "decision-…",
      "decision_title": "Size the fleet for flat load",
      "basis_at": "2026-09-01T00:00:00Z",
      "node_ids": ["decision-…", "hypothesis-…"],
      "reason": "the bet 'Load stays flat' was to be checked by 2026-09-01; nothing has been recorded for or against it",
      "dimensions": [
        { "dimension": "information", "status": "assessed", "level": "partial", "reasons": [ … ], "node_ids": [ … ] },
        { "dimension": "calibration", "status": "not_assessed", "why": "…" }
      ]
    }
  ],
  "next_cursor": "…"
}
```

`truncated` (in the envelope) says more findings follow, and `data.next_cursor` is
present exactly then; pass it as `cursor` to continue. Nothing is dropped silently.
`kinds` limits the page to some of the six kinds, `evidence_window_days` overrides the
default window (90), and `limit` defaults to 25 (at most 1000). An unknown kind, or a
cursor no scan returned, is refused.

| Kind | Dimensions it bears on | Why |
| --- | --- | --- |
| `bet_past_check_date` | Information, Calibration | the bet counts as information on record; confidence was declared over it |
| `premise_superseded`, `premise_rejected` | Information, Reasoning | the prior decision counts as information and as stated reasoning |
| `assumption_refuted` | Information | the assumption counts as information on record |
| `bet_failed` | Information, Calibration | as for a bet past its check date |
| `evidence_not_rechecked` | Information | the evidence is what Information counts |

**Cost.** A page costs what the findings cost (see below) plus one profile read per
distinct decision on the page: never more than `limit` of each, however many decisions
the graph holds. A profile read is a handful of anchored lookups, not a scan.

### `get_suggestions`

`data` is the page `scan_decision_quality` returns, in the same shape and order, without
the findings that have been acknowledged. It takes the same arguments (`kinds`,
`evidence_window_days`, `limit`, `cursor`), reads and refuses them the same way, and adds
`exclude_acknowledged`: true by default, "what is new since I last looked"; false returns
every finding, as `scan_decision_quality` always does. On the CLI it is
`--exclude-acknowledged false`. `acknowledge_suggestion` is MCP-only.

A finding is acknowledged by its `finding_id`. The id changes when the finding's basis
does (newer evidence is linked, another decision supersedes the premise, a bet gets
another check date), so a finding whose basis moved on after it was acknowledged is a new
finding and is shown again: an old acknowledgement never hides it.

Acknowledged findings are left out before the page is cut (see
[Paging and cost](#attention-findings-what-needs-a-look)), so a consumer that has dealt
with a long run of findings still gets a full page, and `truncated` and `data.next_cursor`
say exactly whether more follow.

#### Acknowledging a finding

An acknowledgement is two recorded events, both attributed to an actor and never edited:

1. `suggestion.surfaced`: this `finding_id` (about `decision_id`) was shown to
   `recipient_actor_id` over `channel` at `sent_at`. `channel` is a label the consumer
   chooses (`slack`, `linear`, `mcp`); HiveMind attaches no meaning to it. The projection
   is a `Notification` node keyed by the event's uuid, with the finding on it. It is not a
   notification about a blocker: it has no `blocker_id` and draws no
   `NOTIFICATION_FOR_BLOCKER` edge, and the blocker queries do not see it.
2. `notification.acknowledged`, naming that uuid as `notification_id`, with `ack_at`, an
   optional `snooze_until` and an optional `action`: `seen` (looked at, nothing more
   claimed), `acted` (dealt with) or `dismissed` (set aside on purpose). Acknowledgements
   written before `action` existed carry none and are unchanged.

`acknowledge_suggestion` appends both through the ordinary write path: `finding_id` and
`decision_id` are read off the finding, `action` defaults to `seen`, `channel` to `mcp`,
and the recipient is the acting actor. A consumer that surfaces findings itself (a Slack
bot, a Linear connector) records `suggestion.surfaced` when it posts and
`notification.acknowledged` when someone responds, and `get_suggestions` reads both the same
way.

A finding is hidden when a surfaced record of its `finding_id` has been acknowledged. Every
action hides it; the ledger keeps which. Acknowledgement is by finding, not by actor: once
anyone has acknowledged a finding, `get_suggestions` leaves it out for everyone, and
`exclude_acknowledged: false` shows it again. An acknowledgement with a `snooze_until` hides
the finding only until that moment. The finding is read through the same pure read of the
graph as everything else (no LLM, nothing derived beyond these events), so it is reversible
by replay: nothing is deleted, and a finding whose basis changes has a new `finding_id` and
is shown again.

The write path does not check the finding. Finding findings is the scan's work, and a write
that ran it would be Layer 3 inside Layer 1: an acknowledgement of an id nothing reports
acknowledges nothing, and the decision id is recorded as given.

### `analyze_failure_modes`

The failure-mode analysis (which conditions go with decisions that did not hold up)
groups decisions by the profile too. Its `by_condition` lists one group for each
dimension and level: `framing`, `alternatives`, `information`, `reasoning`,
`values_tradeoffs`, `bias_exposure` or `calibration`, each with the group label `none`,
`partial`, `solid` or `not_assessed`, and the same `total`, `failed`, `failure_rate`,
`effect_vs_baseline` and `confidence` as every other group. These groups sort decisions
and nothing else: what counts as a failure is still the outcome view's call
(superseded, a premise that no longer stands, contested), a level is never itself a
failure, and `not_assessed` is its own group, never folded into `none`. The groups take
part in `findings` like any other. stdio MCP only.

### `quality-scan`

`hivemind quality-scan` reads the first page of findings (at most `--limit`, 1–50,
default 10) and files one Linear ticket for each, or prints them with `--dry-run`. Its
`finding_id` is what to dedupe on across runs.

A ticket is titled `[HiveMind] <kind>: <decision title>` (the decision id when it has no
title). Its body, in Markdown, has:

- the decision's title and id, the kind of finding and the `finding_id`, and a link to the
  decision when `--hivemind-base-url` is set;
- **Why it needs a look**: the finding's reason in words, naming every node it names by its
  own title, statement or content, not by a bare id;
- **Node IDs**: every node the finding rests on;
- **Dimensions it bears on**: the dimensions in the table under `scan_decision_quality`, one
  bullet each with its level and, beneath it, each reason with the ids it rests on, or "not
  assessed" and why. These are the profile's own lines, worded as in the export below.

There is no score and no tier in a ticket, and none in the dry-run JSON (`finding_id`,
`decision_id`, `kind`, `title`, `description`).

### The decision-log export

`hivemind export --format markdown` writes each decision's file with a **Quality profile**
section after Outcome and before Provenance: a line saying what the floors are (rules version
`floor_version`, what the record states and not whether it is sound), then the seven
dimensions in order, each as a bullet with its level and one nested bullet per reason with
the ids it rests on, or "not assessed" and why, and, when there are any, the attention lines
under "Worth a second look". When a model assessed the decision, a line names the model and
prompt version and each dimension gains a nested `model` bullet with the answer and the passage
it quotes. It is the text of `score_decision` for that decision, laid out for reading; nothing
is added and nothing is graded.

```markdown
## Quality profile

Floor rules version 2: what the record states, not whether it is sound.

- **Framing** — none
  - no question is recorded: the record does not say what question this decision answers
- **Alternatives** — partial
  - 1 alternative recorded besides the chosen option (`org:launch:option:delay`)
  - 1 alternative without a description of its own (text a capture surface fills in does not count) (`org:launch:option:delay`)
- **Information** — solid
  - 2 evidence items counted: recorded before the decision or attached at capture (`org:launch:evidence:legal-caveat`, `org:launch:evidence:qa-failures`)
  - ...
- **Values / Tradeoffs** — not assessed: Judged only: nothing recorded can stand in for a judgement ...
```

(An excerpt of a real exported decision; the other dimensions read the same way.)

The Outcome section states what happened to the decision (superseded, premise refuted or
superseded, contested) and lists no quality signal: a decision with no options or nothing
declared it rests on is no longer a "Thin structure" reason there, and reads as its
Alternatives and Information lines instead.

The export itself is a pure Layer 2 projection and never imports the profile: the CLI passes
it a function that returns each exported decision's section, and an export given none omits
the section and changes nothing else. **Cost:** one profile read per exported decision (a
handful of anchored lookups each), on top of the reads the export already makes for it.

## Where the retired deductions went

Earlier versions graded a decision with five deductions and a tier. They were outcome,
status and provenance signals wearing a quality label, and they are gone from
`score_decision` and `scan_decision_quality` along with the score and the tier.

| Was a deduction for | Now |
| --- | --- |
| superseded | the "did it hold up" outcome view (`verify`), shown beside quality and never folded into it. Supersession is a fact there (which decision replaced it); nothing weighs how quickly it happened, and a fast reversal is never penalized |
| premised on a refuted assumption, or on a superseded or rejected decision | staleness: the `assumption_refuted`, `bet_failed`, `premise_superseded` and `premise_rejected` findings |
| contested | a status, shown as such: deducting for disagreement would be conformity bias |
| agent-only, unreviewed | the provenance line |
| thin structure (no options, nothing declared it rests on) | the Alternatives and Information floors. It is no longer an outcome reason either: `verify`, `why`, `decision_quality_candidates` and the export do not list it; the decision-log export states it in its Quality profile section instead |


## Deferred, not built

Nothing below exists in the code. It is kept as the design the profile was built to
leave room for, and as the reason there is no number in a response.

- **Composite.** An earlier design rolled the seven dimensions into a weighted `[0,1]`
  composite with tunable, versioned weights. It is not built: most dimensions read
  `not assessed` without a model, and an average over them would be invented
  confidence. The roll-up is the attention list.
- **Confidence.** Derived from a composite: how sure we are that a decision was
  well-made. Deferred with it. (Distinct from the *capture-time, author-reported*
  confidence the decider declares, which Calibration reads and which is built.)
- **Reputation.** An actor's importance-weighted average of the quality of their
  decisions. Deferred, and it needs both a composite and Importance.
- **Importance (a second axis).** A magnitude, not a percentage:
  `Importance = Stakes × Irreversibility × Actionability`, with Stakes unbounded and
  log-scaled (`severity × reach`), Irreversibility in `[0,1]` as a discount (two-way
  doors matter less) and Actionability in `[0,1]` as a gate. Reversibility lives here,
  not in the quality dimensions.
- **Validation.** Perturbation and ablation (degrade one dimension, confirm that
  dimension moves), dogfooding against expert agreement, and prospective prediction of
  reverts with zero outcome leakage.

Open questions for that work: Reputation computation at scale.

## References

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — three-layer boundary, event→query flow.
- [`PRINCIPLES.md`](../PRINCIPLES.md) — §2 auditability, §7 enforced layer boundary.
- [`DECISION_QUALITY_LITERATURE.md`](DECISION_QUALITY_LITERATURE.md) — research basis.
- [`VISION.md`](../VISION.md) / [`STRATEGY.md`](../STRATEGY.md) — Layer-3 capabilities as an investment front.
