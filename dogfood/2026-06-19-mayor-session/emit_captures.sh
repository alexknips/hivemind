#!/usr/bin/env bash
# Emit all classified captures from the mayor session dogfood
# Labelled as STRONG-EXTRACTOR CEILING (Sonnet-as-classifier)
set -e

BIN="/home/ubuntu/gc/.gc/worktrees/hivemind/polecats/gastown.furiosa/target/release/hivemind"
export HIVEMIND_DIR="/tmp/hivemind-dogfood/hivemind"
ALEX="--actor human:alex.knips@gmail.com"
MAYOR="--actor agent:claude:gastown.mayor"

hm() { "$BIN" "$@"; }

echo "=== Emitting dogfood captures ==="

# ── EVIDENCE 1: Pool scaling root cause (chunk_038) ──
E_POOL=$(hm $ALEX emit evidence.recorded \
  --content "gc sling to a pool does NOT clear a bead pre-existing assignee. scale_check only counts unassigned-demand, so beads already assigned to crew/refinery are invisible — only 1 polecat spawns instead of N. Verified: 6 UI beads were assigned to the crew; after clearing assignees the pool scaled 1 to 8 polecats.")
echo "E_POOL: $E_POOL"

# ── DECISION 1: Pool scaling fix (chunk_038) ──
D_POOL=$(hm $MAYOR emit decision.proposed \
  --title "When routing to a pool, always clear the bead pre-existing assignee" \
  --rationale "gc sling does not clear assignees; scale_check skips beads with an assignee set, so the pool never scales. Fix: after gc sling, also run gc bd update id --assignee empty. Baked into the overnight heartbeat dispatch rule." \
  --topic-keys "pool-scaling,dispatch,gc-sling,polecat,workflow" \
  --options "clear-assignee-on-sling,leave-assignee-as-is" \
  --chose "clear-assignee-on-sling" \
  --evidence "$E_POOL")
echo "D_POOL: $D_POOL"
hm $ALEX emit decision.accepted --decision-id "$D_POOL" 2>&1

# ── DECISION 2: STUB classifier for .12, polling sidecar over inotify (chunk_043/046) ──
D_STUB=$(hm $MAYOR emit decision.proposed \
  --title "Stub ingest classifier (return 202) for capture client .12; real Haiku classifier is separate follow-on bead" \
  --rationale "Correct per AGENTS.md section 3: classifier = swappable Layer-3, NOT inline in ingest. Ingest must still validate and append raw evidence before 202 for provenance. Polling sidecar chosen over inotify: inotify breaks the no-pip onboarding bar (AGENTS.md section 7)." \
  --topic-keys "classifier,ingest,sidecar,layer-separation,architecture" \
  --options "stub-classifier-now,implement-classifier-inline,inotify-sidecar" \
  --chose "stub-classifier-now")
echo "D_STUB: $D_STUB"
hm $ALEX emit decision.accepted --decision-id "$D_STUB" 2>&1

# ── DECISION 3: Hook + sidecar share ONE ingest-client core (chunk_043) ──
D_CORE=$(hm $MAYOR emit decision.proposed \
  --title "Hook and sidecar shippers must share a single ingest-client core" \
  --rationale "Apples-to-apples for the later real-world comparison alex asked for. If they diverge they test different paths. Only the trigger differs (file signal vs polling); the ingest path is identical." \
  --topic-keys "capture,ingest,architecture,sidecar,hook" \
  --options "shared-core,separate-implementations" \
  --chose "shared-core")
echo "D_CORE: $D_CORE"
hm $ALEX emit decision.accepted --decision-id "$D_CORE" 2>&1

# ── DECISION 4: extraction_confidence rename (chunk_047-048) ──
D_CONFNAME=$(hm $MAYOR emit decision.proposed \
  --title "Rename classifier per-capture confidence field to extraction_confidence" \
  --rationale "A bare confidence field would get conflated with the real Quality+Importance composite score. extraction_confidence is the classifier self-estimate that a capture was pulled correctly, NOT the decision quality score. Bare confidence would trip the no-invented-confidence honesty rail (AGENTS.md section 6)." \
  --topic-keys "classifier,schema,confidence,capture,naming" \
  --options "rename-to-extraction-confidence,keep-bare-confidence" \
  --chose "rename-to-extraction-confidence")
echo "D_CONFNAME: $D_CONFNAME"
hm $ALEX emit decision.accepted --decision-id "$D_CONFNAME" 2>&1

# ── DECISION 5: Defer 2-axis scoring model to post-PoC (chunk_048) ──
D_DEFER=$(hm $MAYOR emit decision.proposed \
  --title "Defer full 2-axis scoring model (Quality+Importance) to post-PoC; classifier ships as honest extractor only" \
  --rationale "Sequencing concern: validated Quality+Importance scoring is Layer-3 / built-later. Shipping the extractor now (decision/evidence/hypothesis with rationale and evidence) is the right first step. Filed uuq9.19 to track the deferred scorer so the deferral is explicit." \
  --topic-keys "scoring,classifier,sequencing,layer-3,defer" \
  --options "defer-scoring-to-post-poc,implement-scoring-now" \
  --chose "defer-scoring-to-post-poc")
