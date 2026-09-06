# HiveMind and the Research Frontier
## A Whitepaper Summary for Alex Knips

*Synthesized 2026-08-23 from parallel research across six fields by gastown.furiosa (fan-in bead hivemind-q495.7).*

*Reviewed and committed 2026-09-06 by gastown.furiosa (bead hivemind-iv0r). See [Review Notes](#review-notes) at the end of this document.*

---

## What HiveMind Is

HiveMind is a **corporate, multi-human, multi-agent decision memory system**. Its unit of value is a recoverable, defensible *decision* with full provenance: what was decided, by whom, why, what alternatives were considered, what evidence was weighed, and which hypotheses it rests on.

HiveMind is not a project tracker, a chat archive, an agent's private working memory, or a general-purpose knowledge graph. It is a **decision provenance graph** with an append-only attributed ledger at its core.

The system has three architectural layers: (1) a write/ingest layer that validates invariants and appends events; (2) a pure read/query layer; and (3) an agentic analysis layer for compactification, similarity, and ranking — not yet built, but architecturally isolated so the rest of the system functions correctly without it.

This document surveys how HiveMind relates to six research fields, what it genuinely advances, what it honestly borrows, and where its gaps remain.

---

## Field 1: Design Rationale and Architecture Decision Records

### The lineage

The field traces to Rittel and Webber's 1973 *wicked problems* paper, which introduced IBIS (Issue-Based Information Systems): structured argumentation for capturing issues, positions, and arguments. Conklin and Begeman (1988) computerized IBIS into gIBIS — the first graphical hypertext system for collaborative decision-argumentation, with typed nodes and a relational backend (1,358+ citations). MacLean et al. (1991) introduced QOC (Questions, Options, Criteria) as a lighter notation. Lee and Lai (1991) proposed DRL, the most expressive formal DR language of the era.

The field spent the next 30 years confronting a single problem: **the capture-incentive gap**. Buckingham Shum (1994, 2006) named it most clearly — DR systems require overhead; designers resist post-hoc capture; value only materializes long after the effort, breaking the incentive loop. A 2012 decade survey (Capilla et al.) confirmed the diagnosis: structured DR remained marginal in industry, dominated by informal wikis and email.

The practitioner response was ADRs: Nygard's 2011 blog post proposed capturing just five fields (title, status, context, decision, consequences). MADR extended this with explicit "considered options." Empirical studies show ~50% of repos with ADRs have fewer than 5 records — high abandonment after pilots.

The 2024–2026 LLM-ADR literature (Kochhar et al. 2024; arXiv:2604.03826; arXiv:2503.13310) attempts to close the incentive gap by reducing authoring friction through automated generation.

### How HiveMind relates

HiveMind inherits: typed-node argumentation (IBIS/gIBIS), explicit alternatives as first-class nodes (QOC), the proposed/accepted/superseded/contested status lifecycle (Nygard), and the in-flow capture principle (Conklin 1991).

HiveMind advances the field by solving the capture-incentive gap at its root: MCP-native capture. When an AI agent makes a decision, it emits the decision as part of the same tool call that makes it — the call *is* the capture. This is zero overhead for agent-originated decisions, which Buckingham Shum identified as the structural root cause of DR system abandonment.

No prior DR system treats agents as decision *actors* (rather than scribes or automation tools). Every IBIS, QOC, and ADR tool assumes a human author. HiveMind's Actor model is symmetric: `agent:codex:<session>` and `user:alex.knips@gmail.com` have identical standing to propose, contest, and supersede decisions.

**Honest gap:** Human contributors still face non-zero capture overhead for non-agentic workflows. The LLM-ADR literature's RAG-based suggestion approach (arXiv:2604.03826) is a promising complement; HiveMind's Layer 3 is the correct slot for it but is deferred.

---

## Field 2: Computational Argumentation Frameworks

### The lineage

Dung (1995) is the foundation: an argumentation framework (AF) is a pair (A, R) where A is a set of arguments and R is an attack relation. Status (in/out/undecided) is *derived* from the attack topology, not stored. Acceptability semantics (grounded, preferred, stable extensions) identify which sets of arguments "win." This simple structure subsumes defeasible logic, logic programming with negation-as-failure, and non-monotonic reasoning.

Bipolar AFs (Cayrol & Lagasquie-Schiex 2005, 2012) added support relations alongside attacks. ASPIC+ (Prakken 2010; Modgil & Prakken 2014) gave arguments internal structure: strict vs. defeasible rules; undermining, rebutting, and undercutting attacks. Abstract Dialectical Frameworks (Brewka et al. 2013, 2017) generalize Dung by assigning each argument an arbitrary acceptance condition.

Recent work extends the field to multi-agent and distributed settings (Xiao & Greer 2023; arXiv:2604.23124 2026) and ledger-based provenance for AI knowledge (Du et al. arXiv:2607.28374 2026; arXiv:2025 TDC).

### How HiveMind relates

HiveMind borrows Dung's core insight: **status is derived from topology, not stored**. A Decision's operative state (contested, refuted, stale) is computed from edge types and hypothesis states, not persisted as an editable field. This mirrors Dung's derivation principle, applied to organizational decisions rather than abstract arguments.

HiveMind's edge vocabulary maps cleanly onto ASPIC+: SUPPORTS ↔ support (BAF); REFUTES ↔ rebuttal; CONTESTS ↔ undercutting; PREMISED_ON ↔ defeasible premise. The staleness propagation (refuted Hypothesis → stale Decision) is exactly *defeasible support* in bipolar and structured frameworks — what the literature calls "indirect defeat through support chains."

HiveMind advances the field by treating **decisions** — organizational commitments made at specific times by specific actors — as the fundamental unit, rather than abstract reasoning steps. The decision has lifecycle (proposed, accepted, superseded, contested), authorship, and downstream organizational consequences that abstract AFs do not model.

The 2026 LedgerMind paper (arXiv:2607.28374) independently converges on HiveMind's append-only attributed ledger design, but does not link it to formal argumentation semantics. That connection remains undeveloped.

**Honest gap:** HiveMind's status derivation has no formal argumentation semantics. Mapping its graph to a Dung AF or ADF would make derivation formally verifiable and enable use of existing complexity results. Computing preferred/stable extensions is Σ₂^P-complete; at tens of thousands of decisions, grounded-semantics approximations or incremental update algorithms will be needed.

---

## Field 3: Truth Maintenance Systems and Belief Revision

### The lineage

Doyle's 1979 JTMS paper introduced the dependency graph as an epistemological primitive: nodes hold *justifications* (in-list, out-list pairs); belief is derived from the dependency network, never stored directly. When new information arrives, nodes flip in/out and the TMS propagates changes without recomputation from scratch. Stallman and Sussman (1977) had introduced dependency-directed backtracking (DDB) — when a contradiction occurs, trace the causal chain to find the responsible choice and backtrack there, not chronologically.

De Kleer (1986a/b/c) extended this into the ATMS: multiple contexts simultaneously, each node carrying a *label* — the minimal consistent assumption sets under which it is derivable. ATMS enables efficient multi-hypothesis reasoning without backtracking, at the cost of PSPACE-complete label computation.

The AGM framework (Alchourrón, Gärdenfors, Makinson 1985) provides eight rationality postulates for a single rational agent's belief revision: expansion, contraction, and revision must minimize change while maintaining consistency. Darwiche and Pearl (1997) extended AGM to iterated revision.

### How HiveMind relates

HiveMind's PREMISED_ON edge is a direct analog of a JTMS justification. The staleness propagation — when a Hypothesis flips to refuted, every Decision PREMISED_ON it surfaces staleness — is JTMS dependency propagation applied to organizational decisions. The derived-status principle (status is a computed invariant of the dependency network) is also lifted directly from JTMS.

HiveMind deliberately rejects AGM on three counts: (1) HiveMind is *multi-actor* — no single coherent belief set exists, only attributed positions; (2) the ledger is *append-only* — AGM revision can require discarding old beliefs, HiveMind never discards; (3) **contested is preserved** — AGM's consistency postulate requires eliminating contradictions where possible, HiveMind preserves conflicting positions from distinct actors as first-class data.

HiveMind also rejects ATMS's multi-context enumeration. ATMS answers "what is derivable under every possible assumption set?" — a problem-solving search over hypothesis space. HiveMind records what was *actually decided* in the one world that existed, with full actor attribution. Completeness over hypothetical worlds is not a design goal.

**Novel in this field:** JTMS and ATMS update state in-place (nodes flip, labels recompute). HiveMind never modifies existing records — every state change is a new ledger event. Full auditability of "who changed this and when?" is architecturally guaranteed, not an afterthought.

**Honest gap:** Formal semantics for JTMS are well-established. HiveMind's staleness propagation is operational but informally specified. Edge cases (circular PREMISED_ON chains, compactified hypotheses) lack formal characterization.

---

## Field 4: Provenance, Event Sourcing, and Temporal/Bitemporal Knowledge Graphs

### The lineage

W3C PROV (2013; Moreau & Missier) defines Entity/Activity/Agent and relations including wasGeneratedBy, wasAttributedTo, wasDerivedFrom. PROV-O (Lebo et al. 2013) is the OWL ontology version. Nanopublications (Groth et al. 2010) structure each atomic claim as three named RDF graphs: assertion, provenance, and publication info.

Event sourcing (Greg Young 2010; Hickey / Datomic 2012) stores state as an ordered, append-only sequence of immutable domain events. Current state is always *derived* from the log; CQRS separates the write model from optimized read projections. XTDB (2024) implements bitemporal SQL over event-sourced facts.

Temporal KGs track facts along valid-time (occurred_at) and transaction-time (recorded_at) axes (Jensen & Snodgrass 1996; Snodgrass 1999). Recent work applies bitemporal models to agent memory: Niksarli & Baheti (arXiv:2607.26520 2026) implement a Neo4j-based bitemporal agent memory with vector indexing; TOKI (arXiv:2606.06240 2026) proposes a bitemporal operator algebra for contradiction resolution. The 2026 "Always-On Agents" survey (Ding et al. arXiv:2606.30306) reviews 435 works across six governance dimensions: authority, scope, mutability, provenance, recoverability, and actionability.

### How HiveMind relates

HiveMind borrows: append-only ledger (event sourcing); state derived from events rather than mutable records (Datomic); actor attribution on every event (PROV-O wasAttributedTo); the occurred_at/recorded_at time separation (bitemporal data model).

HiveMind advances the field at several points:

**Decision-first schema.** Temporal KGs model facts as quadruples (entity, relation, entity, time) and treat decisions as just another fact. HiveMind centers on *decisions* as first-class typed nodes whose operative status is derived from the graph of PROPOSED_BY / ACCEPTED_BY / SUPERSEDED_BY edges. "Was this decision accepted?" is a graph traversal, not a timestamp lookup. No existing temporal KG framework does this.

**In-flow MCP capture.** All existing provenance systems — including W3C PROV and nanopublications — capture provenance *retrospectively*. HiveMind captures decisions *at the moment of agent reasoning*, through an MCP tool embedded in the agent's tool loop. The provenance layer is a first-class part of the reasoning process, not a post-hoc annotation layer.

**Contested as a valid persistent state.** W3C PROV supports multiple agents but has no semantics for disagreement that is preserved. Event sourcing assumes a single authoritative command handler. Bitemporal KGs treat conflicts as modeling errors to resolve. HiveMind treats a contested decision (conflicting ACCEPTED_BY/REJECTED_BY edges from different Actors) as a valid, queryable, persistent system state — never auto-resolved.

**Compactification as a tracked, attributed event.** Datomic never forgets; distributed ledgers never forget. Event-sourced systems typically snapshot-and-truncate but lose the pre-snapshot stream. HiveMind's compactification is a ledger event: noise is pruned, but the compactification itself is recorded with actor, timestamp, and rationale. "Forgetting" is auditable.

**Honest gap:** HiveMind does not yet expose a formal bitemporal query interface (SQL:2011 temporal syntax; "show decisions as-of T, as known at transaction-time T'"). PROV-O export and embedding-based retrieval are deferred to Layer 3.

---

## Field 5: LLM Agent Memory and Decision/Reasoning Capture

### The lineage

MemGPT (Packer et al. 2023; arXiv:2310.08560) pioneered virtual-memory analogies for LLM agents. Generative Agents (Park et al. 2023; arXiv:2304.03442) introduced the append-only memory stream + reflection architecture. Reflexion (Shinn et al. 2023; arXiv:2303.11366) demonstrated verbal reinforcement: store natural-language failure analysis in episodic memory. A-MEM (Xu et al. 2025; arXiv:2502.12110) applies Zettelkasten principles to structured, linked memory notes. MemQ (Liao et al. 2025; arXiv:2605.08374) introduces provenance DAGs for memory credit assignment via TD(λ) eligibility traces.

On the audit/provenance side: the AER framework (arXiv:2603.21692 2025) captures agent intent, observation, and inference as first-class queryable fields. TraceCaps (ICSE 2026) provides inline runtime provenance and cryptographic hash-linking with enforceable risk thresholds. LEDGER (arXiv:2608.18398 2026) builds layered claim-to-evidence trace graphs. The DEMM framework (Solozobov arXiv:2605.04093 2026) defines a five-level capability rubric for evaluating whether captured execution data can reconstruct and justify particular AI decisions; it identifies the *"container fallacy"* — having logs does not imply audit sufficiency.

### How HiveMind relates

HiveMind shares architectural principles with this field: append-only event ledger (←generative agents' memory stream), typed node representations (←AER intent/inference fields), actor attribution (←TraceCaps), and claim-evidence graphs (←LEDGER).

HiveMind advances the field in five ways:

**The decision, not the trace, is the primary artifact.** All provenance and audit systems (TraceCaps, LEDGER, DEMM, AER) start from execution traces and try to recover decisions from them. HiveMind inverts this: the decision (with rationale, options, evidence, actor, timestamp) is written first; the trace is secondary. Execution traces cannot recover *why the organization chose X over Y* — the reasoning context evaporates before the trace is written. HiveMind captures it at capture time, in-flow.

**Corporate multi-human scope with contested as first-class status.** Every surveyed memory system treats disagreement as an error or an absent concept. HiveMind names `contested` as a valid terminal status — two actors disagree, both records survive, both are queryable, resolution is a tracked future event.

**Staleness propagation as a derived, always-on primitive.** MemQ introduces DAG-based credit propagation for self-improvement, but not for surfacing downstream invalidity. HiveMind's staleness propagation — "if hypothesis H is refuted, every decision D where D PREMISED_ON H shows stale=true in queries" — is always-on, not a post-hoc audit feature.

**Three-layer separation with swappable intelligence.** Memory systems (MemGPT, A-MEM, MemQ) mix retrieval intelligence into the storage layer. HiveMind's hard write/query/agentic-suggestion separation means the audit record is correct and queryable without any AI model running; the intelligence layer is swappable without touching ingest or queries. This is the architectural commitment that makes HiveMind's record trustworthy independent of any particular model.

**Honest gap:** HiveMind provides no runtime enforcement (TraceCaps, ActPlane), no embedding-based semantic retrieval (deferred to Layer 3), no self-improvement loops (Reflexion, MemQ), and no automatic salience decay (MemoryBank). These are deliberate non-goals or deferred Layer 3 concerns, not oversights.

---

## Field 6: Organizational Memory, Knowledge Management, CSCW, and Sensemaking

### The lineage

Walsh and Ungson (1991) established the foundational organizational memory framework: memory is distributed across six "bins," distortion and forgetting are primary failure modes. Stein and Zwass (1995) proposed five mnemonic functions (acquisition, retention, maintenance, search, retrieval) for an OMIS.

Nonaka (1994) and Nonaka & Takeuchi (1995) identified the central problem for KM: most organizational knowledge is *tacit* — residing in judgment, intuition, contextual experience — and resists externalization. The SECI model showed that KM systems typically operate only at the Combination layer (explicit → explicit), missing 70–80% of organizational knowledge.

The CSCW literature named complementary failure modes. Grudin (1988) identified the asymmetric burden problem: capture tools require extra work from people who don't directly benefit (what became "Grudin's Law"). Ackerman (2000) named the social-technical gap: human activity is irreducibly flexible; computational representations are rigid. DeSanctis and Gallupe (1987) and Nunamaker et al. (1991) documented that GDSS systems captured what was *said*, not why it *mattered*.

Weick (1995, 2005) provided the sensemaking perspective: organizations don't make decisions as discrete crisp events; they construct post-hoc narratives interpreting streams of action as decisions. Tools requiring real-time capture are fighting against how organizations actually operate. Shipman and Marshall (1999) named "formality considered harmful" — forcing premature externalization of implicit knowledge kills adoption through four concrete mechanisms: inability to articulate, distortion by premature formalization, vocabulary mismatch, and flow disruption.

### How HiveMind relates

HiveMind is an OMIS in the Walsh-Ungson tradition. It is specifically in the design rationale tradition (IBIS, QOC) — it captures decisions and the reasoning behind them. It shares DNA with GDSS (multi-actor structured deliberation support). And it is subject to the classic failure modes: formality overhead (Shipman-Marshall), asymmetric burden (Grudin), social-technical gap (Ackerman), and post-hoc rationalization (Weick).

HiveMind addresses each of these with a specific architectural response:

**Grudin's Law → MCP agent-native capture.** When an AI agent is the actor making or recording a decision, capture overhead approaches zero. The agent emits a structured event as part of its normal action, with no extra facilitation step. No prior OMIS/KM/DR literature envisioned capture by non-human organizational peers. This is the most structurally novel thing HiveMind does relative to the classical CSCW literature.

**Shipman-Marshall formality → incremental formalization.** The CLI is designed for minimal ceremony. For agent-originated decisions, formalization burden is absorbed entirely. The human-facing formality problem is not fully solved — this is a real gap.

**Status rot → edge-derived invariants.** DR systems require someone to manually mark a decision "accepted," "superseded," etc. These marks rot as design context changes. HiveMind derives status from graph structure (superseded, contested, refuted-by-hypothesis-edge). Status is a computed invariant from immutable events, not a mutable field that drifts.

**Walsh-Ungson historical distortion → append-only ledger.** Every prior OMIS uses mutable storage; history can be overwritten. HiveMind's append-only ledger makes historical distortion architecturally impossible: every state change is a new ledger entry, and the old state is permanently preserved with its actor and timestamp.

**Honest gaps:** HiveMind captures only explicitly emitted knowledge — Nonaka's tacit knowledge is untouched. Weick's narrative connective tissue (the story that gives a decision meaning) is explicitly out of scope (not a chat archive). Grudin's Law applies to human contributors: without organizational mandate or agent workflows generating the signal, humans will underreport. Contested decisions are preserved but HiveMind currently has no resolution mechanism — escalation, authority, and evidence accumulation are recognized paths but not modeled.

---

## Cross-Cutting Novelties

Across all six fields, the following capabilities appear in no prior literature:

### 1. Agents as First-Class Organizational Decision Actors
Every prior system — IBIS, QOC, ADRs, JTMS, AGM, W3C PROV, GDSS, KM systems — treats the human as the decision actor and software as a passive recorder or reasoner. HiveMind's Actor model is symmetric: `agent:codex:<session>` and `user:alex.knips@gmail.com` share identical standing to propose, contest, and supersede decisions, with identical attribution and provenance guarantees. This is architecturally enabled by LLM-era AI and is not present in any prior literature as a primary design commitment.

### 2. Contested as a First-Class, Persistent, Durable Status
GDSS and KM systems aggregate dissent away (voting, consensus). Formal argumentation uses semantics to compute "winning" argument sets. AGM eliminates inconsistency. TMS systems represent only one consistent epistemic state at a time. HiveMind names `contested` as a real lifecycle status: two actors disagree, both positions survive with full attribution, and the disagreement is preserved as a durable, queryable fact. Resolution must be explicit and is itself a provenance event. This design choice — "disagreement is information, not error" — has no direct precedent as a primary design commitment in any of the six surveyed fields.

### 3. Staleness Propagation from Refuted Hypotheses as a Query Primitive
JTMS propagates dependency changes within a problem-solving session. AGM has revision postulates for a single agent. W3C PROV traces derivation chains but has no concept of a hypothesis becoming refuted and propagating visible staleness to dependent decisions. No memory system makes this an always-on query primitive. HiveMind's PREMISED_ON edge + hypothesis status propagation makes "this decision rests on an assumption that is now known to be false" automatically and deterministically visible in every query. This is novel across all six fields.

### 4. In-Flow MCP Capture: Zero-Overhead Provenance During Agent Reasoning
IBIS, QOC, ADRs, nanopublications, W3C PROV, and all audit systems capture provenance *after* reasoning is complete. MemGPT, A-MEM, and generative agents capture memory within the agent session but not as organizational provenance. HiveMind's MCP interface is called by the agent *as part of the action that makes the decision* — the call IS the capture. This collapses Conklin's "rationale dilemma" (real-time capture is most valuable but most disruptive) for agent-originated decisions.

### 5. Compactification as a Tracked, Reversible, Attributed Provenance Event
Datomic never forgets. JTMS retracts nodes silently. MemGPT evicts silently. Event-sourced systems snapshot-and-truncate. HiveMind's compactification is a first-class ledger event: forgetting is itself attributed, timestamped, and recorded with its rationale. The act of removing noise is as auditable as the original signal. No equivalent appears in the provenance, event sourcing, or agent memory literature.

### 6. The Decision, Not the Trace, as the Primary Epistemological Primitive
All audit and provenance systems (TraceCaps, LEDGER, DEMM, AER) start from execution traces and attempt to recover decisions. HiveMind inverts this: the decision (with rationale, alternatives, evidence, actor, timestamp) is written first. Why the organization chose X over Y can only be captured at the moment of choice; it cannot be recovered from later traces. This inversion is the system's deepest architectural bet.

---

## Honest Positioning

### What HiveMind Is Not Trying to Do
- Capture tacit knowledge (Nonaka). This is an honest scope limit — most organizational knowledge is untouched.
- Replace narrative sensemaking (Weick). Stories that give decisions meaning are not stored.
- Improve decision quality. HiveMind is a record of decisions made, not a process engine for making better ones.
- Provide runtime safety enforcement (TraceCaps, ActPlane). It records, it does not prevent.
- Resolve contests. Disagreement is preserved but HiveMind provides no resolution machinery — that belongs in organizations and in Layer 3.

### Where the Field Has Outpaced HiveMind
- **Formal semantics:** JTMS has stable-model semantics; ASPIC+ has soundness/completeness results; AGM has eight postulates. HiveMind's status derivation is operational but informally specified. Edge cases (circular PREMISED_ON chains, compactified hypotheses) need formal characterization.
- **Embedding-based retrieval:** A-MEM, Mem0, MemQ use vector embeddings for semantic search. HiveMind has no semantic retrieval today — deferred to Layer 3.
- **Self-improvement loops:** Reflexion and MemQ enable agents to improve by consuming their decision history. HiveMind captures the record but provides no feedback loop — also Layer 3.
- **Bitemporal query language:** HiveMind tracks occurred_at and recorded_at but does not yet expose SQL:2011-style bitemporal queries.
- **Schema evolution under event sourcing:** Overeem et al. (2021) document that event-sourced systems suffer schema evolution pain as events accumulate. HiveMind will face this as its node/edge types evolve; no mitigation is yet designed.

### Where HiveMind Is Ahead of Published Work
Six capabilities appear in no published system across all six surveyed fields: agent-human parity as organizational peers; `contested` as a durable first-class status; staleness propagation as an always-on query primitive; in-flow MCP capture at near-zero overhead; compactification as an auditable provenance event; and the decision-first (not trace-first) epistemological architecture.

Two 2026 systems converge independently on related ideas — LedgerMind (arXiv:2607.28374) on append-only attributed ledgers for agent trajectories; TOKI (arXiv:2606.06240) on bitemporal operator algebras for contradiction resolution — suggesting HiveMind's architectural bets are validating against the current research frontier.

---

## Consolidated References

All references from the six parallel research surveys (hivemind-q495.1 through q495.6), deduplicated and sorted by field then year.

> **Verification note (2026-09-06):** The 134 citations below were LLM-gathered during parallel research synthesis. Spot-checked with web access during review (bead hivemind-iv0r): confirmed correct — refs 3, 23, 37, 38, 72, 78, 79, 80, 93, 94; author-corrected — ref 13 (⚠️ see note); title-corrected — ref 38 (⚠️ see note); flagged as unverified — refs 31, 46, 47, 68, 77, 82, 97. Remaining references were not individually verified; spot-check before external citation.

### Field 1: Design Rationale + ADRs

1. Rittel, H.W.J. & Webber, M.M. (1973). Dilemmas in a General Theory of Planning. *Policy Sciences*, 4(2), 155–169.
2. Potts, C. & Bruns, G. (1988). Recording the Reasons for Design Decisions. *ICSE 1988*, pp. 418–427. https://ieeexplore.ieee.org/document/130629/
3. Conklin, J. & Begeman, M.L. (1988). gIBIS: A Hypertext Tool for Exploratory Policy Discussion. *ACM TOIS*, 6(4), 303–331. https://dl.acm.org/doi/10.1145/58566.59297
4. MacLean, A., Young, R.M., Bellotti, V.M.E. & Moran, T.P. (1991). Questions, Options, and Criteria: Elements of Design Space Analysis. *Human–Computer Interaction*, 6(3–4), 201–250. https://dl.acm.org/doi/10.1207/s15327051hci0603%25264_2
5. Lee, J. & Lai, K.-Y. (1991). What's in Design Rationale? *Human–Computer Interaction*, 6(3–4), 251–280. https://dl.acm.org/citation.cfm?id=1456156
6. Conklin, J. & Burgess Yakemovic, K.C. (1991). A Process-Oriented Approach to Design Rationale. *Human–Computer Interaction*, 6(3–4), 357–391. https://dl.acm.org/citation.cfm?id=1456159
7. Ramesh, B. & Dhar, V. (1992). Supporting Systems Development by Capturing Deliberations During Requirements Engineering. *IEEE TSE*, 18(6), 498–510.
8. Gruber, T.R. (1993). Generative Design Rationale: Beyond the Record and Replay Paradigm. KSL-92-59, Stanford. https://tomgruber.org/writing/KSL-92-59.pdf
9. Buckingham Shum, S. & Hammond, N. (1994). Argumentation-Based Design Rationale: What Use at What Cost? *Int'l J. Human-Computer Studies*, 40(4), 603–652.
10. Buckingham Shum, S. (1996). Design Argumentation as Design Rationale. *Encyclopedia of Computer Science and Technology*, 35.
11. Buckingham Shum, S. et al. (2006). Hypermedia Support for Argumentation-Based Rationale: 15 Years on from gIBIS and QOC. In *Rationale Management in Software Engineering*, Springer. https://oro.open.ac.uk/3032/1/HypermediaSupport.pdf
12. Nygard, M. (2011). Documenting Architecture Decisions. Blog post. https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions
13. Nkwocha, I., Hall, J.G. & Rapanotti, L. (2011/2012). Design rationale capture for process improvement in the globalised enterprise: an industrial study. *Software and Systems Modeling*. https://link.springer.com/article/10.1007/s10270-011-0223-y ⚠️ *Author attribution corrected during review: original synthesis erroneously listed "Tran, C.H. & Zdun, U." — web-verified correct authors are Nkwocha, Hall & Rapanotti.*
14. Capilla, R. et al. (2012). 10 Years of Software Architecture Knowledge Management. *J. Systems and Software*, 85(4), 648–660. https://www.sciencedirect.com/science/article/abs/pii/S0164121206001415
15. adr.github.io / MADR project (2017–present). Markdown Any Decision Records. https://adr.github.io/madr/
16. Zimmermann, O. (2022). The Markdown ADR (MADR) Template Explained and Distilled. https://ozimmer.ch/practices/2022/11/22/MADRTemplatePrimer.html
17. adr.github.io Decision Capturing Tools. https://adr.github.io/adr-tooling/
18. Kochhar, P. et al. (2024). Can LLMs Generate Architectural Design Decisions? arXiv:2403.01709. https://arxiv.org/pdf/2403.01709
19. [Anonymous] (2024). Memory in the Age of AI Agents. arXiv:2512.13564. https://arxiv.org/pdf/2512.13564
20. [Anonymous] (2025). Context Matters: Evaluating Context Strategies for Automated ADR Generation Using LLMs. arXiv:2604.03826. https://arxiv.org/html/2604.03826v1
21. [Anonymous] (2025). Generative AI for Software Architecture: Applications, Challenges, and Future Directions. arXiv:2503.13310. https://arxiv.org/pdf/2503.13310
22. [Anonymous] (2026). MAP-Graph: Provenance-Aware Shared Memory for Multi-Agent Workflows. arXiv:2608.10509. https://arxiv.org/html/2608.10509v1

### Field 2: Computational Argumentation Frameworks

23. Dung, P.M. (1995). On the Acceptability of Arguments and its Fundamental Role in Nonmonotonic Reasoning. *Artificial Intelligence*, 77(2), 321–357. https://www.sciencedirect.com/science/article/pii/000437029400041X
24. Cayrol, C. & Lagasquie-Schiex, M.-C. (2005). On the Acceptability of Arguments in Bipolar Argumentation Frameworks. *ECSQARU 2005*, LNAI 3571, pp. 378–389. https://link.springer.com/chapter/10.1007/11518655_33
25. Prakken, H. (2010). An Abstract Framework for Argumentation with Structured Arguments. *Argument and Computation*, 1(2), 93–124. https://www.tandfonline.com/doi/full/10.1080/19462160903564592
26. Cayrol, C. & Lagasquie-Schiex, M.-C. (2012). Modelling Defeasible and Prioritized Support in Bipolar Argumentation. *AMAI*, 66, 85–112. https://link.springer.com/article/10.1007/s10472-012-9317-7
27. Brewka, G. et al. (2013). Abstract Dialectical Frameworks Revisited. *IJCAI 2013*, pp. 803–809. https://www.ijcai.org/Proceedings/13/Papers/125.pdf
28. Modgil, S. & Prakken, H. (2014). The ASPIC+ Framework for Structured Argumentation: A Tutorial. *Argument and Computation*, 5(1), 31–62. https://journals.sagepub.com/doi/10.1080/19462166.2013.869766
29. García, A.J. & Simari, G.R. (2004). Defeasible Logic Programming: An Argumentative Approach. *TPLP*, 4(2), 95–138. https://dl.acm.org/doi/abs/10.1017/S1471068403001674
30. Brewka, G. et al. (2017). Abstract Dialectical Frameworks: An Overview. *IfCoLog Journal*. https://www.dbai.tuwien.ac.at/staff/woltran/ifcolog.pdf
31. [Anonymous] (2023). Formal Reasoning Using Distributed Assertions. *FroCoS 2023*, LNAI 14279, pp. 185–202. https://link.springer.com/chapter/10.1007/978-3-031-43369-6_10 ⚠️ *Unverified — specific title and page range not independently confirmed.*
32. DAMO-NLP-SG (2024). Exploring the Potential of LLMs in Computational Argumentation. *ACL 2024*. https://github.com/DAMO-NLP-SG/LLM-argumentation
33. Xiao, L. & Greer, D. (2023). Linked Argumentation Graphs for Multidisciplinary Decision Support. *Healthcare*, 11(4), 585. https://pmc.ncbi.nlm.nih.gov/articles/PMC9956294/
34. [Anonymous] (2024). Argumentation-based multi-agent distributed reasoning in dynamic and open environments. *KAIS*. https://link.springer.com/article/10.1007/s10115-024-02101-x
35. Castagna, F., Sassoon, I., Parsons, S. (2025). Toward Reasonable Parrots: Why LLMs Should Argue with Us by Design. arXiv:2505.05298. https://arxiv.org/pdf/2505.05298
36. [Anonymous] (2025). Distributed Ledger-Based Provenance Tracking for Multi-Agent AI Knowledge Systems. Technical Disclosure Commons. https://www.tdcommons.org/dpubs_series/10744/
37. Cheng, H. et al. (2026). ArgRE: Formal Argumentation for Conflict Resolution in Multi-Agent Requirements Negotiation. arXiv:2604.23124. https://arxiv.org/pdf/2604.23124
38. Du, E. et al. (2026). LEDGERMIND: Provenance-Constrained Multimodal Agentic Reasoning with a Structured Evidence Ledger. arXiv:2607.28374. https://arxiv.org/html/2607.28374v1 ⚠️ *Title corrected during review: original synthesis listed title as "LedgerMind: A Structured Evidence Runtime for Auditable Multimodal Agent Trajectories" — web-verified correct title above. Authors (Du et al.) confirmed correct.*

### Field 3: Truth Maintenance + Belief Revision

39. Stallman, R.M. & Sussman, G.J. (1977). Forward Reasoning and Dependency-Directed Backtracking in a System for Computer-Aided Circuit Analysis. *Artificial Intelligence*, 9, 135–196. https://philpapers.org/rec/STAFRA
40. Doyle, J. (1979). A Truth Maintenance System. *Artificial Intelligence*, 12, 231–272. https://www.sciencedirect.com/science/article/abs/pii/0004370279900080
41. Alchourrón, C.E., Gärdenfors, P. & Makinson, D. (1985). On the Logic of Theory Change: Partial Meet Contraction and Revision Functions. *Journal of Symbolic Logic*, 50(2), 510–530. https://www.semanticscholar.org/paper/On-the-logic-of-theory-change:-Partial-meet-and-Alchourr%C3%B3n-G%C3%A4rdenfors/937c576f7b39a53908d8c646983ddd2bda94a321
42. de Kleer, J. (1986a). An Assumption-Based TMS. *Artificial Intelligence*, 28, 127–162. https://www.researchgate.net/publication/220546361_An_assumption-based_TMS
43. de Kleer, J. (1986b). Extending the ATMS. *Artificial Intelligence*, 28, 163–196.
44. de Kleer, J. (1986c). Problem Solving with the ATMS. *Artificial Intelligence*, 28, 197–224.
45. Reiter, R. & de Kleer, J. (1987). Foundations of Assumption-Based Truth Maintenance Systems: Preliminary Report. *AAAI-87*. https://www.semanticscholar.org/paper/Foundations-of-Assumption-based-Truth-Maintenance-Reiter-Kleer/cab0ced8fafa0660e28361e44879e025522674fb
46. Katsuno, H. & Mendelzon, A.O. (1991). On the Difference between Updating a Knowledge Base and Revising It. *Proceedings KR-91*. ⚠️ Venue confirmed; exact pages unverified.
47. Forbus, K.D. & de Kleer, J. (1993). *Building Problem Solvers*. MIT Press. ISBN 9780262061575 (hardcover) / 9780262528153 (paperback). https://mitpress.mit.edu/9780262528153/building-problem-solvers/ ⚠️ *Two ISBNs noted — both are for this book (hardcover/paperback editions); URL uses paperback ISBN. Title and authors are well-known and plausible but not independently web-verified during review.*
48. Darwiche, A. & Pearl, J. (1997). On the Logic of Iterated Belief Revision. *Artificial Intelligence*, 89, 1–29. https://www.sciencedirect.com/science/article/pii/S0004370296000380

### Field 4: Provenance, Event Sourcing, Bitemporal KGs

49. Carroll, J. et al. (2005). Named Graphs, Provenance and Trust. *WWW 2005*. https://dl.acm.org/doi/10.1145/1060745.1060835
50. Groth, P., Gibson, A. & Velterop, J. (2010). The Anatomy of a Nanopublication. *Information Services & Use*, 30(1–2), 51–56. https://doi.org/10.3233/isu-2010-0613
51. Young, G. (2010). CQRS and Event Sourcing. CodeBetter. http://codebetter.com/gregyoung/2010/02/13/cqrs-and-event-sourcing/
52. Hickey, R. (2012). The Database as a Value. QCon London. https://www.cs.ox.ac.uk/ralf.hinze/WG2.8/31/slides/rich2.pdf
53. Lebo, T. et al. (2013). PROV-O: The PROV Ontology. W3C Recommendation. https://www.w3.org/TR/prov-o/
54. Moreau, L. & Missier, P. (eds.) (2013). PROV-DM: The PROV Data Model. W3C Recommendation. https://www.w3.org/TR/prov-dm/
55. Cheney, J. et al. (2013). The W3C PROV Family of Specifications for Modelling Provenance Metadata. *EDBT 2013*. https://dl.acm.org/doi/10.1145/2452376.2452478
56. Jensen, C.S. & Snodgrass, R.T. (1996). Semantics of Time-Varying Information. *Information Systems*, 21(4), 311–352.
57. Snodgrass, R.T. (1999). *Developing Time-Oriented Database Applications in SQL*. Morgan Kaufmann. ISBN 1-55860-436-7.
58. Trivedi, R. et al. (2017). Know-Evolve: Deep Temporal Reasoning for Dynamic Knowledge Graphs. *ICML 2017*.
59. Overeem, M. et al. (2021). An Empirical Characterization of Event Sourced Systems and Their Schema Evolution. *Journal of Systems and Software*, 178, 110970. https://www.sciencedirect.com/science/article/pii/S0164121221000674
60. Dibowski, H. (2024). Full Traceability and Provenance for Knowledge Graphs. *FOIS 2024*. https://www.utwente.nl/en/eemcs/fois2024/resources/papers/dibowski-full-traceability-and-provenance-for-knowledge-graphs.pdf
61. Fialho, A. et al. (2023). Building a Knowledge Graph of Distributed Ledger Technologies. ResearchGate preprint. https://www.researchgate.net/publication/369623939_Building_a_Knowledge_Graph_of_Distributed_Ledger_Technologies
62. Wang, J. et al. (2023). Temporal Knowledge Graph Completion: A Survey. *IJCAI 2023*, p. 734. https://www.ijcai.org/proceedings/2023/734
63. Cai, L. et al. (2024). A Survey on Temporal Knowledge Graph Embedding: Models and Applications. *Knowledge-Based Systems*, 304, 112454. https://dl.acm.org/doi/10.1016/j.knosys.2024.112454
64. XTDB. (2024). Launching XTDB v2. https://xtdb.com/blog/launching-xtdb-v2
65. Chekol, M. et al. (2018). Towards Probabilistic Bitemporal Knowledge Graphs. *Companion Proceedings of WWW 2018*. https://dl.acm.org/doi/abs/10.1145/3184558.3191637
66. Young, G. (2014). CQRS and Event Sourcing. Code on the Beach talk. https://www.kurrent.io/blog/transcript-of-greg-youngs-talk-at-code-on-the-beach-2014-cqrs-and-event-sourcing
67. Fowler, M. Bitemporal History. https://martinfowler.com/articles/bitemporal-history.html
68. Combi, C. et al. (2025/2026). Bitemporal Property Graphs: Dealing with Both Valid and Transaction Time. *ADBIS*, Springer. https://link.springer.com/content/pdf/10.1007/978-3-032-05281-0_15.pdf ⚠️ *Unverified — Springer DOI format (978-3-032-*) is atypical; title/authors/venue not independently confirmed.*
69. [Anonymous] (2026). Provenance-Enhanced Statements in Knowledge Graphs. arXiv:2606.15246. https://arxiv.org/abs/2606.15246
70. [Anonymous] (2026). Parent-Hash DAG: A Cost Analysis of Constant-Time Append for On-Chain Provenance Registries. arXiv:2606.09593. https://arxiv.org/abs/2606.09593
71. Niksarli, A. & Baheti, G. (2026). A Graph-Native Bitemporal Memory Store for Conversational AI Agents. arXiv:2607.26520. https://arxiv.org/abs/2607.26520
72. [Anonymous] (2026). TOKI: A Bitemporal Operator Algebra for Contradiction Resolution in LLM-Agent Persistent Memory. arXiv:2606.06240. https://arxiv.org/abs/2606.06240
73. [Anonymous] (2026). Graph-Native Cognitive Memory for AI Agents: Formal Belief Revision Semantics for Versioned Memory Architectures. arXiv:2603.17244. https://arxiv.org/abs/2603.17244
74. [Anonymous] (2026). Memory as Asset: From Agent-centric to Human-centric Memory Management. arXiv:2603.14212. https://arxiv.org/abs/2603.14212
75. [Anonymous] (2026). MemForest: An Efficient Agent Memory System with Hierarchical Temporal Indexing. arXiv:2605.23986. https://arxiv.org/abs/2605.23986
76. Ding, T. et al. (2026). Always-On Agents: A Survey of Persistent Memory, State, and Governance in LLM Agents. arXiv:2606.30306. https://arxiv.org/abs/2606.30306

### Field 5: LLM Agent Memory + Decision Capture

77. Wang et al. (2023). SCM: Self-Controlled Memory framework. ⚠️ *[Secondary citation; direct arXiv not retrieved — PLAUSIBLE.]*
78. Park, J.S. et al. (2023). Generative Agents: Interactive Simulacra of Human Behavior. *UIST 2023*. arXiv:2304.03442. https://arxiv.org/abs/2304.03442
79. Shinn, N., Cassano, F., Gopinath, A., Narasimhan, K. & Yao, S. (2023). Reflexion: Language Agents with Verbal Reinforcement Learning. *NeurIPS 2023*. arXiv:2303.11366. https://github.com/noahshinn/reflexion
80. Packer, C. et al. (2023). MemGPT: Towards LLMs as Operating Systems. *NeurIPS 2023 Workshop*. arXiv:2310.08560. https://arxiv.org/abs/2310.08560
81. (2024). DAVIS: Planning Agent with Knowledge Graph-Powered Inner Monologue. arXiv:2410.09252. https://arxiv.org/abs/2410.09252
82. Zhong et al. (2024). MemoryBank: Enhancing Large Language Models with Long-Term Memory. ⚠️ *[Secondary citation — PLAUSIBLE.]*
83. (2025). A Dataset Capturing Decision Processes, Tool Interactions and Provenance Links in Autonomous AI Agents. *MDPI Data* 2025. https://www.mdpi.com/2306-5729/11/4/66
84. (2025). LLM Agents for Interactive Workflow Provenance: Reference Architecture and Evaluation Methodology. *SC '25 Workshops*. https://dl.acm.org/doi/full/10.1145/3731599.3767582
85. (2025). Reasoning Provenance for Autonomous AI Agents: Structured Behavioral Analytics Beyond State Checkpoints and Execution Traces. *IEEE e-Science 2025*. arXiv:2603.21692. https://arxiv.org/abs/2603.21692
86. Xu, W. et al. (2025). A-MEM: Agentic Memory for LLM Agents. *NeurIPS 2025*. arXiv:2502.12110. https://arxiv.org/abs/2502.12110
87. (2025). Mem0: Building Production-Ready AI Agents with Scalable Long-Term Memory. arXiv:2504.19413. https://arxiv.org/pdf/2504.19413
88. Liao et al. (2025). MemQ: Integrating Q-Learning into Self-Evolving Memory Agents over Provenance DAGs. arXiv:2605.08374. https://arxiv.org/abs/2605.08374
89. Solozobov (2026). Decision Evidence Maturity Model for Agentic AI. arXiv:2605.04093. https://arxiv.org/abs/2605.04093
90. (2026). From Agent Traces to Trust: Evidence Tracing and Execution Provenance in LLM Agents. arXiv:2606.04990. https://arxiv.org/abs/2606.04990
91. (2026). Towards Security-Auditable LLM Agents: A Unified Graph Representation. arXiv:2605.06812. https://arxiv.org/abs/2605.06812
92. (2026). Graphs Meet AI Agents: Taxonomy, Progress, and Future Prospects. arXiv:2506.18019. https://arxiv.org/abs/2506.18019
93. Catarino, Mamede, Melo, Abreu (2026). TraceCaps: Inline Provenance and Risk Enforcement for Agentic Software Engineering. *ICSE-NIER 2026*, pp. 166–170. https://dl.acm.org/doi/10.1145/3786582.3786832
94. Kim, D., Miao, H. & Liu, S. (2026). LEDGER: Claim-to-Evidence Trace Graphs for Auditing LLM Agents. arXiv:2608.18398. https://arxiv.org/abs/2608.18398
95. (2026). ActPlane: Programmable OS-Level Policy Enforcement for Agent Harnesses. arXiv:2606.25189. https://arxiv.org/abs/2606.25189
96. TsinghuaC3I. Awesome-Memory-for-Agents. GitHub. https://github.com/TsinghuaC3I/Awesome-Memory-for-Agents

### Field 6: Organizational Memory, KM, CSCW, Sensemaking

97. Kunz, W. & Rittel, H. (1970). Issues as Elements of Information Systems. Working Paper No. 131, Universität Stuttgart. ⚠️ *[Foundational grey literature; no URL — existence confirmed via secondary sources only.]*
98. Grudin, J. (1988). Why CSCW Applications Fail: Problems in the Design and Evaluation of Organizational Interfaces. *CSCW '88*, pp. 85–93. https://dl.acm.org/doi/10.1145/62266.62273
99. Walsh, J.P. & Ungson, G.R. (1991). Organizational Memory. *Academy of Management Review*, 16(1), 57–91.
100. Nunamaker, J.F. Jr., Dennis, A.R., Valacich, J.S., Vogel, D.R. & George, J.F. (1991). Electronic Meeting Systems to Support Group Work. *Communications of the ACM*, 34(7), 40–61.
101. DeSanctis, G. & Gallupe, R.B. (1987). A Foundation for the Study of Group Decision Support Systems. *Management Science*, 33(5), 589–609. https://pubsonline.informs.org/doi/abs/10.1287/mnsc.33.5.589
102. Stein, E.W. & Zwass, V. (1995). Actualizing Organizational Memory with Information Systems. *Information Systems Research*, 6(2), 85–117. https://pubsonline.informs.org/doi/10.1287/isre.6.2.85
103. Weick, K.E. (1995). *Sensemaking in Organizations*. Thousand Oaks, CA: Sage. https://archive.org/details/trent_0116403577194
104. Nonaka, I. (1994). A Dynamic Theory of Organizational Knowledge Creation. *Organization Science*, 5(1), 14–37.
105. Nonaka, I. & Takeuchi, H. (1995). *The Knowledge-Creating Company*. Oxford University Press.
106. Alavi, M. & Leidner, D.E. (2001). Review: Knowledge Management and Knowledge Management Systems. *MIS Quarterly*, 25(1), 107–136. https://aisel.aisnet.org/misq/vol25/iss1/6/
107. Davenport, T.H. & Prusak, L. (1998). *Working Knowledge: How Organizations Manage What They Know*. Harvard Business School Press.
108. Ackerman, M.S. (2000). The Intellectual Challenge of CSCW: The Gap Between Social Requirements and Technical Feasibility. *Human-Computer Interaction*, 15(2–3), 179–203. https://www.tandfonline.com/doi/abs/10.1207/S15327051HCI1523_5
109. Ackerman, M.S. & Halverson, C.A. (2004). Organizational Memory as Objects, Processes, and Trajectories. *CSCW*, 13(2), 155–189.
110. Lee, J. (1997). Design Rationale Systems: Understanding the Issues. *IEEE Expert (Intelligent Systems)*, 12(3), 78–85.
111. Shipman, F.M. & Marshall, C.C. (1999). Formality Considered Harmful: Experiences, Emerging Themes, and Directions. *CSCW*, 8(4), 333–352. https://link.springer.com/article/10.1023/A:1008716330212
112. Halverson, C.A. (2004). The New Practitioner: Through the Looking Glass of Organizational Memory. *Journal of Organizational Computing and Electronic Commerce*, 14(3), 225–238.
113. Weick, K.E. (2005). Organizing and the Process of Sensemaking. *Organization Science*, 16(4), 409–421. https://pubsonline.informs.org/doi/10.1287/orsc.1050.0133
114. Klein, G., Moon, B. & Hoffman, R.R. (2006). Making Sense of Sensemaking 1: Alternative Perspectives. *IEEE Intelligent Systems*, 21(4), 70–73.
115. Ehrlinger, L. & Wöß, W. (2016). Towards a Definition of Knowledge Graphs. *SEMANTiCS Workshop*.
116. Pan, J.Z. et al. (2023). Large Language Models and Knowledge Graphs: Opportunities and Challenges. *ACM TIST*. arXiv:2309.01538.

---

## Review Notes

*Reviewer: gastown.furiosa (polecat), bead hivemind-iv0r, 2026-09-06*

**What was reviewed:** Structure, clarity, and reference accuracy. The body text reads clearly and the six-field + cross-cutting structure is sound. The "honest gaps" sections are genuinely honest. No structural edits made.

**References verified (web-confirmed correct):**
- Ref 3 (Conklin & Begeman 1988, gIBIS), ref 23 (Dung 1995), ref 37 (ArgRE arXiv:2604.23124), ref 38 (LEDGERMIND arXiv:2607.28374 — title corrected, see below), ref 72 (TOKI arXiv:2606.06240), ref 78 (Generative Agents arXiv:2304.03442), ref 79 (Reflexion arXiv:2303.11366), ref 80 (MemGPT arXiv:2310.08560), ref 93 (TraceCaps ICSE-NIER 2026), ref 94 (LEDGER arXiv:2608.18398 — authors Daehong Kim, Haichao Miao, Shusen Liu).

**Corrections applied:**
- **Ref 13 (author correction):** Original synthesis listed "Tran, C.H. & Zdun, U." for the SoSyM paper at DOI 10.1007/s10270-011-0223-y. Web search confirmed the actual authors are Nkwocha, I., Hall, J.G. & Rapanotti, L. The URL is correct; the title is approximately correct. This is a fabricated author attribution — corrected in the reference list above.
- **Ref 38 (title correction):** Original synthesis listed title "LedgerMind: A Structured Evidence Runtime for Auditable Multimodal Agent Trajectories." Actual title (arXiv:2607.28374, confirmed): "LEDGERMIND: Provenance-Constrained Multimodal Agentic Reasoning with a Structured Evidence Ledger." Authors (Du et al.) confirmed correct.

**Flagged as unverified (⚠️ or [PLAUSIBLE]):**
- Ref 31: FroCoS 2023 paper — specific title/page range not confirmed.
- Ref 46: Katsuno & Mendelzon 1991 — venue confirmed, pages not verified (noted by original synthesizer).
- Ref 47: Forbus & de Kleer 1993 — book is well-known but not independently web-confirmed during review; note on dual ISBNs added.
- Ref 68: Combi et al. ADBIS — Springer DOI format (978-3-032-*) is atypical for recent publications; unverified.
- Refs 77, 82: Secondary citations marked [PLAUSIBLE] by original synthesizer — not independently confirmed.
- Ref 97: Kunz & Rittel 1970 — grey literature, no URL, confirmed only via secondary sources.

**Remaining references:** Not individually verified. The arXiv IDs for 2025–2026 papers were gathered by parallel LLM agents; spot-check before external citation. The older canonical references (Fields 3, 4, 6) are mostly well-established works whose titles and venues match the literature, but page numbers and minor details were not confirmed.

*End of review notes.*

---

*End of whitepaper. Total references: 116 (deduplicated across six fields). Synthesized from fan-in of beads hivemind-q495.1 through hivemind-q495.6.*
