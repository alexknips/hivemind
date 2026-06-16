# Decision Quality & Value — Literature Reference

*Captured 2026-06-15. Reference for HiveMind's decision-quality / decision-importance scoring
(the basis for **confidence** and **reputation**). Written to preserve the research
context behind the design — read alongside the confidence+reputation design doc.*

## Why this document exists

HiveMind scores decisions on two axes — **Quality** (how well a decision was made) and
**Importance** (how much it matters) — assessed **ex ante** (only from what was knowable at
decision time, never the outcome). This file records the prior art we drew on, **how well
each is actually validated**, and how it maps to our model, so the reasoning isn't lost if
the originating discussion disappears.

## Core principle: separate decision *quality* from *outcome*

A decision must be judged on the reasoning and information available at the time, not on
how it turned out — good decisions can have bad outcomes and vice-versa ("resulting").
- **Annie Duke, _Thinking in Bets_ (2018)** — popularized "resulting": the error of
  judging decision quality by outcome.
- **Kahneman / Lovallo / Sibony** — structured, process-first strategic decision-making.

## Key sources

### 1. Decision Quality (DQ) — Spetzler, Winter & Meyer (2016)
*"Decision Quality: Value Creation from Better Business Decisions", Wiley.*
https://onlinelibrary.wiley.com/doi/book/10.1002/9781119176657
- Six requirements of a good decision **process**: (1) appropriate **frame**, (2) creative,
  doable **alternatives**, (3) relevant, reliable **information**, (4) clear **values /
  tradeoffs**, (5) sound **reasoning**, (6) **commitment** to action.
- Claim: *"a decision is only as good as its weakest link"* (minimum aggregation).
- Usually scored **subjectively** on a 0–100% spider chart; "100% DQ" = diminishing returns.
- **Standing: practitioner/consulting framework, NOT a validated psychometric metric.**
  Theoretically rooted in decision analysis (Howard / Stanford / Strategic Decisions Group)
  but the six-element packaging is pedagogical.
- **The weakest-link claim has no empirical validation** we could find — it is an
  assertion, not a tested aggregation rule. (We therefore do NOT hard-code minimum
  aggregation; see model below.)

### 2. Empirical test of DQ process quality — Group Decision & Negotiation (2026)
*"Quality of Decision-Making Processes in Teams…", Springer.*
https://link.springer.com/article/10.1007/s10726-026-09966-z
- SEM study, n=461 (Germany), operationalizing Spetzler's six elements for team decisions;
  process quality predicted decision **success**.
- **Caveats:** success was **self-reported/perceived** (not objective outcomes); the paper
  itself notes the framework "has so far not been commonly operationalized and tested in
  empirical research." → limited, emerging validation.

### 3. Adult Decision-Making Competence (A-DMC) — Bruine de Bruin, Parker & Fischhoff
*Original: (2007) J. Personality & Social Psychology* — https://pubmed.ncbi.nlm.nih.gov/17484614/
*Robustness/longitudinal: Parker et al. (2018) J. Behavioral Decision Making.*
*"More Than Intelligence?" (2020) Current Directions in Psych. Science* —
https://journals.sagepub.com/doi/full/10.1177/0963721420901592
- A **psychometrically validated** battery of 7 tasks (resistance to framing, recognizing
  social norms, under/overconfidence, applying decision rules, consistency in risk
  perception, resistance to sunk cost). Good reliability; predictive validity vs real-world
  outcomes; robust over an 11-year longitudinal study.
- **Standing: the well-validated decision-science instrument.** Caveat: measures a
  **person's trait competence via standardized tasks**, not the quality of a *single*
  decision from its artifacts — adjacent, not drop-in. We borrow its **validated
  bias-resistance constructs** for our "bias exposure" dimension.

### 4. Process quality predicts outcomes — Lovallo & Sibony (2010), w/ McKinsey
*"The case for behavioral strategy", McKinsey Quarterly.*
https://www.di.univr.it/documenti/OccorrenzaIns/matdid/matdid125734.pdf
- Survey of 2,207 executives / 1,000+ investments: decision **process** was **~6× more
  influential on outcomes than the depth of analysis**; structured de-biasing raised ROI by
  ~7 percentage points.
- **This is the strongest empirical support for our premise**: ex-ante *process* quality is
  real, measurable, and predictive — without endorsing the six-element/weakest-link packaging.

## Synthesis → what we adopt

- DQ is a **scaffold/taxonomy**, not an inherited metric. We **earn** the metric via our own
  validation (perturbation / dogfood + expert agreement / prospective revert-prediction).
- We do **not** assume weakest-link aggregation; we use a **weighted composite** and treat
  the weights as **empirically tunable** (fit them to the validation signal).
- We borrow A-DMC's **validated bias constructs** and lean on **Lovallo/Sibony** as the
  evidentiary foundation that process quality predicts outcomes.

## How it maps to HiveMind's model (summary; full spec in the design doc)

**Two axes, assessed ex ante, server-computed (Layer 3), stored as append-only annotations:**

- **Quality — bounded [0,1] / 0–100%** (how well-made). 7 dimensions:
  Framing · Alternatives · Information · Reasoning · Values&Tradeoffs · Bias exposure ·
  Calibration. (Calibration is **split out** from bias exposure: bias covers non-calibration
  distortions, calibration covers confidence-to-evidence matching; merge only if they
  empirically correlate >~0.7. Reversibility is **not** a quality dimension — it now lives in
  Importance.) → weighted composite = **Quality score**.

- **Importance — UNBOUNDED magnitude** (how much it matters); explicitly NOT a 0–100% score (a
  nuclear-crisis decision is categorically more important than any small-company decision).
  Form: **Importance = Stakes (unbounded, log-scaled; severity × reach) × Irreversibility
  (0–1 discount) × Actionability (0–1 gate)**. Reversible (two-way-door) decisions are
  discounted; an unactionable decision → ~0 importance.

- **Confidence** ≈ the Quality composite. **Reputation** = an actor's track record of
  Quality, **weighted by Importance** (being right on high-importance calls counts more).

- Dimensions stored individually so composites/weights can be **recomputed for free** as
  weighting improves; record the **weight-version** for reproducibility.

## Related internal docs
- `docs/DECISION_SCORING.md` — the design spec this reference supports (the locked model).
- `docs/ARCHITECTURE.md` — the 3-layer boundary (Layer 2 must not invent confidence; Layer 3
  is where this scoring lives).
- `docs/SEARCH_DESIGN.md` — "rank is an ordinal, not a confidence score."
- `docs/SCALE_TARGETS.md` — scale envelope (single-node Postgres, per-tenant).
- Competitor analyses (stigmem, Wasteland, cmem.ai/Dreaming): in the `hivemind-benchmarks`
  repo, `comparative-analysis/`.