echo "D_DEFER: $D_DEFER"
hm $ALEX emit decision.accepted --decision-id "$D_DEFER" 2>&1

# ── DECISION 6: M3 reframe — findable and digestible (chunk_093-094) ──
D_M3=$(hm $MAYOR emit decision.proposed \
  --title "Reframe M3 as Decisions are findable and digestible (search plus summarization), not governance" \
  --rationale "Governance (contest/supersede) is premature before there is enough trusted history worth governing. For the 5-friend PoC, concrete demonstrable capabilities trump outcome claims. M3 absorbs old M4 (search); summarization and compactification is the second pillar. Governance moves to a later milestone." \
  --topic-keys "milestone,m3,search,summarization,product-direction" \
  --options "findable-and-digestible,governance-ux,adoption-dogfood" \
  --chose "findable-and-digestible")
echo "D_M3: $D_M3"
hm $ALEX emit decision.accepted --decision-id "$D_M3" 2>&1

# ── HYPOTHESIS 1: governance is premature for 5-friend PoC ──
H_GOV=$(hm $ALEX emit hypothesis.recorded \
  --statement "Governance features (contest/supersede UX) have low value for the 5-friend PoC because there is no established history to govern yet; their value appears only after heavy regular use.")
echo "H_GOV: $H_GOV"

# ── DECISION 7: Expressed confidence ordinal locked (chunk_147-149) ──
D_CONF=$(hm $MAYOR emit decision.proposed \
  --title "Capture expressed confidence as 3-level ordinal low/medium/high from decider words; never computed by the system" \
  --rationale "Lean = low-confidence decision (captured, not dropped). The decider stated certainty maps: probably/leaning = low, reasonably confident but caveated = medium, decided/committed = high. This is an attribute of the event, not a system calculation. Confidence changes stay real events (append-only). 2 levels via proposed/accepted insufficient: medium tier required for granularity." \
  --topic-keys "confidence,capture-schema,honesty,events,decisions" \
  --options "3-level-expressed-confidence,2-level-event-only,numeric-float-confidence" \
  --chose "3-level-expressed-confidence")
echo "D_CONF: $D_CONF"
hm $ALEX emit decision.accepted --decision-id "$D_CONF" 2>&1

# ── DECISION 8: Extend capture classifier to full typed graph before scorecard (chunk_152) ──
D_EXTEND=$(hm $MAYOR emit decision.proposed \
  --title "Extend capture classifier to full typed graph (SUPERSEDES/ASSUMES/SUPPORTS/REFUTES + actor-links + expressed confidence) before running first fidelity scorecard" \
  --rationale "The existing classifier covers only about half the typed graph (missing relational edges, actor-linkage, expressed confidence). A throwaway fidelity-only extractor would inflate scorecard numbers against something that never ships. Extending the real classifier first means the scorecard measures the real improved system." \
  --topic-keys "classifier,capture,fidelity,typed-graph,m3x" \
  --options "extend-real-classifier,throwaway-fidelity-extractor" \
  --chose "extend-real-classifier")
echo "D_EXTEND: $D_EXTEND"
hm $ALEX emit decision.accepted --decision-id "$D_EXTEND" 2>&1

# ── DECISION 9: Beyond-coarse authorship as research question (chunk_167) ──
D_RESEARCH=$(hm $MAYOR emit decision.proposed \
  --title "Dispatch finer authorship attribution (beyond coarse) as research question hmd-u1v, not immediate implementation" \
  --rationale "How to measure human-AI authorship/agency/locus-of-control from observable signals is an open research question. Build first on coarse mode (uzgg), then let research findings drive whether and how to go finer. Research runs parallel; implementation deferred until findings arrive." \
  --topic-keys "authorship,attribution,research,defer,hmd-u1v" \
  --options "research-question-first,implement-now" \
  --chose "research-question-first")
echo "D_RESEARCH: $D_RESEARCH"
hm $ALEX emit decision.accepted --decision-id "$D_RESEARCH" 2>&1

# ── DECISION 10: Alex unlocks chain: benchmarks/ UBS exemption (chunk_168) ──
D_UBS=$(hm $ALEX emit decision.proposed \
  --title "Exempt benchmarks/ and src/projector/tests.rs from UBS warning baseline; approve 07no retroactively" \
  --rationale "Non-production code (evaluator bench code and test modules) carries the same rationale as alex-approved tests/ exemption. 07no classifier/projector/events.rs production code stays strict at 3039. This unblocks the full capture-extension chain: 21zi then 07no then uzgg." \
  --topic-keys "ubs,baseline,exemption,tests,ci" \
  --options "exempt-non-production-code,no-exemption,raise-baseline" \
  --chose "exempt-non-production-code")
echo "D_UBS: $D_UBS"
hm $ALEX emit decision.accepted --decision-id "$D_UBS" 2>&1

