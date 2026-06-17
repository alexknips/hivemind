# Graph Compactification — Signal/Noise Spec

**Status:** Draft for alex review  
**Bead:** hivemind-6dc2.3  
**Layer:** 3 (agentic / swappable)  
**Constraint:** View-only — no ledger writes, no Layer-1 or Layer-2 mutations

---

## What compactification is (and is not)

Compactification is a **Layer-3 query** that collapses a decision subgraph into a digestible view. It is not a compaction of the ledger — the underlying event log is immutable and untouched. "Reversible" in this context means: the full view is always recoverable by querying without compaction. "Tracked" means: the compact view declares itself as such and reports what was elided, so a caller can audit what they are not seeing.

Silent compaction — returning a partial result that looks complete — is prohibited by the honesty rails in AGENTS.md.

---

## Signal/Noise Decision Table

The unit of compactification is a **focal decision** (a decision_id) and its subgraph as returned by `get_decision_neighborhood`. For a focal decision that is part of a supersession chain, the focal node is the **terminal (most-current) decision in that chain** unless the caller explicitly named a historical one.

| Element | Classification | Reasoning |
|---------|---------------|-----------|
| Terminal decision — title, rationale, topic_keys | **SIGNAL** | The decision that is in effect now; rationale is non-negotiable |
| Terminal decision — chosen_option_id | **SIGNAL** | The actual choice made |
| Terminal decision — status (accepted / proposed / rejected / contested) | **SIGNAL** | Current operational state |
| Terminal decision — PROPOSED_BY (actor) | **SIGNAL** | Provenance of the current decision |
| Terminal decision — ACCEPTED_BY (actor) | **SIGNAL** if accepted or contested | Who accepted it |
| Terminal decision — REJECTED_BY (actor) | **SIGNAL** if rejected or contested | Who rejected it; both sides of contest are always kept |
| Terminal decision — BASED_ON (evidence) | **SIGNAL** | Supporting facts for the current decision |
| Terminal decision — ASSUMES (hypothesis, status=open) | **SIGNAL** | Uncertain assumptions must be visible |
| Terminal decision — ASSUMES (hypothesis, status=refuted) | **SIGNAL** | Refuted assumptions propagate staleness — hiding them is the worst failure mode |
| Terminal decision — ASSUMES (hypothesis, status=supported) | **COMPACT** — surface status only, drop evidence chain | The assumption is confirmed; detailed evidence is audit-only |
| Superseded decisions in chain (all except terminal) | **COMPACT** — summarize as chain metadata | Their rationales are encoded in what superseded them; full detail available via ledger audit |
| Intermediate chain — PROPOSED_BY, ACCEPTED_BY, REJECTED_BY | **COMPACT** | Historical actor provenance; available in full view |
| Terminal decision — unchosen options (HAS_OPTION but not CHOSE) | **COMPACT** | Options considered but not chosen; useful only for deep audit |
| Evidence BASED_ON a superseded (non-terminal) decision | **COMPACT** | The superseding decision carries its own evidence; old evidence is historical |
| Unresolved blockers (Blocker, no BlockerResolved event) | **SIGNAL** | Active blockers are current operational state |
| Resolved blockers | **COMPACT** | Already handled; pure historical noise |
| Notifications (NotificationSent, NotificationAcknowledged) | **COMPACT** | Operational ephemera; not decision content |
| DecisionRequests that spawned the focal decision | **COMPACT** | Fulfilled; the decision itself is the record |
| Contested decision — both ACCEPTED_BY and REJECTED_BY actors | **SIGNAL — never compact** | Contested is a status, not an error; both sides must be preserved and queryable |

---

## CompactView output shape

