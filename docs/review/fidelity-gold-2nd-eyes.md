# Fidelity Gold — 2nd-Eyes Review
**Reviewer:** gastown.furiosa (polecat)  
**Bead:** hivemind-qx7v  
**Date:** 2026-06-19  
**Corpus:** /data/projects/hivemind-benchmarks/fidelity-bench/corpus.yaml (18 cases)  
**Contract ref:** LOCKED CONTRACT (alex, 2026-06-18), projector/mod.rs, events.rs

---

## Summary

Reviewed all 18 cases against the LOCKED CONTRACT and the implementation
(src/events.rs, src/projector/mod.rs). Found **5 substantive issues**
requiring mayor/alex decision, plus 3 documentation corrections.

**Cases confirmed correct** (no changes needed): A1, A2, A3, B1, B2, B3,
C1, C2, C3, D2, E1, E3 (confidence label), F1, F2, F3.

**Cases with issues**: D1, D3 (missing BlockedActor, type question),
E2 (likely fabrication), E3 (missing derived status), corpus header.

---

## Issue 1 — E2: AcceptedBy(d, marco) is likely a fabrication [CRITICAL]

**Case:** E2-meeting-decision-plus-tangent  
**Current gold:**
```yaml
nodes:
  - {kind: Decision, key: d, text: "cut CSV export from v1 (save two weeks)", status: accepted}
  - {kind: Actor, key: marco, text: "Marco"}
edges:
  - {kind: AcceptedBy, from: d, to: marco}
```

**Finding:** Marco's role in the text is "Marco will tell support about the
export change" — a notification/communication task. The decision was made
collectively ("Decided: cut the CSV export from v1") with no named
decision-maker. AcceptedBy implies Marco accepted/approved the decision, but
the text establishes only a communication role. This is a fabrication.

The comment says the case "must NOT become nodes (precision under distraction)"
for the coffee machine/naming chatter — the same precision discipline should
apply to the AcceptedBy edge: Marco is NOT the decision acceptor.

**Proposed correction:**
```yaml
nodes:
  - {kind: Decision, key: d, text: "cut CSV export from v1 (save two weeks)", status: accepted}
edges: []
# No named decision-maker; Marco's role is notification execution, not
# acceptance. Fabricating AcceptedBy(d, marco) is exactly the precision
# penalty this case tests against.
```

---

## Issue 2 — D1/D3: Missing BlockedActor edges [SIGNIFICANT]

**Cases:** D1-launch-blocked-legal, D3-deploy-gate-blocked

**Finding:** `BlockerReportedPayload.blocked_actor_id` is **required** in
events.rs (line 257). The projector at mod.rs:411-416 ALWAYS emits
`BlockedActor(blocker_id, blocked_actor_id)` for every `blocker.reported`
event. Neither D1 nor D3 include BlockedActor in their gold.

- D1: Priya "raised the request" and is implicitly blocked waiting for Sam's
  sign-off. `BlockedActor(blk, priya)` would be emitted by capture.
- D3: Mia is the "release owner" who can't proceed. `BlockedActor(blk, mia)`
  would be emitted.

If the evaluator runs real capture and diffs vs gold, these would score as
false positives, unfairly penalizing capture recall.

**Proposed correction for D1:**
```yaml
edges:
  - {kind: DecisionRequestedBy, from: req, to: priya}
  - {kind: BlockedActor, from: blk, to: priya}       # ADD: required by schema
  - {kind: BlockerForDecision, from: blk, to: req}
  - {kind: BlockerRequiredOwner, from: blk, to: sam}
```

**Proposed correction for D3:**
```yaml
edges:
  - {kind: DecisionRequestedBy, from: req, to: mia}
  - {kind: BlockedActor, from: blk, to: mia}         # ADD: required by schema
  - {kind: BlockerForDecision, from: blk, to: req}
```

**Note:** This also means the D1/D3 Actor nodes for priya/mia are already in
the gold (via DecisionRequestedBy), so no new nodes are needed.

---

## Issue 3 — D1/D3: BlockerForDecision type mismatch [SIGNIFICANT]

**Cases:** D1, D3

**Finding:** Gold uses `{kind: BlockerForDecision, from: blk, to: req}` where
`req` is a `DecisionRequest` node. But the projector's type constraint at
mod.rs:150 specifies:
```rust
Self::BlockerForDecision => (NodeKind::Blocker, NodeKind::Decision),
```
The endpoint should be `Decision`, not `DecisionRequest`. There is no
`BlockerForDecisionRequest` edge type.

Two options:
- (a) If blockers can block DecisionRequests, add a `BlockerForDecisionRequest`
  edge kind and update the schema, or relax the type constraint.
- (b) If `BlockerForDecision` is intentionally overloaded to cover both
  Decision and DecisionRequest targets, update the projector type constraint
  and document this.

**Proposed resolution:** Mayor/alex to decide. If (a), a schema extension is
needed. If (b), update projector type comment and corpus header.

---

## Issue 4 — E3: Missing derived status [MINOR]

**Case:** E3-tentative-low-confidence-decision  
**Current gold:** Decision node has `confidence: low` but no `status:` label.

**Finding:** All other Decision nodes in the gold show a derived `status:`
(downstream consequence check per contract). A low-confidence lean that hasn't
been formally confirmed has derived status `proposed` (proposed but not
accepted). E3's Decision should show `status: proposed` for consistency.

**Proposed correction:**
```yaml
- {kind: Decision, key: d, text: "use gRPC for the internal API", confidence: low, status: proposed}
```

---

## Issue 5 — Confidence field not in schema [CRITICAL — schema gap]