# ── DECISION 11: Python eval steer — Rust v0 disposable (chunk_168-169) ──
D_PYTHON=$(hm $MAYOR emit decision.proposed \
  --title "Rust fidelity evaluator (21zi) lands as disposable v0; no further Rust eval extensions; richer long-term eval defaults to Python" \
  --rationale "alex: might not even want Rust for benchmarks long run, don't over invest. v0 gives first structural-match scorecard immediately (build is done). Future eval work (semantic matching, LLM-judge, authorship measurement, uplift A/B) is more natural in Python. Python eval also removes benchmarks/ from product UBS/CI gates permanently." \
  --topic-keys "eval,python,rust,benchmarks,scope" \
  --options "rust-v0-then-python-future,extend-rust-eval,full-python-now" \
  --chose "rust-v0-then-python-future")
echo "D_PYTHON: $D_PYTHON"
hm $ALEX emit decision.accepted --decision-id "$D_PYTHON" 2>&1

# ── DECISION 12: Docker fix — scope to --bin hivemind (chunk_171) ──
D_DOCKER=$(hm $MAYOR emit decision.proposed \
  --title "Scope Dockerfile cargo build to --bin hivemind; exclude fidelity-eval binary from prod image" \
  --rationale "The fidelity evaluator binary is a throwaway eval tool and should not build in the prod Docker image. The fix permanently decouples eval from product docker and CI. Adding benchmarks/ to Docker layers would build the throwaway eval in prod, which is over-investment against the eval steer." \
  --topic-keys "docker,ci,eval,dockerfile,build" \
  --options "exclude-eval-bin,add-benchmarks-to-docker" \
  --chose "exclude-eval-bin")
echo "D_DOCKER: $D_DOCKER"
hm $ALEX emit decision.accepted --decision-id "$D_DOCKER" 2>&1

# ── EVIDENCE 2: 07no bead-state lesson (chunk_175) ──
E_BEAD=$(hm $MAYOR emit evidence.recorded \
  --content "Pool reconciler skips beads that are BLOCKED status or assigned to a non-self/mayor agent. The 07no slings (wk2s, i0pz) made convoys but no polecat claimed because 07no was BLOCKED and assigned to gastown.mayor. Fix: set status=open and cleared assignee; polecat claimed immediately. Lesson: a slung polecat will not pick up a BLOCKED or non-self/mayor-assigned bead.")
echo "E_BEAD: $E_BEAD"

# ── DECISION 13: Coarse authorship model design (chunk_163-167) ──
D_AUTH=$(hm $MAYOR emit decision.proposed \
  --title "Coarse authorship model: Actor.kind human or agent; capture agent as participant; session-initiator as INITIATED_BY edge; mode DERIVES from participant kinds" \
  --rationale "Coarse mode {human-only, human+AI, AI-autonomous} derives from which participant kinds are present — no stored mode field. Descriptive only; observable and mark-inferred and never-invent. Defer: who-leads-within-human+AI, contribution-roles. session_initiator implemented as INITIATED_BY edge (Decision to Actor) for traversability, consistent with ProposedBy/AcceptedBy/RejectedBy." \
  --topic-keys "authorship,actor,participants,session-initiator,schema" \
  --options "coarse-derive-mode,store-mode-field,no-authorship-capture" \
  --chose "coarse-derive-mode")
echo "D_AUTH: $D_AUTH"
hm $ALEX emit decision.accepted --decision-id "$D_AUTH" 2>&1

# ── DECISION 14: Compactification semantics (chunk_094, 150) ──
D_COMPACT=$(hm $MAYOR emit decision.proposed \
  --title "Compactification removes noise never signal: main decisions + rationale + current status kept; superseded intermediate steps and redundant evidence compacted reversibly" \
  --rationale "Per AGENTS.md principle. Signal = current decisions + rationale + status. Noise = superseded intermediate steps, redundant evidence already captured elsewhere, pure planning narration without a decision. Forgetting is itself a tracked reversible event." \
  --topic-keys "compactification,signal-noise,graph,semantics,m3" \
  --options "signal-forward-compactification,full-retention" \
  --chose "signal-forward-compactification")
echo "D_COMPACT: $D_COMPACT"
hm $ALEX emit decision.accepted --decision-id "$D_COMPACT" 2>&1

# ── DECISION 15: Milestones as product-wide hq coordination beads (chunk_093) ──
D_MILESTONES=$(hm $MAYOR emit decision.proposed \
  --title "Milestones are product-wide concepts defined as coordination beads in the hq store; existing per-rig milestones stay as-is; product-wide scheme starts with M3" \
  --rationale "Per-rig epics (backend M1 CLI=v0.1, M2=v0.2; UI M1=v0.1) stay rig-local. New hq coordination bead for M3 points at per-rig execution epics by ID. Mayor tracks the union. No back-fix: existing milestones unchanged." \
  --topic-keys "milestone,coordination,hq-store,product-structure" \
  --options "hq-coordination-beads,per-rig-milestones-only" \
  --chose "hq-coordination-beads")
echo "D_MILESTONES: $D_MILESTONES"
hm $ALEX emit decision.accepted --decision-id "$D_MILESTONES" 2>&1

echo ""
echo "=== All captures emitted ==="