```
CompactView {
    // The focal decision (terminal in any supersession chain)
    decision: DecisionView,         // existing type; full title + rationale + status

    // If the decision is part of a supersession chain, summarize it
    supersession_chain: Option<SupersessionSummary>,

    // Present only if decision is contested: both sides
    contest: Option<ContestView>,

    // Hypotheses assumed by the terminal decision, with status
    // Open + refuted hypotheses are included; supported ones included as status-only
    hypotheses: Vec<HypothesisSummaryView>,

    // Evidence IDs supporting the terminal decision (BASED_ON edges)
    evidence_ids: Vec<String>,

    // Unresolved blockers for this decision
    active_blockers: Vec<BlockerSummary>,

    // Transparency: what was elided
    elided: ElidedSummary,
}

SupersessionSummary {
    chain_length: usize,         // total decisions in chain
    oldest_id: String,           // root of the chain (oldest decision)
    // caller can call get_supersession_chain for the full ordered list
}

ContestView {
    accepted_by: Vec<String>,    // actor ids
    rejected_by: Vec<String>,    // actor ids
}

HypothesisSummaryView {
    id: String,
    status: HypothesisStatus,    // open | supported | refuted
    statement: String,
    // refuting_evidence_ids present only when status=refuted (signal that something broke)
    refuting_evidence_ids: Option<Vec<String>>,
}

BlockerSummary {
    id: String,
    reason: String,
    priority: DecisionBlockerPriority,
    blocked_actor_id: String,
}

ElidedSummary {
    superseded_decision_count: usize,
    unchosen_option_count: usize,
    resolved_blocker_count: usize,
    notification_count: usize,
    // Ledger offsets for elided nodes — enables audit without re-querying the compact view
    event_origins: Vec<u64>,
}
```

---

## What the layer-3 function calls (no new graph queries)

The implementation wraps existing Layer-2 functions only:

1. `get_decision(graph, decision_id)` — resolve the focal decision
2. `get_supersession_chain(graph, decision_id)` — find the terminal node + chain metadata
3. `get_decision(graph, terminal_id)` — if terminal differs from focal
4. `get_decision_neighborhood(graph, terminal_id, NeighborhoodRequest::all())` — walk the subgraph
5. `get_active_decision_blockers(graph, ...)` — find unresolved blockers

No direct graph queries (Cypher/SQL) in the compactification layer. All graph access goes through Layer-2.

---

## CLI and MCP surfaces

**CLI (new subcommand under `hivemind query`):**
```
hivemind query compact-view <decision-id> [--summary]
```
Returns `CompactView` as JSON by default; `--summary` renders human-readable text. Mirrors the existing `neighborhood` subcommand pattern.

**MCP tool (new tool in `src/mcp.rs`):**
```
Tool name: hivemind_compact_view
Input:  { decision_id: string }
Output: CompactView (JSON)
```
This satisfies the M3 parent epic's requirement for MCP tool exposure.

---

## Honesty rails

Every `CompactView` response:

1. Is typed distinctly from `NeighborhoodView` — callers cannot mistake it for a full view
2. Includes `elided: ElidedSummary` — counts and event_origins for everything dropped
3. If `elided.superseded_decision_count > 0`, the response includes `supersession_chain` with `oldest_id` so audit is a single follow-up query away
4. A contested decision is **never** compacted — if `decision.status == Contested`, `contest` is always present with both actor lists

---

## What this spec does NOT cover

- LLM-generated narrative summaries — that is the sibling `text summarization + MCP tool` bead under the same M3 epic
- Similarity ranking between decisions — separate Layer-3 feature
- Background compaction jobs — no async workers; compactification is always on-demand (per the M3 epic: "recall is an EXPLICIT pull")
- Ledger pruning or physical deletion — prohibited; the ledger is immutable

---

## Open questions for alex review

1. **Unchosen options on the terminal decision** — the spec marks these COMPACT. Is that the right call, or should they always surface? (They answer "what was considered" but not "what was decided".)

2. **Hypothesis supported-evidence chain** — for `status=supported`, the spec drops the supporting evidence IDs from the compact view. Should the evidence IDs still be listed (just the IDs, not the full content)?

3. **Supersession chain oldest_id only vs full chain** — the spec provides `oldest_id` in `SupersessionSummary` and relies on a follow-up `get_supersession_chain` call for the ordered list. Alternative: embed the full `decision_ids: Vec<String>` in `SupersessionSummary`. Which is cheaper for the caller?

4. **CLI surface** — `hivemind query compact-view` as a new subcommand vs `hivemind query neighborhood --compact` flag. Separate subcommand is cleaner (distinct return type); flag is discoverable. Preference?
