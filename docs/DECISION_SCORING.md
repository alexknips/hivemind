# Decision Scoring

> **Status: shipped as a quality *profile* and a list of *attention findings*. There is
> no score.** `score_decision` returns the profile of one decision,
> `scan_decision_quality` returns one page of attention findings and `get_suggestions`
> returns the same page without the findings someone has acknowledged, on the stdio MCP
> server, the HTTP MCP endpoint and the CLI. All are read on demand from what the
> ledger states, with no model and no network. No response carries a composite number,
> a tier or a grade. **Deferred, not built:** Composite, Confidence, Reputation,
> Importance, and model assessments beside the floors (see [Deferred](#deferred-not-built)).

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
4. **No model, no network.** The floors run self-hosted with no API key. A model
   assessment beside a floor is deferred; when it lands it is stored as an append-only
   annotation that supersedes the previous one by reference, never as an edit to the
   decision.

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
supports a hypothesis), however many decisions there are, plus one anchored read
for each distinct superseding decision on the page, at most `limit` more. When
findings are left out, the same anchored read is made for each one stepped over on
the way to a full page: the extra cost grows with the findings left out that sort
before the end of the page, and stops there. The rows those reads return grow with
the grounding links, hypotheses and evidence, not with decisions times a per-decision
cost. Nothing walks the premise graph, so a `FOLLOWS_FROM` cycle cannot loop and
costs nothing extra.

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
  "provenance": { "authorship": "agent_only", "review": "self_accepted", "line": "not yet reviewed by a human" }
}
```

Every dimension is either `assessed` (an ordinal `level`, its `reasons` and the
`node_ids` they rest on) or `not_assessed` (a `why`, and no `level` key). The text
summary shows one line per dimension: the level, the ids and the reasons, or why it was
not assessed.

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
      "basis_at": "2026-09-01T00:00:00Z",
      "node_ids": ["decision-…", "hypothesis-…"],
      "reason": "the bet hypothesis-… was to be checked by …; nothing has been recorded for or against it",
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
`--exclude-acknowledged false`.

A finding is acknowledged by its `finding_id`. The id changes when the finding's basis
does (newer evidence is linked, another decision supersedes the premise, a bet gets
another check date), so a finding whose basis moved on after it was acknowledged is a new
finding and is shown again: an old acknowledgement never hides it.

Acknowledged findings are left out before the page is cut (see
[Paging and cost](#attention-findings-what-needs-a-look)), so a consumer that has dealt
with a long run of findings still gets a full page, and `truncated` and `data.next_cursor`
say exactly whether more follow.

**No event records an acknowledgement yet.** Until one does, nothing is acknowledged and
`get_suggestions` returns what `scan_decision_quality` returns, whichever way
`exclude_acknowledged` is set. The argument, its default and the way findings are left
out are in place, so a consumer written against `get_suggestions` today needs no change
when acknowledgements arrive.

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
default 10) and files one Linear ticket for each, or prints them with `--dry-run`. The
ticket names the kind of finding and the decision, and carries the reason and the node
ids. Its `finding_id` is what to dedupe on across runs.

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
| thin structure (no options, nothing declared it rests on) | the Alternatives and Information floors. It is no longer an outcome reason either: `verify`, `why`, `decision_quality_candidates` and the export do not list it |


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
- **Model assessments.** A model may assess Framing and Values / Tradeoffs, which have
  no floor beyond "a question was recorded", and may enrich the others, but only with
  its basis quoted from the decision's own text, beside the floor and never replacing
  it, and stored as an append-only annotation event that supersedes the previous one by
  reference.
- **Validation.** Perturbation and ablation (degrade one dimension, confirm that
  dimension moves), dogfooding against expert agreement, and prospective prediction of
  reverts with zero outcome leakage.

Open questions for that work: the annotation event schema and name, when a model
assessment runs and how the agent is invoked, and Reputation computation at scale.

## References

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — three-layer boundary, event→query flow.
- [`PRINCIPLES.md`](../PRINCIPLES.md) — §2 auditability, §7 enforced layer boundary.
- [`DECISION_QUALITY_LITERATURE.md`](DECISION_QUALITY_LITERATURE.md) — research basis.
- [`VISION.md`](../VISION.md) / [`STRATEGY.md`](../STRATEGY.md) — Layer-3 capabilities as an investment front.
