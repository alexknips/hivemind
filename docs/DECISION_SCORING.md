# Decision Scoring

> **Status: shipped as of v0.6.0 (decision-quality layer, he9a.3).**
> The 2-axis scorer described here is implemented in `src/queries/inhouse_scorer.rs`
> and exposed via MCP tools `score_decision` / `scan_decision_quality` and the
> `query scan_decision_quality` CLI subcommand. Open questions at the bottom of
> this file are deferred to post-v0.6.0 follow-ups; this document remains the
> design reference.

HiveMind records *what* was decided, by whom, with what options and evidence
([`ARCHITECTURE.md`](ARCHITECTURE.md)). Decision scoring adds a separate,
derived judgement: *how well-made* a decision was and *how much it mattered* —
computed after the fact by an agent, never mixed into the decision record
itself.

The research basis is [`DECISION_QUALITY_LITERATURE.md`](DECISION_QUALITY_LITERATURE.md).
(That document's model section must be reconciled to match this one — see
[Reconciliation with the literature doc](#reconciliation-with-the-literature-doc).)

## Architectural placement (and why it stays in bounds)

Scoring is agentic analysis. It lives strictly in **Layer 3**, above the write
and query paths, per [`PRINCIPLES.md` §7](../PRINCIPLES.md) and
[`ARCHITECTURE.md` → Layer Boundary](ARCHITECTURE.md). Four properties keep it
compliant and trustworthy:

1. **Ex ante.** A score uses **only what was knowable at decision time, never the
   outcome.** It measures the decision, not the luck. Outcome data is forbidden
   input.
2. **Server-computed, not client-reported.** The decider supplies *evidence and
   artifacts*; a **server-side Layer-3 agent assigns the scores.** Clients never
   self-report scores. This preserves provenance and removes the obvious gaming
   path.
3. **Outside write and query.** Layer 2 "does not call LLMs, rank, cluster,
   summarize, or invent confidence" ([`ARCHITECTURE.md`](ARCHITECTURE.md)).
   Scoring does all of those things — so it cannot live there. It reads the
   ledger/projection and writes only its own annotations.
4. **Append-only annotations.** Scores are stored as **immutable annotation
   events that reference the decision**, not as edits to it. Re-assessment on new
   evidence appends a **new annotation that supersedes the prior one by
   reference** — the same supersession-not-overwrite pattern decisions already
   use. Full history is preserved; auditability ([`PRINCIPLES.md` §2](../PRINCIPLES.md))
   holds.

## The model

Two independent axes, plus two derived quantities.

| Axis | Range | Question |
| --- | --- | --- |
| **Quality** | bounded `[0,1]` | How well-made was the decision? |
| **Importance** | **unbounded magnitude** (not 0–100%) | How much does it matter? |
| *derived* **Confidence** | from Quality composite | How sure are we it was well-made? |
| *derived* **Reputation** | per actor | Track record on decisions that mattered |

### Axis 1 — Quality `[0,1]`

A weighted composite of **seven dimensions**. Each dimension is scored
numerically **with an explanation**, and is **stored individually** so the
composite (and any reweighting) recomputes for free.

| # | Dimension | What it assesses |
| --- | --- | --- |
| 1 | **Framing** | Was the right problem/question framed? |
| 2 | **Alternatives** | Were genuine alternatives generated and considered? |
| 3 | **Information** | Was the relevant information gathered and used? |
| 4 | **Reasoning** | Is the inference from information to choice sound? |
| 5 | **Values / Tradeoffs** | Were the values and tradeoffs made explicit and weighed? |
| 6 | **Bias exposure** | Exposure to **non-calibration** cognitive distortions: anchoring, confirmation, sunk-cost, framing, motivated reasoning. |
| 7 | **Calibration** | Matching confidence to evidence; acknowledging unknowns; avoiding over- and under-confidence. |

- **Aggregation:** weighted composite. **Weights are tunable and versioned** —
  start with sensible defaults, then *fit them from validation data*. Each stored
  composite **records its weight-version** for reproducibility. (A reasonable,
  explicitly non-binding starting point is equal weights, to be refit.)
- **Bias vs Calibration are kept separate** by design. Bias covers distortions
  *other than* confidence/evidence mismatch; calibration covers that mismatch.
  **Safeguard:** if the two empirically correlate above ~0.7, consider merging
  them into one dimension.
- The **Quality composite drives Confidence.**

### Floors: what each dimension can say without a model

> **Status.** The floors below are implemented in `src/quality_profile.rs` as a
> library type that returns a *profile* for any decision, however it was captured.
> No tool, verb or export shows the profile yet; the composite scorer above is
> still what `score_decision` returns.

A **floor** is the part of a dimension that can be computed from facts the record
already states, deterministically, self-hosted, with no model and no network. A
floor says what is written down, never that it is sound. The profile lists all
seven dimensions for a decision. Each one is either

- **assessed**: an ordinal `level` (`none`, `partial`, `solid`), the `reasons`
  behind it and the `node_ids` they rest on; or
- **not assessed**: a `why`, and no level. A dimension with no basis says so
  rather than guessing.

There is no composite number, no tier and no grade in the profile: each
dimension stands alone. `none` means the record states nothing toward the
dimension, not that the decision was bad. Every profile records the
`floor_version` of the rules that produced it; the version moves whenever a rule
changes the level a record gets.

| Dimension | Floor from | Levels | Judged part (needs a model) |
| --- | --- | --- | --- |
| **Framing** | the question the decision answers was recorded | `none` no question recorded · `partial` a question is recorded. Never `solid` without a model. | whether it is the right question |
| **Alternatives** | options recorded, and whether each rejected option carries a description of its own | `none` fewer than two options recorded · `partial` some alternative has no description of its own · `solid` every alternative has one | whether the alternatives are genuine |
| **Information** | evidence linked to the decision, and whether it says where it was observed | `none` no evidence counts · `partial` evidence counts, none says where it was observed · `solid` at least one counted item says where it was observed | whether it is the relevant information |
| **Reasoning** | a rationale is recorded (stated, not judged sound) | `none` no rationale · `partial` a rationale is recorded. Never `solid` without a model. | whether the inference is sound |
| **Values / Tradeoffs** | none: judged only | not assessed | whether the values and tradeoffs were made explicit and weighed |
| **Bias exposure** | none in this version: it would need whether a counter-option or counter-evidence was recorded and how old the premises were | not assessed | whether a distortion shaped the choice |
| **Calibration** | none in this version: it would need the confidence the decider declared at capture and what the decision rests on | not assessed | whether confidence matches the evidence |

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

**Information (ex ante).** Something counts toward a floor only if it was
recorded before the decision or attached at capture. Evidence recorded after the
decision, then linked to it, is reported as `later` and never raises the level.
Evidence that was recorded before the decision counts even when it was linked to
the decision afterwards. "Before" is the ledger offset of the recording event, so
it does not depend on a clock.

**Read-only and bounded.** A profile reads one decision and its direct options
and evidence with anchored lookups, never a scan of the graph. The same graph
gives the same profile, with reasons and ids in a fixed order, on the in-memory,
Postgres and Kuzu projections.

### Axis 2 — Importance (unbounded magnitude)

Importance is a **magnitude, not a probability or percentage.** It is explicitly
**not** on a 0–100% scale.

```
Importance = Stakes × Irreversibility × Actionability
```

- **Stakes** — unbounded, **log-scaled.** Severity and reach are **factors of
  stakes** (`Stakes ≈ severity × reach`), not separate top-level dimensions.
- **Irreversibility** ∈ `[0,1]` — a discount. Reversible decisions (two-way
  doors) are **less important to get right** than one-way doors, so they are
  discounted toward 0.
- **Actionability** ∈ `[0,1]` — a **gate.** An unactionable item or non-decision
  drives importance to ≈ 0.

Note that **reversibility lives here, in Importance** — it is *not* a quality
dimension. A reversible decision is not lower *quality*; it simply matters less.

### Derived — Confidence

Confidence is derived from the **Quality composite**: it expresses how sure we
are that a decision was well-made. (This Layer-3 derived confidence is distinct
from the separate, still-open question of a *capture-time, author-reported*
confidence field — see [Open questions](#open-questions-for-the-implementation-bead).)

### Derived — Reputation

An actor's reputation is the **importance-weighted average of the Quality of
their decisions** over their history. High-importance, well-made calls dominate;
reversible or trivial decisions barely move it. Reputation therefore reflects a
track record on the decisions that actually mattered.

## Storage model

- Scores are **append-only annotation events** referencing the decision id
  (immutable; never an edit to the decision).
- **Every per-dimension score is stored individually** — the seven Quality
  dimensions and the Importance factors (stakes severity, reach, irreversibility,
  actionability) — each as a numeric value plus its explanation.
- **Composites are derived, not stored as truth.** Quality composite, Importance,
  Confidence, and Reputation recompute from the stored per-dimension values.
  Each composite carries its **weight-version**.
- **Re-assessment** appends a new annotation event that **supersedes the prior
  assessment by reference.** The decision's own record is untouched.

This mirrors the existing event model: raw, attributed facts are the stored
truth; everything aggregate is a rebuildable projection.

## Validation plan

Post-PoC and **non-blocking** — the "this actually works" proof, run after the
model exists, not a gate on shipping the PoC.

1. **Perturbation / ablation.** Degrade a decision along one dimension and
   confirm *that* dimension's score drops. Ground truth is true by construction;
   the scorer must beat a trivial baseline.
2. **Dogfood + expert agreement.** Score the real ledger and compare against
   expert judgement — concurrent validity against the real decision distribution.
3. **Prospective revert/supersession prediction.** Ex-ante scores should predict
   later reverts/supersessions, with **zero outcome leakage** (the scores were
   assigned before the outcome existed).

The historical-decisions study is **optional, stage-2 marketing** — non-blocking
and *not* the primary validation.

## Reconciliation with the literature doc

[`DECISION_QUALITY_LITERATURE.md`](DECISION_QUALITY_LITERATURE.md)'s model
section has been brought into line with the locked model above:

- **Reversibility moves to Importance** (it is no longer a quality dimension).
- **Calibration is split out from Bias** (two separate quality dimensions).

> **Done in this PR.** The literature doc was folded into this branch from an
> operator-staged backup (the refinery's separate landing was blocked on a
> permission prompt). Its model section now matches: reversibility under
> Importance, calibration split out from bias, and the axis named Importance.

## Open questions (for the implementation bead)

All deferred to the post-PoC implementation, not part of this capture:

- The concrete annotation **event schema and name** (a new Layer-3 scoring/
  assessment event under `schemas/`).
- **Default weights** and the procedure to fit them from validation data.
- The **bias/calibration merge** decision, pending the >0.7 correlation check.
- **When the scorer runs** (on capture, on demand, or batch) and how the
  server-side agent is invoked.
- **Reputation** computation and refresh mechanics at scale.
- Relationship to a possible **capture-time author-reported confidence** field
  (Layer-1, author-supplied) vs this Layer-3 derived confidence — an operator
  question currently open and tracked separately.

## References

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — three-layer boundary, event→query flow.
- [`PRINCIPLES.md`](../PRINCIPLES.md) — §2 auditability, §7 enforced layer boundary.
- [`DECISION_QUALITY_LITERATURE.md`](DECISION_QUALITY_LITERATURE.md) — research basis.
- [`VISION.md`](../VISION.md) / [`STRATEGY.md`](../STRATEGY.md) — Layer-3 capabilities as an investment front.