**Case:** E3 (and the LOCKED CONTRACT generally)

**Finding:** The LOCKED CONTRACT specifies "Decisions carry EXPRESSED
confidence {low|medium|high}". E3's gold expects `confidence: low` on the
Decision node. However:

- `DecisionProposedPayload` (events.rs:165-178) has NO `confidence` field.
  Only `extraction_confidence: f64` exists (Haiku's self-assessment, not
  expressed decision confidence).
- The projector does not store any expressed confidence on Decision nodes.

This means:
1. The capture pipeline cannot currently emit `confidence: low/medium/high`
   on a Decision — the field doesn't exist in the event schema.
2. E3's expected `{kind: Decision, confidence: low}` tests a feature that
   isn't implemented. The evaluator would never score a match on `confidence`.

**Proposed resolution (two options):**
- (a) Add `confidence: Option<String>` to `DecisionProposedPayload`, update
  the projector to store it on Decision nodes, update the classifier prompt
  to extract expressed confidence. This makes E3 fully scoreable.
- (b) Mark `confidence` scoring as **Phase 2** in the corpus (not scored in
  Phase 1 evaluator), treating it like `status` (shown as label, not target).

Mayor/alex to choose. If (b), the corpus comment on E3 should clarify:
"confidence label shown for reference; not scored in Phase 1 evaluator."

---

## Issue 6 — ProposedBy edge in evaluator scoring [DESIGN QUESTION]

**Affects:** All cases with Decision nodes (A1–F1) where no actor is named.

**Finding:** `project_event` for `DecisionProposed` (mod.rs:204-209) ALWAYS
emits `ProposedBy(decision_id, actor_id)`. The evaluator will use a known
actor (e.g., `evaluator:system`) when running the capture pipeline. This will
produce `ProposedBy(d, "evaluator:system")` for every Decision case.

The gold does NOT include ProposedBy edges for anonymous cases (A1-A3, B1-B3,
C1-C3, E1, E3, F1). These would all appear as false positives in the evaluator
scorecard, unfairly penalizing capture precision.

**Proposed resolution:** Exclude Actor-linked projected edges from scoring
unless the actor is **named in the input text**. This aligns with the corpus
design principle: actor edges score only when input names an actor (D1, D2, D3,
E2). Document this exclusion rule in the corpus header.

**Alternative:** Include `ProposedBy(d, "evaluator:system")` in the gold for
all Decision cases — but this makes the corpus less readable and couples it to
the evaluator's actor_id.

---

## Documentation Corrections (non-blocking)

### Doc-1: Corpus header edge list incomplete
The header lists projected edges: `ProposedBy, AcceptedBy, RejectedBy,
Supersedes, BlockerForDecision, BlockedActor, DecisionRequestForDecision`.
Missing: `DecisionRequestedBy, DecisionRequestRequiredOwner, BlockerRequiredOwner`.
All three appear in D1 cases. Add them to the header.

### Doc-2: Edge notation inconsistency
Semantic edges: `BASED_ON, HAS_OPTION, CHOSE, ASSUMES, SUPPORTS, REFUTES,
SUPERSEDES` (CAPS_CASE — matches string representation in code).
Projected edges: `ProposedBy, AcceptedBy, ...` (CamelCase — matches enum
variant names, not string representations like "PROPOSED_BY").
The evaluator must normalize both forms. Document this in header or
standardize to one convention.

### Doc-3: Confidence label completeness
Per contract: "unlabelled decisions = firm/high". The gold omits explicit
`confidence: high` on A1–F1 Decision nodes. The evaluator should default
to `high` for unlabelled decisions. Confirm this is the intended behavior
and document the default rule explicitly in the corpus header.

---

## Confirmed Correct (no changes needed)

| Case | Verdict |
|------|---------|
| A1-ledger-store | Correct. Options, Evidence, edges all well-formed. |
| A2-frontend-framework | Correct. Intentional Evidence omission (experience, not data). |
| A3-cloud-region | Correct. Three options, two Evidence, correct edges. |
| B1-pricing-reversal | Correct. Supersession + Evidence. |
| B2-repo-topology-double | Correct. Double supersession chain. |
| B3-auth-vendor-switch | Correct. Superseded + new with options. |
| C1-perf-hypothesis-refuted | Correct. ASSUMES + REFUTES + BASED_ON. |
| C2-growth-assumption-supported | Correct. ASSUMES + SUPPORTS + BASED_ON. |
| C3-threat-model-mixed | Correct. Mixed support/refute. BASED_ON to rank only is defensible. |
| D2-hiring-contested | Correct. ProposedBy + RejectedBy → contested is emergent, correct. |
| E1-slack-implicit-option | Correct. Implicit option (cron) correctly captured. |
| E3 (confidence label) | Correct. `confidence: low` is right for the lean. Schema gap is separate issue. |
| F1-unilateral-no-alternatives | Correct. Bare Decision, empty edges. |
| F2-status-update-non-decision | Correct. Empty gold. |
| F3-request-only-no-decision | Correct. DecisionRequest only, no edges. |

---

## Priority Order for Corrections

1. **Issue 5** (schema gap: confidence field) — blocks E3 from being scoreable
2. **Issue 1** (E2 fabrication) — incorrect edge in gold
3. **Issue 2** (D1/D3 missing BlockedActor) — gold misses required projected edges
4. **Issue 3** (BlockerForDecision type) — possible type constraint violation
5. **Issue 6** (ProposedBy evaluator design) — evaluator design question
6. **Issue 4** (E3 status) — minor consistency
7. **Doc-1/2/3** — header corrections
